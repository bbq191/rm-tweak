//! 母版库（中间层暂存池）领域模块——三层架构（内容源 → **母版库** → 读器）的交汇点。
//! 三个正交动作各一个方法：**入库**（`stage_new` / `stage_from_path` / [`StagingStore`] 上传模板 / `fetch_article`）、
//! **优化**（`optimize`，只对 EPUB）、**落库**（`deliver` 投 xochitl；KOReader 由 koreader-serve 从同一目录 adopt，
//! 之后前端调 `mark_delivered` 记一笔）。落库＝纯复制母版字节（两读器同字节可对照），母版默认保留可反复落库。
//! 目录 `$XDG_STATE_HOME/shelf/books/staging/`（/home 分区，重启/OTA 不丢；**不套 LRU 淘汰**，留住用户还没落库的书）。
//! 落库记录是同目录隐藏 sidecar `.<name>.delivered`（`sidecar` 模块管读写；本模块只在落库/删书时调它）。
use bookconv::optimize::{self, FootnoteMode, OptimizeOpts};
use crate::mkdir::MkdirQueue;
use crate::render_check;
use crate::sidecar::{self, Delivered, RenderCheck};
use bookconv::wash::WashOpts;
use serde::Serialize;
use rmsvc_core::asset::{AssetItem, AssetStore};
use rmsvc_core::formats::{self, BOOK_EXTS};
use rmsvc_core::fs::{plain_name, unique_path, write_atomic};
use rmsvc_core::xochitl::{Delivery, Xochitl};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 漫画按卷拆分投递，单份等渲染确认的上限（真机观测单卷渲染+缩略图+建索引耗时 15-30 秒，给足
/// 余量；超时不算失败，只是没等到确认就接着投下一份，见 `Staging::try_deliver_split`）。
const PIECE_RENDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// 落库前等待「建文件夹」代理真的建出来目标文件夹的上限——`shelf-mkdir-agent.qmd` 是 8 秒一次
/// Timer 轮询，给够 2-3 个周期的余量；等不到不算错误，`ensure_folder` 会原样放行，交给
/// `Xochitl::upload` 现有的"找不到就落书库根"兜底（改动前就有的行为，不是新错误）。
const FOLDER_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// 优化进度回调节流：`optimize_epub_file_streaming` 阶段二可能有几百个条目（大漫画），每条目都
/// 写 sidecar+推 SSE 事件在这台设备的存储上是真实开销（原子写=开临时文件+写+改名，不是内存操作）；
/// 每 N 条目才落一次盘/推一次事件，首尾两条（第 1 条、最后一条）永远落，保证 UI 能看到"刚开始动"
/// 和"到 100% 了"，中间稀疏一点不影响"看着在动"这个体验目标。
const OPTIMIZE_PROGRESS_STRIDE: usize = 5;

/// 忙锁占用时的统一提示——优化/落库/删除三处几乎逐字重复过（2026-09-19 代码质量审计）。`extra`
/// 是各自独有的后缀（删除那处要额外提示"再删除"），其余传空串。
fn busy_err(name: &str, extra: &str) -> String {
    format!("《{name}》正在处理中，请稍候{extra}")
}

/// 母版库一本书的展示条目。`format`（epub / pdf / cbz / other）从扩展名判、优化等级从内埋标记判（轻量只读中央目录）。
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct StagingEntry {
    pub name: String,
    pub bytes: u64,
    pub format: &'static str,
    /// 是否**当前版本的完整优化**（含清洗层）。`level` 更细：full / core（只跑核心遍，如网文·格式转换产物）/ old / none。
    pub optimized: bool,
    pub level: &'static str,
    /// 入库时间（unix 秒），列表最新在前。
    pub mtime: u64,
    /// 落库记录。时间早于 `mtime`（之后又优化过）= 母版已变，UI 标"旧"。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered: Option<Delivered>,
    /// 是否正有一个异步操作（「优化」或「落库」）在这条目上跑——UI 据此禁用删除/落库/再次优化等按钮，
    /// 防止双击/并发操作同一条目（2026-09-18 真机反馈：优化耗时可能到分钟级，同步阻塞体验像卡死；
    /// 2026-09-19 落库同理补上——超限漫画按卷拆分要挨个建包+上传，同样能拖到分钟级）。
    #[serde(default)]
    pub busy: bool,
}

/// 投原生成功后交给自检线程的计划：投书时刻（毫秒，圈"之后进库"的候选）+ 书名 + 期望页数。
#[derive(Clone, Debug, PartialEq)]
pub struct RenderPlan {
    pub name: String,
    pub title: Option<String>,
    pub expected: u64,
    pub since_ms: u64,
}

/// `deliver` 的结果：回执文案 + （EPUB 才有）渲染自检计划。
#[derive(Debug, PartialEq)]
pub struct DeliverOutcome {
    pub message: String,
    pub render: Option<RenderPlan>,
}

/// `fetch_article` 的结果：落地名 + 标题 + 同步优化态（没请求优化＝两个字段都是"未发生"，不是"失败"）。
#[derive(Debug, PartialEq)]
pub struct FetchArticleOutcome {
    pub name: String,
    pub title: String,
    pub optimized: bool,
    pub optimize_error: Option<String>,
}

/// 落库去向（记录用）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reader {
    Native,
    Koreader,
}

impl Reader {
    pub fn parse(s: &str) -> Result<Reader, String> {
        match s {
            "native" => Ok(Reader::Native),
            "koreader" => Ok(Reader::Koreader),
            _ => Err("target 只能是 native / koreader".into()),
        }
    }
}

#[derive(Clone)]
pub struct Staging {
    dir: PathBuf,
    xochitl: Arc<Xochitl>,
    /// 投原生体积门（字节，0=不拦）：xochitl `/upload` 超限会直接断连，先拦下来给指引。
    native_limit: u64,
    /// 正在跑异步操作（「优化」/「落库」）的条目名集合——进程内存态，**不落盘**：进程重启＝没有任何
    /// 操作还在跑，"忙"状态天然清零是正确语义（不是遗留 bug），比落盘更简单也更不会出现"重启后
    /// 永久卡忙、谁都清不掉"的死锁。sidecar 里的 `OptimizeCheck`/`DeliverCheck`.status 只管"上次
    /// 结果展示"，不参与这个忙锁判断——两者职责分开。加锁是全局唯一入口（`try_start_busy`），
    /// 同一条目「优化」跟「落库」互斥——不允许同时跑（两个都要读/写同一份母版库文件）。
    busy: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
}

impl Staging {
    pub fn new(dir: PathBuf, xochitl: Arc<Xochitl>, native_limit: u64) -> Staging {
        Staging { dir, xochitl, native_limit, busy: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())) }
    }
    /// 这条目当前是否有异步操作在跑。
    pub fn is_busy(&self, name: &str) -> bool {
        self.busy.lock().unwrap().contains(name)
    }
    /// 尝试给条目加忙锁；已经忙着 → false（调用方据此拒绝这次操作，不排队不覆盖）。
    fn try_start_busy(&self, name: &str) -> bool {
        self.busy.lock().unwrap().insert(name.to_string())
    }
    fn end_busy(&self, name: &str) {
        self.busy.lock().unwrap().remove(name);
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }

    /// 母版库里某本书的路径（校验单段文件名）。
    fn path_of(&self, name: &str) -> Result<PathBuf, String> {
        Ok(self.dir.join(plain_name(name)?))
    }
    fn existing(&self, name: &str) -> Result<PathBuf, String> {
        let p = self.path_of(name)?;
        if !p.is_file() {
            return Err("母版库里没有这本书".into());
        }
        Ok(p)
    }

    // ───────────── 入库 ─────────────

    /// 新入库（字节）：原子写，同名加数字前缀不覆盖。返回落地文件名。
    pub fn stage_new(&self, name: &str, bytes: &[u8]) -> Result<String, String> {
        let target = unique_path(&self.dir, plain_name(name)?);
        write_atomic(&target, bytes).map_err(|e| format!("写母版库失败: {e}"))?;
        Ok(landed_name(&target))
    }

    /// 新入库（已落盘的暂存文件）：同分区 rename 不拷贝（上传 / inbox 追平的大书走这里）。返回落地文件名。
    pub fn stage_from_path(&self, name: &str, src: &Path) -> Result<String, String> {
        let target = unique_path(&self.dir, plain_name(name)?);
        if std::fs::rename(src, &target).is_err() {
            std::fs::copy(src, &target).map_err(|e| format!("写母版库失败: {e}"))?;
            let _ = std::fs::remove_file(src);
        }
        Ok(landed_name(&target))
    }

    /// 网文抓取（Readability + 白名单）→ 组 EPUB 落母版库。`optimize`＝网页「同步优化」复选框：请求了就紧接着
    /// 跑一遍跟「母版库→优化」按钮同一个 `optimize()`，不用用户再手动点一次——`article.rs` 的属性
    /// 白名单本来就不留 class/style，正文没有任何 CSS，不经 wash 层的边距/段距归零会在 xochitl 上按默认段距
    /// 渲染出大片留空（真机反馈）。`optimize=false` 保留原行为：core 级落库，用户按需再点。同步优化失败不影响
    /// 入库结果（已经抓到的文章不因为这一步失败就整个丢掉），失败原因原样带回给调用方决定怎么措辞。
    pub fn fetch_article(&self, url: &str, optimize: bool) -> Result<FetchArticleOutcome, String> {
        let (epub, title) = bookconv::article::build_article_epub(url)?;
        let fname = format!("{}.epub", bookconv::util::sanitize_filename(&title, "article"));
        let name = self.stage_new(&fname, &epub)?;
        let (optimized, optimize_error) = if optimize {
            match self.optimize(&name, |_, _| {}) {
                Ok(_) => (true, None),
                Err(e) => (false, Some(e)),
            }
        } else {
            (false, None)
        };
        Ok(FetchArticleOutcome { name, title, optimized, optimize_error })
    }

    // ───────────── 优化 ─────────────

    /// 对母版库里的 EPUB 跑通用优化（清洗+优化：Inline 脚注 + 外链 css 缩进 + 边距段距归零等，两读器
    /// 都能显示），原子回写。返回回执文案。**不再分档位**（2026-09-19 用户明确要求去掉"优化分档位"
    /// 这个选择——原来还有「清洗但保留段距」「只优化不清洗」两档给诗集/剧本/已排好版的书用，但这个
    /// 选择埋在母版库页一个全局下拉里、跟"点哪本书的优化按钮"脱节，容易选错却不易发现；只保留最
    /// 常用、原本就标"推荐"的那档，即完整清洗+优化）。
    /// **同步、阻塞**——大漫画真机实测能跑到分钟级（`trim_margins` 裁边扫描，见书架白皮书 §05），
    /// HTTP 接口不该直接暴露这个方法，用 [`Self::spawn_optimize`] 走后台线程。
    ///
    /// **流式路径**（2026-09-19 真机坐实）：改走 [`optimize::optimize_epub_file_streaming`] 而不是
    /// 整本读进内存的 [`optimize::optimize_epub_with`]——真机拿用户自己传的 552MB《镖人》全集测过
    /// 内存版，`VmRSS` 几十秒冲到 1.4GB+、系统可用内存探底到 ~25MB，逼近全系统级 OOM（book-serve
    /// 自己的 `systemd` `MemoryMax=192M` 没有真正生效，见白皮书 §03az）。流式版峰值内存量级是
    /// "一张图 + 全书文字部分"，不随书变大线性涨，细节见该函数文档注释。
    /// `on_progress(done, total)` 原样转发给 [`optimize::optimize_epub_file_streaming`]（2026-09-19
    /// 补，给 [`Self::spawn_optimize`] 挂真实进度用；这个方法本身不关心怎么展示，不耦合 sidecar/
    /// EventBus——同步调用方（如 [`Self::fetch_article`] 的"同步优化"复选框）传空闭包即可）。
    pub fn optimize(&self, name: &str, mut on_progress: impl FnMut(usize, usize)) -> Result<String, String> {
        if formats::ext_of(name) != "epub" {
            return Err("只有 EPUB 能优化，PDF 不支持".into());
        }
        let p = self.existing(name)?;
        // 点前缀隐藏名——真机 552MB《镖人》全集坐实优化能跑到分钟级（流式虽然不再吃内存，但大书
        // 图片多、逐张处理仍要时间），这份临时产物会在目录里存在相当一段时间；`list()` 本来就按
        // `.` 前缀跳过 sidecar，不带点前缀的话这份半成品会被当成一条离谱的"母版库条目"混进列表
        // （2026-09-19 真机复现：真显示过一条 `format:"other"` 的 `....epub.optimizing.tmp`）。
        let tmp = p.with_file_name(format!(".{}.optimizing.tmp", name));
        let result = optimize::optimize_epub_file_streaming(&p, &tmp, &OptimizeOpts { wash: Some(WashOpts::default()), footnote: FootnoteMode::Anchor }, &mut on_progress);
        let rep = match result {
            Ok(r) => r,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp); // 半成品清掉，不留垃圾在母版库目录
                return Err(e);
            }
        };
        std::fs::rename(&tmp, &p).map_err(|e| format!("回写母版库失败: {e}"))?;
        Ok(format!("已优化《{name}》{}", optimize_note(&rep)))
    }

    /// [`Self::optimize`] 的异步版：先做零耗时的格式/存在性校验（错误立即回给调用方，不用等后台线程），
    /// 校验过了才加忙锁、起后台线程跑真正耗时的部分。成功返回后 HTTP 层应立即回"已开始"，真正结果
    /// 通过 `bus` 的 `books`/`staging` 事件 + `GET /staging` 列表里这条目的 `delivered.optimize`
    /// （[`sidecar::OptimizeCheck`]）异步呈现。`catch_unwind` 兜底优化过程中的 panic（如损坏文件触发
    /// 库内部意外崩溃）——绝不能让忙锁卡死在 true 再也清不掉、这条目从此删不掉优化不了。
    pub fn spawn_optimize(&self, name: &str, bus: Arc<rmsvc_core::events::EventBus>) -> Result<(), String> {
        if formats::ext_of(name) != "epub" {
            return Err("只有 EPUB 能优化，PDF 不支持".into());
        }
        self.existing(name)?;
        if !self.try_start_busy(name) {
            return Err(busy_err(name, ""));
        }
        let now = rmsvc_core::clock::now_secs();
        let _ = self.set_optimize_check(name, sidecar::OptimizeCheck { status: "pending".into(), message: String::new(), at: now, progress: None });
        self.spawn_bg(name, bus, |this, name, bus| {
            let mut last_reported = 0usize;
            let on_progress = |done: usize, total: usize| {
                if done == total || done == 1 || done - last_reported >= OPTIMIZE_PROGRESS_STRIDE {
                    last_reported = done;
                    let _ = this.set_optimize_check(name, sidecar::OptimizeCheck {
                        status: "pending".into(),
                        message: String::new(),
                        at: rmsvc_core::clock::now_secs(),
                        progress: Some(sidecar::StepProgress { done: done as u32, total: total as u32 }),
                    });
                    bus.publish("books", "staging");
                }
            };
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| this.optimize(name, on_progress)))
                .unwrap_or_else(|_| Err("优化过程内部异常（已捕获，不影响其他操作）".to_string()));
            let at = rmsvc_core::clock::now_secs();
            let oc = match &result {
                Ok(msg) => sidecar::OptimizeCheck { status: "ok".into(), message: msg.clone(), at, progress: None },
                Err(e) => sidecar::OptimizeCheck { status: "failed".into(), message: e.clone(), at, progress: None },
            };
            let _ = this.set_optimize_check(name, oc);
        });
        Ok(())
    }

    /// [`Self::spawn_optimize`]/[`Self::spawn_deliver`] 共用的"起后台线程"外壳（2026-09-19 代码
    /// 质量审计：两处 `thread::spawn`+`end_busy`+`bus.publish` 逐行同构，业务内容——调
    /// `optimize`/`deliver`、写哪个 `sidecar::*Check`、`spawn_deliver` 还要另起渲染自检子线程——
    /// 本身不同，不下沉进来，留在各自的 `body` 闭包里。`body` 内部对业务调用本身的 `catch_unwind`
    /// （把 panic 转成带具体原因的 `Err` 写进 sidecar）**保留在各自闭包里、不合并**——两处 panic
    /// 提示文案不同（"优化过程内部异常"/"落库过程内部异常"），硬并到这一层反而丢信息；这里外层
    /// 再包一层 `catch_unwind` 只是兜底 `body` 自身（比如 sidecar 写入）意外 panic 时仍能
    /// `end_busy`+`publish`，不影响正常路径的行为。
    fn spawn_bg(&self, name: &str, bus: Arc<rmsvc_core::events::EventBus>, body: impl FnOnce(&Staging, &str, &Arc<rmsvc_core::events::EventBus>) + Send + 'static) {
        let (this, name) = (self.clone(), name.to_string());
        std::thread::spawn(move || {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| body(&this, &name, &bus)));
            this.end_busy(&name);
            bus.publish("books", "staging");
        });
    }

    /// 写异步优化结果到边车（书已从母版库删除 → Err，调用方只记日志/静默丢弃，不阻断别的流程）。
    pub fn set_optimize_check(&self, name: &str, oc: sidecar::OptimizeCheck) -> Result<(), String> {
        let p = self.existing(name)?;
        sidecar::update(&p, |d| d.optimize = Some(oc))
    }

    // ───────────── 落库 ─────────────

    /// 加入 xochitl：纯复制原字节（不再优化）。xochitl 只读 EPUB/PDF（CBZ 漫画不加入 xochitl，用户定）。`folder`
    /// 空＝书库根目录（2026-09-19 用户明确要求去掉"留空落进配置里的缺省文件夹"这条隐藏行为——跟
    /// KOReader 那边"留空＝根目录"的语义对齐，不再有一个不写在界面上的"默认文件夹"概念；
    /// [`crate::config::BookConfig::library_folder`] 配置项随这次改动一并删除，不再有任何地方读它）；
    /// 母版库条目投完**永远保留**（2026-09-19 用户明确要求去掉"投完自动删除"这个功能——母版是可以
    /// 反复投给两个读器对照、换设备重投的底本，不该被一次性动作悄悄清掉；要删由用户自己在列表里点
    /// 删除）。返回回执文案 + EPUB 的渲染自检计划（调用方起线程跑 `render_check::run`）。
    /// **同步、阻塞**——超限漫画按卷拆分要挨个建包+上传，真机能到分钟级；跟 [`Self::optimize`] 一样，
    /// HTTP 接口不该直接暴露这个方法，用 [`Self::spawn_deliver`] 走后台线程。
    pub fn deliver(&self, name: &str, folder: &str, mkdir: &MkdirQueue, bus: &rmsvc_core::events::EventBus) -> Result<DeliverOutcome, String> {
        let ct = bookconv::convert::direct_content_type(name)
            .ok_or("xochitl 只读 EPUB / PDF；此格式请「加入 KOReader」")?;
        let p = self.existing(name)?;
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        let folder = folder.trim();
        self.ensure_folder(folder, mkdir);
        if self.native_limit > 0 && size > self.native_limit {
            // 超限：EPUB 格式的漫画按 NCX 结构递归拆分成若干份分别投递，不再是全有全无
            // （2026-09-18 用户真机反馈驱动，见 bookconv::comic_split 与书架白皮书 §03aw）。
            // 非 EPUB、非漫画、或拆不出方案（没有可用 toc.ncx）的原样保留改动前的整本拒绝行为。
            if formats::ext_of(name) == "epub" {
                if let Some(outcome) = self.try_deliver_split(name, &p, folder, bus)? {
                    return Ok(outcome);
                }
            }
            return Err(format!(
                "《{name}》{} MB 超过 xochitl 上传上限（{} MB），会直接断连。PDF 请自行分割成多份后重新上传；非漫画或没有可用目录结构的 EPUB 无法自动分卷，请改用 KOReader 读",
                size >> 20,
                self.native_limit >> 20
            ));
        }
        // 2026-09-19 OOM 审计：这条路径以前 `std::fs::read` 整本读进 `Vec<u8>`，自检+上传各自又在
        // 内部再叠一份（`text_profile` 解压全部条目含图片、`Xochitl::upload` 内部克隆一份拼
        // multipart body），≤90MB 的书峰值能叠到 ~180-270MB。现在全程不把整本读进内存：自检走
        // `text_profile_file`（流式开文件，图片条目连解压都跳过），上传走 `upload_file`（流式发送
        // 体，见 rmsvc_core::xochitl 文档）。自检计划在上传前算好（投书时刻要早于 xochitl 给文档的
        // createdTime）；统计失败就不自检，不影响投书。
        let render = if formats::ext_of(name) == "epub" {
            bookconv::stats::text_profile_file(&p).ok().map(|prof| RenderPlan { name: name.to_string(), title: prof.title.clone(), expected: prof.expected_pages(), since_ms: rmsvc_core::clock::now_ms() })
        } else {
            None
        };
        let message = match self.xochitl.upload_file(&p, name, ct.mime(), folder)? {
            Delivery::Delivered(_) => format!("已加入 xochitl《{name}》"),
            Delivery::LikelyDelivered(_) => format!("已加入 xochitl《{name}》（设备处理较慢，稍候刷新书库）"),
        };
        let _ = self.mark_delivered(name, Reader::Native);
        Ok(DeliverOutcome { message, render })
    }

    /// 落库前确保目标文件夹真的存在（2026-09-19，用户反馈"文件夹里写了名字依然不会创建文件夹"）：
    /// 已经存在（或本来就是空串＝书库根）直接放行；不存在就往 `mkdir` 队列扔一个"建文件夹"请求，
    /// 同步等 `shelf-mkdir-agent.qmd`（MainView 注入，8 秒一次 Timer 轮询，唯一合法的建文件夹路径，
    /// 外部进程不能直接写 xochitl 书库的 `.metadata`）真的建出来再放行。等不到就超时放弃——不是
    /// 新错误，[`rmsvc_core::xochitl::Xochitl::upload`] 本来就有"文件夹名找不到就落书库根"的
    /// best-effort 兜底，改动前就是这个行为，这里只是尽量把"真建出来"这条更好的结果多等一会。
    fn ensure_folder(&self, folder: &str, mkdir: &MkdirQueue) {
        if folder.is_empty() || self.xochitl.find_folder(folder).is_some() {
            return;
        }
        if mkdir.add(folder).is_err() {
            return; // 名字不合法（目前只剩"空"这一种情况——2026-09-19 起 `/`\`\` 不再算不合法，见 mkdir.rs::add）
        }
        let lib_dir = self.xochitl.library_dir();
        rmsvc_core::fswatch::watch_until(lib_dir, render_check::DEBOUNCE, FOLDER_WAIT_TIMEOUT, |_| self.xochitl.find_folder(folder).is_some());
    }

    /// [`Self::deliver`] 的异步版：同 [`Self::spawn_optimize`] 套路——先做零耗时校验（格式/文件存在），
    /// 校验过了才加忙锁、起后台线程跑真正耗时的部分。成功返回后 HTTP 层立即回"已开始"，真正结果通过
    /// `bus` 的 `books`/`staging` 事件 + `GET /staging` 列表里这条目的 `delivered.deliver`
    /// （[`sidecar::DeliverCheck`]）异步呈现；渲染自检计划、`mark_delivered` 全部在 `deliver` 内部
    /// 完成，不劳 HTTP 层操心。`catch_unwind` 兜底同 `spawn_optimize`。
    pub fn spawn_deliver(&self, name: &str, folder: &str, mkdir: Arc<MkdirQueue>, bus: Arc<rmsvc_core::events::EventBus>) -> Result<(), String> {
        bookconv::convert::direct_content_type(name)
            .ok_or("xochitl 只读 EPUB / PDF；此格式请「加入 KOReader」")?;
        self.existing(name)?;
        if !self.try_start_busy(name) {
            return Err(busy_err(name, ""));
        }
        let now = rmsvc_core::clock::now_secs();
        let _ = self.set_deliver_check(name, sidecar::DeliverCheck { status: "pending".into(), message: String::new(), at: now, progress: None });
        let folder = folder.to_string();
        self.spawn_bg(name, bus, move |this, name, bus| {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| this.deliver(name, &folder, &mkdir, bus)))
                .unwrap_or_else(|_| Err("落库过程内部异常（已捕获，不影响其他操作）".to_string()));
            let at = rmsvc_core::clock::now_secs();
            let dc = match &result {
                // 成功/失败落定后进度条意义不大（`status` 本身就是终态），不保留最后一次的
                // `progress`——避免网页刷新时短暂显示一条"3/8"却又同时是 ok/failed 的矛盾态。
                Ok(outcome) => sidecar::DeliverCheck { status: "ok".into(), message: outcome.message.clone(), at, progress: None },
                Err(e) => sidecar::DeliverCheck { status: "failed".into(), message: e.clone(), at, progress: None },
            };
            let _ = this.set_deliver_check(name, dc);
            if let Ok(outcome) = &result {
                if let Some(plan) = outcome.render.clone() {
                    let (staging2, bus2, lib2) = (this.clone(), bus.clone(), this.xochitl.library_dir().to_path_buf());
                    std::thread::spawn(move || crate::render_check::run(&staging2, &bus2, &lib2, &plan));
                }
            }
        });
        Ok(())
    }

    /// 写异步落库结果到边车（书已从母版库删除 → Err，调用方只记日志/静默丢弃，不阻断别的流程——
    /// 比如落库还在跑的时候用户自己手动删了这本书）。
    pub fn set_deliver_check(&self, name: &str, dc: sidecar::DeliverCheck) -> Result<(), String> {
        let p = self.existing(name)?;
        sidecar::update(&p, |d| d.deliver = Some(dc))
    }

    /// 超限 EPUB 漫画的拆分投递：不是漫画 / 拆不出方案 → `Ok(None)`（调用方退回改动前的整本拒绝）；
    /// 拆出方案 → 挨个上传能塞进预算的那几份，聚合成一条回执。原书字节在母版库/KOReader 完全不受
    /// 影响——拆出来的那几份只存在于内存里，上传即弃，从不落母版库，母版库原书一份字节不动。
    /// 渲染自检对拆分出来的每一份跳过——`RenderPlan` 按母版库条目名找书，
    /// 拆分份没有母版库条目，硬接只会认错书，留作已知范围限制（见书架白皮书 §03aw）。
    /// 2026-09-19 改走流式路径 `comic_split::deliver_split_streaming`——真机 785MB《镖人（11 卷）》
    /// 坐实旧写法（`std::fs::read` 整本读 + `check::read_entries` 整本解压进 `Vec<Entry>`）把
    /// book-serve 逼近系统内存上限（`VmRSS` 观测到 1.65GB/2GB，同一天早些时候修的
    /// `optimize_epub_file_streaming` 是同一类风险，这条落库拆分路径当时没顺带改）。流式版只在
    /// 规划阶段读小文件（OPF/NCX/HTML 文本），图片体积从 zip 目录直接查表拿、不解压；逐份处理时
    /// 才把这一份需要的图片读回真实字节，传完立刻丢，峰值内存只有"一份的体积"（≤ `native_limit`）。
    ///
    /// **上传完一份等 xochitl 真的渲染完再传下一份，不猜固定等待时间**（真机《镖人》11 卷三轮
    /// 真实投递坐实：连续紧挨着上传，xochitl 那边忙着给刚收到的那卷渲染 PDF+生成封面缩略图+建
    /// `.epubindex`——`journalctl` 能看到每卷这套处理耗时 15-30 秒不等（`entryUploadTimer timed
    /// out` 反复出现），下一份的上传连接精确被同一卷打断（`Connection reset by peer`/`Broken
    /// pipe`，三轮位置一致，不是随机网络抖动）；固定 5 秒/15 秒间隔都是瞎猜、不可靠，改用整本投递
    /// 渲染自检同一套机制（`render_check::probe`+`fswatch::watch_until`）等这一份真的出现页数再
    /// 放行下一份——每份限时 [`PIECE_RENDER_TIMEOUT`]，超时也不算失败（上传本身已经成功，只是没
    /// 等到确认，继续投下一份，不为等不到的确认阻塞整本书）。
    fn try_deliver_split(&self, name: &str, p: &Path, folder: &str, bus: &rmsvc_core::events::EventBus) -> Result<Option<DeliverOutcome>, String> {
        let stem = name.strip_suffix(".epub").unwrap_or(name).to_string();
        let native_limit = self.native_limit;
        let lib_dir = self.xochitl.library_dir().to_path_buf();
        // 逐份上传/等渲染都可能耗时到分钟级（真机《镖人》11 卷坐实）——每完成一份就把进度写进
        // sidecar 的 `deliver` 字段（status 仍是 "pending"，`progress.{done,total}` 是结构化
        // 份数给网页画真百分比进度条用，`message` 仍留一句人话＋已完成的具体卷名），`GET /staging`
        // 就能看到实时进度，不用等整本投完才有任何反馈（2026-09-19 用户先反馈"能否显示优化及投书
        // 进度"，后又反馈"正在处理中请稍候"这种静态文案该换成进度条/百分比，这里是后一条的落地）。
        // **每写完一份 sidecar 进度也要 `bus.publish`**——不发事件的话，网页那套"SSE 推事件才刷新
        // 列表"的零轮询机制根本不知道这条记录变了，进度条数字冻结在第一份，要手动刷新页面才看得到
        // 新值（2026-09-19 用户反馈"进度条不会动，要自己刷新"，根因是这个函数当时没拿到 `bus`）。
        let mut done_titles: Vec<String> = Vec::new();
        let outcome = bookconv::comic_split::deliver_split_streaming(p, native_limit, |piece_name, bytes, idx, total| {
            let since_ms = rmsvc_core::clock::now_ms();
            self.xochitl.upload(bytes, piece_name, "application/epub+zip", folder).map(|_| ())?;
            let plan = RenderPlan { name: piece_name.to_string(), title: None, expected: 0, since_ms };
            if render_check::probe(&lib_dir, &plan).is_none() {
                rmsvc_core::fswatch::watch_until(&lib_dir, render_check::DEBOUNCE, PIECE_RENDER_TIMEOUT, |_| render_check::probe(&lib_dir, &plan).is_some());
            }
            done_titles.push(piece_name.to_string());
            // 份数（几完成/共几份）交给 `progress` 结构化字段，网页拿去画真百分比进度条，这里
            // `message` 只留"具体是哪几卷"——两边不重复说同一件事（份数），各自负责一半信息。
            let progress = sidecar::DeliverCheck {
                status: "pending".into(),
                message: format!("已加入：{}", done_titles.join("、")),
                at: rmsvc_core::clock::now_secs(),
                progress: Some(sidecar::StepProgress { done: idx as u32, total: total as u32 }),
            };
            let _ = self.set_deliver_check(name, progress);
            bus.publish("books", "staging");
            Ok(())
        })?;
        let Some(outcome) = outcome else { return Ok(None) }; // 不是漫画，或没超预算——退回原来的整本流程
        if outcome.delivered.is_empty() {
            return Err(format!("《{name}》按卷拆分后一份都没能投上：{}", outcome.failed.join("；")));
        }
        let mut message = format!("《{stem}》超限，已按卷拆分加入 xochitl：{}", outcome.delivered.join("、"));
        if !outcome.failed.is_empty() {
            message.push_str(&format!("（{} 未投：{}）", outcome.failed.len(), outcome.failed.join("；")));
        }
        let _ = self.mark_delivered(name, Reader::Native);
        Ok(Some(DeliverOutcome { message, render: None }))
    }

    /// 写渲染自检结果到边车（书已从母版库删除 → Err，调用方只记日志）。
    pub fn set_render(&self, name: &str, rc: RenderCheck) -> Result<(), String> {
        let p = self.existing(name)?;
        sidecar::update(&p, |d| d.render = Some(rc))
    }

    /// 记这份母版库文件是由哪个原始输入处理出来的（CLI push 上传时带 `?srcName=&srcBytes=` 才有，见
    /// `sidecar::SourceRef` 文档）。书已从母版库删除 → Err，调用方（`staging_upload`）只记日志不阻断上传结果。
    pub fn set_source(&self, name: &str, source: sidecar::SourceRef) -> Result<(), String> {
        let p = self.existing(name)?;
        sidecar::update(&p, |d| d.source = Some(source))
    }

    /// 记一次落库：写 sidecar `.<name>.delivered`。
    pub fn mark_delivered(&self, name: &str, reader: Reader) -> Result<(), String> {
        let p = self.existing(name)?;
        let now = rmsvc_core::clock::now_secs();
        sidecar::update(&p, |d| match reader {
            Reader::Native => d.native = Some(now),
            Reader::Koreader => d.koreader = Some(now),
        })
    }

    /// xochitl 的渲染缓存 `<uuid>.pdf`（`shelf doctor --render` 取回量首行缩进）。只认 uuid 形状，只读。
    pub fn render_pdf(&self, uuid: &str) -> Result<Vec<u8>, String> {
        if uuid.len() != 36 || !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Err("uuid 形状不对".into());
        }
        std::fs::read(self.xochitl.library_dir().join(format!("{uuid}.pdf"))).map_err(|_| "书库里没有这份渲染缓存（xochitl 还没渲染，或书已删）".to_string())
    }

    // ───────────── 查 / 删 ─────────────

    pub fn remove(&self, name: &str) -> Result<(), String> {
        if self.is_busy(name) {
            return Err(busy_err(name, "再删除"));
        }
        self.remove_unlocked(name)
    }

    /// 实际删除，不查忙锁——[`Self::remove`] 自己查完忙锁之后调这个真正干活；外部一律走
    /// [`Self::remove`]，不要绕过忙锁检查直接调这个。
    fn remove_unlocked(&self, name: &str) -> Result<(), String> {
        let p = self.path_of(name)?;
        sidecar::remove(&p);
        std::fs::remove_file(&p).map_err(|e| format!("删除失败: {e}"))
    }

    /// 所在分区剩余空间（字节）。`df -k` 解析：表头后的所有行拍平成 token（设备名太长时 busybox 会把数字
    /// 换到下一行，真机 `/dev/mapper/home-encrypted-disk` 就这样），第 4 个 token = Available KB；解析不了 None。
    pub fn free_bytes(&self) -> Option<u64> {
        let out = std::process::Command::new("df").arg("-k").arg(&self.dir).output().ok()?;
        let s = String::from_utf8_lossy(&out.stdout);
        let toks: Vec<&str> = s.lines().skip(1).flat_map(|l| l.split_whitespace()).collect();
        toks.get(3)?.parse::<u64>().ok().map(|kb| kb * 1024)
    }

    /// 列母版库，最新入库在前（同秒按名）。隐藏文件（sidecar / 半成品）不列。
    pub fn list(&self) -> Vec<StagingEntry> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return out };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(md) = e.metadata() else { continue };
            if name.starts_with('.') || !md.is_file() {
                continue;
            }
            let format = match formats::ext_of(&name).as_str() {
                "epub" => "epub",
                "pdf" => "pdf",
                "cbz" => "cbz",
                _ => "other",
            };
            // 优化状态只对 EPUB 有意义。标记分等级：完整（含 wash）= 版本号本身；只跑核心遍 = `<版本>-core`；旧版本号 = old。
            let level = if format != "epub" {
                "none"
            } else {
                match e.path().to_str().and_then(optimize::optimized_version_file) {
                    Some(v) if v == optimize::OPTIMIZE_VERSION => "full",
                    Some(v) if v.ends_with("-core") => "core",
                    Some(_) => "old",
                    None => "none",
                }
            };
            let mtime = md.modified().ok().map(rmsvc_core::clock::secs_of).unwrap_or(0);
            let busy = self.is_busy(&name);
            out.push(StagingEntry { name, bytes: md.len(), format, optimized: level == "full", level, mtime, delivered: sidecar::read(&e.path()), busy });
        }
        out.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.name.cmp(&b.name)));
        out
    }
}

/// 上传模板适配：母版库作为 [`AssetStore`]——扩展名门＝书籍格式白名单，install＝同分区 rename 入库。
/// 暂存目录应传 spool 的 `.work/`（与母版库同分区），见 `AssetUploadFlow::in_dir`。
pub struct StagingStore<'a>(pub &'a Staging);

impl AssetStore for StagingStore<'_> {
    fn kind(&self) -> &'static str {
        "book"
    }
    fn allowed_ext(&self) -> &'static [&'static str] {
        BOOK_EXTS
    }
    fn install(&self, name: &str, staged: &Path) -> Result<AssetItem, String> {
        let landed = self.0.stage_from_path(name, staged)?;
        let bytes = std::fs::metadata(self.0.dir.join(&landed)).map(|m| m.len()).unwrap_or(0);
        Ok(AssetItem::plain(landed, bytes))
    }
    fn list(&self) -> Vec<AssetItem> {
        self.0.list().into_iter().map(|e| AssetItem::plain(e.name, e.bytes)).collect()
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.0.remove(name)
    }
    fn reject_message(&self) -> String {
        reject_message()
    }
    fn success_message(&self, requested: &str, item: &AssetItem) -> String {
        if item.name == requested {
            "已入母版库".into()
        } else {
            format!("已入母版库（已有同名，存为 {}）", item.name)
        }
    }
}

/// 非书籍文件的拒收文案（上传门与 inbox 追平同一句）。
pub fn reject_message() -> String {
    format!("不是书籍格式，母版库只收 {}", formats::dotted(BOOK_EXTS))
}

/// 优化回执尾注：`（N 章，前→后 字节，剥伪 DRM…）`。
fn optimize_note(rep: &optimize::Report) -> String {
    let mut note = format!("（{}，{} 章，{}→{} 字节", if rep.wash.is_some() { "清洗+优化" } else { "只优化" }, rep.html_files, rep.bytes_before, rep.bytes_after);
    if let Some(w) = &rep.wash {
        if !w.pseudo_drm_stripped.is_empty() {
            note.push_str(&format!("，剥伪 DRM {} 项", w.pseudo_drm_stripped.len()));
        }
        if !w.empty_pages_removed.is_empty() {
            note.push_str(&format!("，删空页 {}", w.empty_pages_removed.len()));
        }
        if w.toc_generated > 0 {
            note.push_str(&format!("，自动目录 {} 条", w.toc_generated));
        }
        if w.toc_parts_restructured > 0 {
            note.push_str(&format!("，目录按分部重建 {} 条", w.toc_parts_restructured));
        }
        if w.ncx_uid_fixed > 0 {
            note.push_str("，修复目录标识符不匹配");
        }
        if w.ncx_doctype_stripped > 0 {
            note.push_str("，剥离目录外部DTD引用");
        }
        if w.ncx_manifest_id_fixed > 0 {
            note.push_str("，修复目录条目标识符");
        }
    }
    note.push('）');
    note
}

fn landed_name(p: &Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("book").to_string()
}


#[cfg(test)]
mod tests {
    use super::*;
    use rmsvc_core::asset::AssetUploadFlow;

    fn staging(t: &tempfile::TempDir) -> Staging {
        let x = Arc::new(Xochitl::new("127.0.0.1:1", Path::new("/nonexistent"), 1));
        let s = Staging::new(t.path().join("staging"), x, 1024 * 1024);
        s.ensure().unwrap();
        s
    }

    /// 测试用的 mkdir 队列——这些 deliver 测试全部传空 folder（`ensure_folder` 见到空串直接短路
    /// 返回，压根不会碰 mkdir），队列本身指哪个临时目录不重要，只要类型对得上。
    fn empty_mkdir(t: &tempfile::TempDir) -> MkdirQueue {
        MkdirQueue::new(&t.path().join("state"), &t.path().join("xochitl"))
    }

    fn mini_epub(files: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            zw.start_file("mimetype", o).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            for (n, d) in files {
                zw.start_file(*n, o).unwrap();
                zw.write_all(d.as_bytes()).unwrap();
            }
            zw.finish().unwrap();
        }
        buf
    }

    #[test]
    fn put_list_read_remove() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        assert_eq!(s.stage_new("a.epub", b"not-a-zip").unwrap(), "a.epub");
        assert_eq!(s.stage_new("a.epub", b"xx").unwrap(), "1_a.epub", "同名不覆盖");
        s.stage_new("doc.pdf", b"%PDF").unwrap();
        let list = s.list();
        assert_eq!(list.len(), 3);
        let epub = list.iter().find(|e| e.name == "a.epub").unwrap();
        assert_eq!((epub.format, epub.level, epub.optimized), ("epub", "none", false), "非 zip 不该判已优化");
        assert_eq!(list.iter().find(|e| e.name == "doc.pdf").unwrap().format, "pdf");
        s.remove("a.epub").unwrap();
        assert_eq!(s.list().len(), 2);
        for bad in ["../x", ".hidden", "a/b"] {
            assert!(s.stage_new(bad, b"y").is_err(), "{bad:?} 非法名拒绝");
        }
    }

    #[test]
    fn delivered_record_roundtrip_and_reader_parse() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        s.stage_new("b.epub", b"x").unwrap();
        assert!(s.list()[0].delivered.is_none(), "未落库无记录");
        s.mark_delivered("b.epub", Reader::Native).unwrap();
        s.mark_delivered("b.epub", Reader::Koreader).unwrap();
        let d = s.list()[0].delivered.clone().unwrap();
        assert!(d.native.is_some() && d.koreader.is_some());
        assert_eq!(s.list().len(), 1, "sidecar 不当条目列出");
        assert!(Reader::parse("nowhere").is_err() && Reader::parse("native").is_ok());
        assert!(s.mark_delivered("nope.epub", Reader::Native).is_err(), "不存在的书拒绝");
        s.remove("b.epub").unwrap();
        assert!(!sidecar::path_for(&s.dir().join("b.epub")).exists(), "删书连带删 sidecar");
    }

    #[test]
    fn optimize_gates_and_runs_single_full_pass() {
        // 2026-09-19 用户明确要求去掉"优化分档位"——不再有 Plain/KeepSpacing 两档，`optimize()`
        // 永远跑完整的清洗+优化（原来标"推荐"的那档，也是绝大多数书要的那档）。
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        let opf = r#"<package version="2.0"><metadata><dc:title>t</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#;
        let epub = mini_epub(&[("content.opf", opf), ("c1.xhtml", r#"<html><head></head><body><h1>第一章</h1><p style="font-size:9px;margin:1em">正文</p></body></html>"#)]);
        s.stage_new("x.epub", &epub).unwrap();
        s.stage_new("p.pdf", b"%PDF").unwrap();
        assert!(s.optimize("p.pdf", |_, _| {}).unwrap_err().contains("只有 EPUB"));
        assert!(s.optimize("none.epub", |_, _| {}).is_err());
        let mut progresses = Vec::new();
        let msg = s.optimize("x.epub", |done, total| progresses.push((done, total))).unwrap();
        assert!(msg.contains("清洗+优化") && msg.contains("自动目录 1 条"), "{msg}");
        assert!(!progresses.is_empty(), "on_progress 应该原样转发自 optimize_epub_file_streaming");
        assert_eq!(progresses.last().unwrap().0, progresses.last().unwrap().1, "最后一次回调应该是 done==total");
        let e = s.list().into_iter().find(|e| e.name == "x.epub").unwrap();
        assert!(e.optimized && e.level == "full");
    }

    #[test]
    fn optimize_temp_file_dot_prefixed_so_list_does_not_surface_mid_flight_product() {
        // 真机回归（2026-09-19，《镖人》552MB 全集）：流式优化耗时到分钟级，临时产物在目录里存在
        // 的时间不再是"同步写一次内存 buffer"那种毫秒级窗口——之前用不带点前缀的命名，真机
        // `GET /staging` 撞见过一条 `format:"other"` 的 `....epub.optimizing.tmp` 离谱条目。
        // 这里不模拟并发时序（太脆），直接断言临时产物命名遵循 `list()` 已有的点前缀过滤规则。
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        s.stage_new("x.epub", b"PK").unwrap();
        std::fs::write(s.dir().join(".x.epub.optimizing.tmp"), b"partial").unwrap();
        let names: Vec<String> = s.list().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["x.epub".to_string()], "优化中途产物不该出现在列表里: {names:?}");
    }

    #[test]
    fn busy_lock_blocks_second_start_and_conflicting_delete_deliver() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        s.stage_new("x.epub", b"PK").unwrap();
        assert!(s.try_start_busy("x.epub"), "第一次加锁应该成功");
        assert!(!s.try_start_busy("x.epub"), "已经忙着，第二次应该失败");
        assert!(s.is_busy("x.epub"));
        assert!(s.remove("x.epub").unwrap_err().contains("正在处理中"), "忙的时候不该能删");
        let bus = Arc::new(rmsvc_core::events::EventBus::new());
        assert!(s.spawn_deliver("x.epub", "", Arc::new(empty_mkdir(&t)), bus).unwrap_err().contains("正在处理中"), "忙的时候不该能起第二个落库");
        assert_eq!(s.list().iter().find(|e| e.name == "x.epub").unwrap().busy, true, "GET /staging 列表应体现 busy");
        s.end_busy("x.epub");
        assert!(!s.is_busy("x.epub"));
        assert!(s.remove("x.epub").is_ok(), "解锁后恢复正常");
    }

    #[test]
    fn spawn_deliver_runs_in_background_records_result_then_clears_busy() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t); // xochitl 指向不可达地址（见 staging() 测试 helper），deliver 必然失败——够测异步管线本身
        s.stage_new("d.pdf", &vec![b'%'; 10]).unwrap();
        let bus = Arc::new(rmsvc_core::events::EventBus::new());
        s.spawn_deliver("d.pdf", "", Arc::new(empty_mkdir(&t)), bus).unwrap();
        assert!(s.is_busy("d.pdf"), "spawn 返回时忙锁应已生效");
        let bus2 = Arc::new(rmsvc_core::events::EventBus::new());
        assert!(s.spawn_deliver("d.pdf", "", Arc::new(empty_mkdir(&t)), bus2).unwrap_err().contains("正在处理中"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while s.is_busy("d.pdf") && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!s.is_busy("d.pdf"), "后台线程应该在超时前跑完并清忙锁");
        let e = s.list().into_iter().find(|e| e.name == "d.pdf").unwrap();
        assert!(!e.busy);
        let dc = e.delivered.and_then(|d| d.deliver).expect("应该写了异步落库结果");
        assert_eq!(dc.status, "failed", "测试环境 xochitl 不可达，落库必然失败");
    }

    #[test]
    fn spawn_deliver_rejects_bad_format_synchronously_without_busy_lock() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        s.stage_new("c.cbz", b"PK").unwrap();
        let bus = Arc::new(rmsvc_core::events::EventBus::new());
        assert!(s.spawn_deliver("c.cbz", "", Arc::new(empty_mkdir(&t)), bus.clone()).unwrap_err().contains("只读 EPUB / PDF"));
        assert!(!s.is_busy("c.cbz"), "校验失败不该留下忙锁");
        assert!(s.spawn_deliver("none.epub", "", Arc::new(empty_mkdir(&t)), bus).is_err());
    }

    #[test]
    fn spawn_optimize_runs_in_background_and_records_result_then_clears_busy() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        let opf = r#"<package version="2.0"><metadata><dc:title>t</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#;
        let epub = mini_epub(&[("content.opf", opf), ("c1.xhtml", "<html><head></head><body><h1>第一章</h1><p>正文</p></body></html>")]);
        s.stage_new("x.epub", &epub).unwrap();
        let bus = Arc::new(rmsvc_core::events::EventBus::new());
        s.spawn_optimize("x.epub", bus).unwrap();
        // 起线程那一刻就该忙（同步部分：格式校验+加锁，不依赖线程调度时机）
        assert!(s.is_busy("x.epub"), "spawn 返回时忙锁应已生效");
        // 忙着的时候第二次调用应该被拒绝，不会排队/覆盖
        let bus2 = Arc::new(rmsvc_core::events::EventBus::new());
        assert!(s.spawn_optimize("x.epub", bus2).unwrap_err().contains("正在处理中"));
        // 等后台线程跑完（真实测试书毫秒级，给足超时兜底 CI 慢机器）
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while s.is_busy("x.epub") && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!s.is_busy("x.epub"), "后台线程应该在超时前跑完并清忙锁");
        let e = s.list().into_iter().find(|e| e.name == "x.epub").unwrap();
        assert!(!e.busy);
        let oc = e.delivered.and_then(|d| d.optimize).expect("应该写了异步优化结果");
        assert_eq!(oc.status, "ok");
        assert!(oc.message.contains("已优化"), "{}", oc.message);
    }

    #[test]
    fn spawn_optimize_rejects_non_epub_and_missing_file_synchronously() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        s.stage_new("p.pdf", b"%PDF").unwrap();
        let bus = Arc::new(rmsvc_core::events::EventBus::new());
        assert!(s.spawn_optimize("p.pdf", bus.clone()).unwrap_err().contains("只有 EPUB"));
        assert!(!s.is_busy("p.pdf"), "校验失败不该留下忙锁");
        assert!(s.spawn_optimize("none.epub", bus).is_err());
    }

    /// 造一本 2 卷合集漫画（跟 bookconv::comic_split 测试用例同一套结构），塞进 mini_epub 装不了的
    /// 二进制字节所以这里直接手搓 zip——is_comic/comic_split 只看扩展名和 NCX 结构，不校验图片
    /// 内容本身是不是合法 JPEG，够测这条集成路径。
    fn multivol_comic_epub(pages_per_vol: &[usize]) -> Vec<u8> {
        use std::io::Write;
        let mut buf = Vec::new();
        let mut manifest = String::new();
        let mut spine = String::new();
        let mut navpoints = String::new();
        let mut page_no = 0usize;
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        for (vi, &n) in pages_per_vol.iter().enumerate() {
            let vol_start = page_no;
            for _ in 0..n {
                files.push((format!("text/p{page_no:04}.html"), format!(r#"<html><body><img src="../images/{page_no:04}.jpg"/></body></html>"#).into_bytes()));
                files.push((format!("images/{page_no:04}.jpg"), vec![7u8; 200]));
                manifest += &format!(r#"<item id="h{page_no}" href="text/p{page_no:04}.html" media-type="application/xhtml+xml"/><item id="i{page_no}" href="images/{page_no:04}.jpg" media-type="image/jpeg"/>"#);
                spine += &format!(r#"<itemref idref="h{page_no}"/>"#);
                page_no += 1;
            }
            navpoints += &format!(r#"<navPoint id="nv{vi}"><navLabel><text>卷{vi}</text></navLabel><content src="text/p{vol_start:04}.html"/></navPoint>"#);
        }
        files.push(("content.opf".into(), format!(r#"<package version="3.0"><metadata><dc:title>t</dc:title></metadata><manifest>{manifest}<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx">{spine}</spine></package>"#).into_bytes()));
        files.push(("toc.ncx".into(), format!(r#"<ncx><navMap>{navpoints}</navMap></ncx>"#).into_bytes()));
        files.push(("META-INF/container.xml".into(), br#"<container><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#.to_vec()));
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        zw.start_file("mimetype", o).unwrap();
        zw.write_all(b"application/epub+zip").unwrap();
        for (n, d) in files {
            zw.start_file(n, o).unwrap();
            zw.write_all(&d).unwrap();
        }
        zw.finish().unwrap();
        buf
    }

    #[test]
    fn deliver_oversized_epub_comic_attempts_split_instead_of_flat_reject() {
        // 25+ 张图满足 is_comic 阈值（2 卷各 15 张，每页 html+img 约 200B）；native_limit 给 1KB
        // 逼近强制超限，触发拆分路径。测试环境 Xochitl 指向不可达地址，upload() 必然失败，验证不了
        // 真正投递成功——那部分已经在真机+bookconv 单测（`comic_split::tests::against_real_naruto_book`）
        // 分别验证过；这里只验证 deliver() 确实走了"按卷拆分尝试"这条新路径，不是笼统整本拒绝。
        let t = tempfile::tempdir().unwrap();
        let x = Arc::new(Xochitl::new("127.0.0.1:1", Path::new("/nonexistent"), 1));
        let s = Staging::new(t.path().join("staging"), x, 1024);
        s.ensure().unwrap();
        let epub = multivol_comic_epub(&[15, 15]);
        s.stage_new("manga.epub", &epub).unwrap();
        let err = s.deliver("manga.epub", "", &empty_mkdir(&t), &rmsvc_core::events::EventBus::new()).unwrap_err();
        assert!(err.contains("按卷拆分") || err.contains("上传失败") || err.contains("拆分后一份都没能投上"), "应该走拆分路径而不是整本拒绝: {err}");
        assert!(err.contains("卷0") || err.contains("卷1"), "错误信息应该点出具体是哪一卷: {err}");
    }

    #[test]
    fn deliver_oversized_non_comic_epub_keeps_flat_reject() {
        let t = tempfile::tempdir().unwrap();
        let opf = r#"<package version="2.0"><metadata><dc:title>t</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#;
        let long_text = "正".repeat(600_000); // 600_000 * 3 字节(UTF-8) ≈ 1.7MB，确保超过测试用 1MB native_limit
        let epub = mini_epub(&[("content.opf", opf), ("c1.xhtml", &format!("<html><body><p>{long_text}</p></body></html>"))]);
        let s = staging(&t);
        s.stage_new("novel.epub", &epub).unwrap();
        let err = s.deliver("novel.epub", "", &empty_mkdir(&t), &rmsvc_core::events::EventBus::new()).unwrap_err();
        assert!(err.contains("超过 xochitl 上传上限") && err.contains("分卷"), "非漫画超限应该保持改动前的整本拒绝: {err}");
    }

    #[test]
    fn deliver_gates_format_before_touching_xochitl() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        s.stage_new("c.cbz", b"PK").unwrap();
        s.stage_new("d.pdf", b"%PDF").unwrap();
        assert_eq!(s.list().iter().find(|e| e.name == "c.cbz").unwrap().format, "cbz");
        assert!(s.deliver("c.cbz", "", &empty_mkdir(&t), &rmsvc_core::events::EventBus::new()).unwrap_err().contains("只读 EPUB / PDF"));
        // 体积门：超过 native_limit（测试设 1MB）不碰 xochitl，回执指引分卷
        s.stage_new("huge.pdf", &vec![b'%'; 2 * 1024 * 1024]).unwrap();
        let e = s.deliver("huge.pdf", "", &empty_mkdir(&t), &rmsvc_core::events::EventBus::new()).unwrap_err();
        assert!(e.contains("超过 xochitl 上传上限") && e.contains("分卷"), "{e}");
        // PDF 走到 xochitl 才失败（不可达），母版仍在、无落库记录
        assert!(s.deliver("d.pdf", "", &empty_mkdir(&t), &rmsvc_core::events::EventBus::new()).is_err());
        assert!(s.list().iter().any(|e| e.name == "d.pdf" && e.delivered.is_none()));
    }

    #[test]
    fn deliver_ensures_folder_enqueues_and_waits_for_agent_to_create_it() {
        // 2026-09-19 用户反馈"文件夹里写了名字依然不会创建文件夹"：deliver() 落库前要先经
        // ensure_folder 确认目标文件夹真实存在，不存在就入队等 shelf-mkdir-agent.qmd 建出来。
        // 这里模拟"代理真的建出来了"（另起一个线程，短延迟后往 lib_dir 写一份 CollectionType
        // .metadata，等价于 Library.createCollection 真机执行后落盘的结果），验证 ensure_folder
        // 真的会在代理建好之后很快继续（而不是傻等满 20s 超时）。
        let t = tempfile::tempdir().unwrap();
        let lib_dir = t.path().join("xochitl");
        std::fs::create_dir_all(&lib_dir).unwrap();
        let x = Arc::new(Xochitl::new("127.0.0.1:1", &lib_dir, 1)); // 端口 1 必然连不上，只测 ensure_folder 本身
        let s = Staging::new(t.path().join("staging"), x, 1024 * 1024);
        s.ensure().unwrap();
        s.stage_new("x.epub", b"PK").unwrap();
        let mkdir = MkdirQueue::new(&t.path().join("state"), &lib_dir);
        assert!(mkdir.list().is_empty());

        let lib_dir2 = lib_dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(300));
            std::fs::write(lib_dir2.join("f1.metadata"), r#"{"type":"CollectionType","visibleName":"新文件夹","parent":""}"#).unwrap();
        });

        let started = std::time::Instant::now();
        let _ = s.deliver("x.epub", "新文件夹", &mkdir, &rmsvc_core::events::EventBus::new());
        // 20s 超时是"等不到才放弃"的兜底上限；代理已经在 300ms+3s 防抖内把文件夹建出来了，
        // ensure_folder 应该在远小于超时的时间内就继续往下走（这里用 15s 卡一个宽松上限，
        // 只为区分"真的检测到了"和"傻等满超时"两种情况，不是卡精确耗时）。
        assert!(started.elapsed() < std::time::Duration::from_secs(15), "elapsed={:?}，看起来是等满了超时而不是检测到文件夹已建出来", started.elapsed());
        assert_eq!(mkdir.list().len(), 1, "ensure_folder 应该把这个文件夹名入队过");
        assert_eq!(mkdir.list()[0].name, "新文件夹");
    }

    #[test]
    fn upload_flow_lands_books_and_rejects_non_books() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        let work = t.path().join(".work");
        let mut body = Vec::new();
        // 2026-09-18 起母版库只收 EPUB/PDF（cbz 已随"仅 KOReader"档退役）：接受项/重名项都改用
        // .epub 夹具，pic.jpg 仍测"非书籍格式拒收"，e.pdf 空内容仍测"空文件拒收"（合法格式但空）。
        for (f, d) in [("../中文 名.epub", "内容"), ("pic.jpg", "x"), ("e.pdf", ""), ("中文 名.epub", "again")] {
            body.extend_from_slice(format!("--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{f}\"\r\n\r\n{d}\r\n").as_bytes());
        }
        body.extend_from_slice(b"--B--\r\n");
        let out = AssetUploadFlow::in_dir(work.clone()).run(&StagingStore(&s), &body[..], "B").unwrap();
        assert!(out[0].ok && out[0].name == "中文 名.epub" && out[0].message == "已入母版库");
        assert!(!out[1].ok && out[1].message.contains("不是书籍格式") && out[1].message.contains(".epub"));
        assert!(!out[2].ok && out[2].message == "空文件");
        assert!(out[3].ok && out[3].name == "1_中文 名.epub" && out[3].message.contains("存为 1_中文 名.epub"));
        assert_eq!(std::fs::read(s.dir().join("中文 名.epub")).unwrap(), "内容".as_bytes());
        assert!(std::fs::read_dir(&work).unwrap().next().is_none(), "暂存 .work 应清空");
    }
}
