//! 母版库（中间层暂存池）领域模块——三层架构（内容源 → **母版库** → 读器）的交汇点。
//! 三个正交动作各一个方法：**入库**（`stage_new` / `stage_from_path` / [`StagingStore`] 上传模板 / `fetch_article`）、
//! **优化**（`optimize`，只对 EPUB）、**落库**（`deliver` 投 xochitl；KOReader 由 koreader-serve 从同一目录 adopt，
//! 之后前端调 `mark_delivered` 记一笔）。落库＝纯复制母版字节（两读器同字节可对照），母版默认保留可反复落库。
//! 目录 `$XDG_STATE_HOME/shelf/books/staging/`（/home 分区，重启/OTA 不丢；**不套 LRU 淘汰**，留住用户还没落库的书）。
//! 落库记录是同目录隐藏 sidecar `.<name>.delivered`（`sidecar` 模块管读写；本模块只在落库/删书时调它）。
use bookconv::optimize::{self, FootnoteMode, OptimizeOpts};
use crate::mkdir::MkdirQueue;
use crate::ops::OpRegistry;
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

// 按职责拆成子模块（原 `staging.rs` 一个文件 1800+ 行）：`Staging` 的方法按动作分散在各子模块的 `impl Staging` 里，
// 对外路径（`crate::staging::Staging` 等）不变；子模块内的私有项以 `pub(super)` 提供给兄弟模块与测试。
mod deliver;
mod direction;
mod intake;
mod library;
mod optimizing;

use self::library::ProbeCache;

#[cfg(test)]
mod tests;


/// 忙锁占用时的统一提示——优化/落库/删除三处几乎逐字重复过（2026-09-19 代码质量审计）。`extra`
/// 是各自独有的后缀（删除那处要额外提示"再删除"），其余传空串。
fn busy_err(name: &str, extra: &str) -> String {
    format!("《{name}》正在处理中，请稍候{extra}")
}

/// 异步操作（优化 / 投递）结果 → 边车终态 `(status, message)`：成功 `ok`、用户取消 `cancelled`、其余 `failed`。
/// 优化与投递两处原来各写一遍同样的三分支 match（2026-09-24 审计合并）。
fn final_status(result: Result<&str, &str>) -> (String, String) {
    match result {
        Ok(msg) => ("ok".into(), msg.to_string()),
        Err(e) if e.contains(optimize::CANCELLED_MSG) => ("cancelled".into(), e.to_string()),
        Err(e) => ("failed".into(), e.to_string()),
    }
}

/// 母版库一本书的展示条目。`format`（epub / pdf / cbz / other）从扩展名判、优化等级从内埋标记判（轻量只读中央目录）。
/// `rename_all = "camelCase"`：既有字段全是单词、camelCase 变换不影响它们的 JSON key，这次
/// 新增的 `pdf_source` 借这个转成前端习惯的 `pdfSource`，不用单独给这一个字段挂 `rename`。
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
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
    /// 这份 EPUB 是不是入库 PDF 转出来的（`format=="epub"` 才有意义；跟 `optimized`/`level` 的
    /// 常规 full/core/old/none 阶梯正交——PDF 转出来是一次性产物，视为已经完成，不再进那条
    /// 阶梯，也不再显示「优化」按钮，见 `looks_like_pdf_derived_epub` 文档）。
    #[serde(default)]
    pub pdf_source: bool,
    /// 按书设置的翻页方向（`auto`/`rtl`/`ltr`，边车 `direction`，2026-09-25）；只对 EPUB 有意义，其它格式恒 `auto`。
    pub direction: &'static str,
    /// 设置的方向与母版文件 OPF 里实际写的不一致＝要再点一次「优化」才生效（此时 `optimized` 也报 `false`，
    /// 网页的「优化」按钮、网关批量队列的资格判断都靠它，不必各自再懂方向）。`auto` 恒为 `false`。
    pub direction_stale: bool,
}

/// 原 PDF 备份一条（`GET /staging` 的 `originals`）。
#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct OriginalEntry {
    pub name: String,
    pub bytes: u64,
    pub backed_up_at: u64,
    pub expires_at: u64,
}

/// 投原生成功后交给自检线程的计划：投书时刻（毫秒，圈"之后进库"的候选）+ 书名 + 期望页数。
#[derive(Clone, Debug, PartialEq)]
pub struct RenderPlan {
    pub name: String,
    pub title: Option<String>,
    pub expected: u64,
    pub since_ms: u64,
    /// 新版管线处理过的漫画：导入完成后登记"首次打开时设页边距"（见 `comic_margins.rs`）。
    pub comic: bool,
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
    /// 正在跑异步操作（「优化」/「落库」）的登记簿：忙锁 + 取消协作，见 [`crate::ops`]。
    ops: OpRegistry,
    /// 列表里"优化等级 / 是否 PDF 转来"的判定缓存：判定要开 zip 读中央目录，书多时前端每 3 秒轮询一次
    /// 列表会持续吃 CPU（电池）。文件内容只随「优化」改写——按（大小, 修改时间）失效，命中就不再碰文件。
    probes: Arc<std::sync::Mutex<std::collections::HashMap<String, ProbeCache>>>,
    /// 漫画页边距待办（可选：测试里不装）。见 [`crate::comic_margins`]。
    comic_margins: Option<Arc<crate::comic_margins::ComicMargins>>,
    /// 母版库"落名"临界区：挑一个不撞名的文件名（`unique_path` 先查存在）再 rename/写入，两步之间不能插进别的落名，
    /// 否则两个同名书会挑到同一个名字、后到的把先到的覆盖掉。网页上传 / inbox 追平 / 抓网文 / 改名 / 恢复原 PDF
    /// 都从这里过。只包"挑名 + 落地"这一小段本地文件操作——此前网页上传是把 spool 锁一直攥到整个 multipart
    /// 请求体收完（WiFi 上传大书能到分钟级），期间别的上传和 inbox 追平全被卡住（2026-09-24 审计）。
    land: Arc<std::sync::Mutex<()>>,
    /// 阅读方向手动清单（可选：测试里不装）。按书设了方向、且知道落库 uuid 时同步写进/移出，见 [`Self::set_direction`]。
    reading_direction: Option<Arc<crate::reading_direction::ReadingDirection>>,
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
            // 落地名与请求名不同有两种原因：EPUB 按 `书名 - N卷` 规范命名，或母版库里已有同名（加数字前缀）。
            // 规范命名是常态，不该说成"已有同名"；只有落地名不是规范名的改动才是撞名。
            if item.name == canonical_staged_name(requested) {
                format!("已入母版库（按规范命名存为 {}）", item.name)
            } else {
                format!("已入母版库（已有同名，存为 {}）", item.name)
            }
        }
    }
}

/// 非书籍文件的拒收文案（上传门与 inbox 追平同一句）。
/// PDF→EPUB 成功后原 PDF 的隐藏备份目录（母版库下，点前缀 → `list()` 看不见）。
pub const PDF_ORIGINALS_DIR: &str = ".pdf-originals";
/// 备份保留时长：7 天。启动时和每次新备份时清过期的。
pub const PDF_ORIGINALS_KEEP_SECS: u64 = 7 * 86_400;

pub fn reject_message() -> String {
    format!("不是书籍格式，母版库只收 {}", formats::dotted(BOOK_EXTS))
}

fn canonical_staged_name(name: &str) -> String {
    if formats::ext_of(name) == "epub" {
        bookconv::naming::canonical_file_name(name)
    } else {
        name.to_string()
    }
}

impl Staging {
    pub fn new(dir: PathBuf, xochitl: Arc<Xochitl>, native_limit: u64) -> Staging {
        Staging {
            dir,
            xochitl,
            native_limit,
            ops: OpRegistry::default(),
            probes: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            comic_margins: None,
            land: Arc::new(std::sync::Mutex::new(())),
            reading_direction: None,
        }
    }
    /// 接上阅读方向手动清单（`State::new` 用）。
    pub fn with_reading_direction(mut self, rd: Arc<crate::reading_direction::ReadingDirection>) -> Staging {
        self.reading_direction = Some(rd);
        self
    }
    /// 接上漫画页边距待办队列（`State::new` 用）。
    pub fn with_comic_margins(mut self, q: Arc<crate::comic_margins::ComicMargins>) -> Staging {
        self.comic_margins = Some(q);
        self
    }
    /// 「实验室→漫画页边距」开关是否打开（没接队列 = 关）。
    pub(crate) fn comic_margin_switch_on(&self) -> bool {
        self.comic_margins.as_ref().is_some_and(|q| q.enabled())
    }
    /// 优化时纯图漫画页补白到哪种页框：开关开 → 页边距最小的页框，关 → 屏幕比例（改动前的行为）。
    pub(crate) fn comic_frame(&self) -> bookconv::imgopt::EpubComicFrame {
        if self.comic_margin_switch_on() {
            bookconv::imgopt::EpubComicFrame::MinMargin
        } else {
            bookconv::imgopt::EpubComicFrame::Screen
        }
    }
    /// 这本 EPUB 是否该在原生书库里设成漫画页边距：**开关开 + 漫画（以图为主，允许有文字页）+ 已用当前版本管线优化过 +
    /// 文字页都已补留边 + 页框是最小边距页框**。补白比例按最小边距算，旧页框产物（含开关关着时优化的）在最小边距下会贴左、
    /// 右侧空一大块；文字页没留边的旧产物（此前只放行"整本零文字"的漫画）在边距 1 下文字会贴屏幕边——都反而更糟
    /// （见 `imgopt::EPUB_FRAME_ASPECT`、`bookconv::comic_pad`）；文字书 / PDF 不是漫画，完全不碰。
    pub(crate) fn comic_margin_eligible(&self, path: &Path) -> bool {
        self.comic_margin_switch_on()
            && path.to_str().and_then(optimize::optimized_version_file).as_deref() == Some(optimize::OPTIMIZE_VERSION)
            && bookconv::comic_detect::is_min_margin_comic_file(path)
            && bookconv::comic_detect::is_min_margin_framed_file(path)
    }
    /// 登记"这本书首次打开时设页边距"。失败只记日志，不影响投书。
    pub(crate) fn register_comic_margins(&self, uuid: &str, name: &str) {
        let Some(q) = &self.comic_margins else { return };
        match q.add(uuid, bookconv::imgopt::EPUB_COMIC_MARGINS) {
            Ok(_) => println!("[book-serve] 《{name}》是新版管线的漫画，已登记首次打开时设页边距 {}", bookconv::imgopt::EPUB_COMIC_MARGINS),
            Err(e) => println!("[book-serve] 《{name}》登记页边距失败（不影响投书）: {e}"),
        }
    }
    /// 这条目当前是否有异步操作在跑。
    pub fn is_busy(&self, name: &str) -> bool {
        self.ops.is_busy(name)
    }
    /// 尝试给条目加忙锁；已经忙着 → false（调用方据此拒绝这次操作，不排队不覆盖）。
    pub(crate) fn try_start_busy(&self, name: &str) -> bool {
        self.ops.try_start(name)
    }
    pub(crate) fn end_busy(&self, name: &str) {
        self.ops.end(name);
    }
    /// 当前这步操作声明"我会检查取消标记"。
    pub(crate) fn mark_cancellable(&self, name: &str) {
        self.ops.mark_cancellable(name);
    }
    pub(crate) fn is_cancelled(&self, name: &str) -> bool {
        self.ops.is_cancelled(name)
    }
    /// 请求取消这本书正在跑的优化/投递。`Ok(true)`＝已登记，会在下一个检查点停下；`Ok(false)`＝这一步无法中途停止
    /// （如单文件上传）；`Err`＝这本书当前没有在处理。
    pub fn request_cancel(&self, name: &str) -> Result<bool, String> {
        self.ops.request_cancel(name)
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }

    /// 进入"落名"临界区（见 `land` 字段）。
    pub(super) fn land_guard(&self) -> std::sync::MutexGuard<'_, ()> {
        rmsvc_core::sync::lock(&self.land)
    }

    /// 母版库里某本书的路径（校验单段文件名）。
    fn path_of(&self, name: &str) -> Result<PathBuf, String> {
        Ok(self.dir.join(plain_name(name)?))
    }
    /// 母版库里是否还有这本书。
    pub fn has(&self, name: &str) -> bool {
        self.existing(name).is_ok()
    }
    fn existing(&self, name: &str) -> Result<PathBuf, String> {
        let p = self.path_of(name)?;
        if !p.is_file() {
            return Err("母版库里没有这本书".into());
        }
        Ok(p)
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
}
