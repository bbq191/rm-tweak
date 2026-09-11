//! 落库：加入 xochitl（整本 / 大文件通道 / 超限分卷）、渲染记录与落库记录、建文件夹等待。
use super::*;

/// 漫画按卷拆分投递，单份等渲染确认的上限（真机观测单卷渲染+缩略图+建索引耗时 15-30 秒，给足
/// 余量；超时不算失败，只是没等到确认就接着投下一份，见 `Staging::try_deliver_split`）。
pub(super) const PIECE_RENDER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(90);

/// 落库前等待「建文件夹」代理真的建出来目标文件夹的上限——`shelf-mkdir-agent.qmd` 现为长轮询
/// （入队即刻响应，旧版是 8 秒一次 Timer 轮询），20 秒足够留出建夹 + 落盘的余量；等不到不算错误，`ensure_folder` 会原样放行，交给
/// `Xochitl::upload` 现有的"找不到就落书库根"兜底（改动前就有的行为，不是新错误）。
pub(super) const FOLDER_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// EPUB 入库/优化统一按 `书名 - 卷/部/上/下`（数字在前）命名，见 `bookconv::naming`；其它格式原名不动。
/// 大文件通道的安全上限（1GiB）：再大 xochitl 首次渲染的内存/时间没有验证过。
pub(super) const MAX_DIRECT_BYTES: u64 = 1 << 30;

impl Staging {
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
            //
            // **优先走大文件通道**（2026-09-20 用户要求突破上传限制，真机验证 PDF 154MB/EPUB 153MB 可行）：
            // 占位文档 + 磁盘上替换成真文件，不分卷、不限漫画。只有本机没有 xochitl 书库目录（非设备环境）
            // 或造占位失败才退回下面的分卷/拒绝。
            if let Some(outcome) = self.try_deliver_direct(name, &p, size, folder)? {
                return Ok(outcome);
            }
            if formats::ext_of(name) == "epub" {
                if let Some(outcome) = self.try_deliver_split(name, &p, folder, bus)? {
                    return Ok(outcome);
                }
            } else if formats::ext_of(name) == "pdf" {
                if let Some(outcome) = self.try_deliver_split_pdf(name, &p, folder, bus)? {
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
            let comic = self.comic_margin_eligible(&p);
            bookconv::stats::text_profile_file(&p).ok().map(|prof| RenderPlan { name: name.to_string(), title: prof.title.clone(), expected: prof.expected_pages(), since_ms: rmsvc_core::clock::now_ms(), comic })
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

    /// 给"已加入 xochitl 但没有渲染记录"的书补记（2026-09-20：大文件通道上线前直接投入的书没有渲染徽章，列表里不统一）。
    /// 按书名（规范名或文件名 stem）+ 文件大小在 xochitl 书库里认领对应文档；没渲染缓存（`.pdf`）＝没打开过 → `onopen`（记当前
    /// 占位页数，之后 `list()` 看到页数变了就升级）；有缓存＝已渲染过 → 直接 `ok` 记真页数。返回补记了几本。幂等、只补缺的。
    pub fn backfill_render_records(&self) -> usize {
        let lib = self.xochitl.library_dir().to_path_buf();
        if !lib.is_dir() {
            return 0;
        }
        let docs = rmsvc_core::xochitl::find_documents_since(&lib, 0);
        let mut n = 0;
        for e in self.list() {
            let has_native = e.delivered.as_ref().map(|d| d.native.is_some() && d.render.is_none()).unwrap_or(false);
            let ext = formats::ext_of(&e.name);
            if !has_native || (ext != "epub" && ext != "pdf") {
                continue;
            }
            let stem = e.name.strip_suffix(&format!(".{ext}")).unwrap_or(&e.name).to_string();
            let canon = bookconv::naming::canonical_book_name(&stem);
            let eq = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
            let doc = docs.iter().find(|d| {
                (eq(&d.visible_name, &canon) || eq(&d.visible_name, &stem) || eq(&d.visible_name, &e.name))
                    && std::fs::metadata(lib.join(format!("{}.{ext}", d.uuid))).map(|m| m.len() == e.bytes).unwrap_or(false)
            });
            let Some(doc) = doc else { continue };
            let pages = rmsvc_core::xochitl::page_count(&lib, &doc.uuid).unwrap_or(0);
            let opened = ext == "pdf" || lib.join(format!("{}.pdf", doc.uuid)).exists();
            let rc = sidecar::RenderCheck { uuid: doc.uuid.clone(), pages, expected: 0, status: if opened { "ok".into() } else { "onopen".into() }, at: rmsvc_core::clock::now_secs() };
            if self.set_render(&e.name, rc).is_ok() {
                n += 1;
            }
        }
        n
    }

    /// 大文件通道（见 [`rmsvc_core::xochitl::Xochitl::upload_large_file`]）：成功 `Ok(Some)`；条件不满足（非
    /// EPUB/PDF、超过安全上限、本机没有 xochitl 书库目录、造占位失败）→ `Ok(None)` 让调用方退回旧路径；
    /// 占位已上传之后才出的错 → `Err`（不再退回分卷，否则会在书库里留下重复内容）。
    pub(super) fn try_deliver_direct(&self, name: &str, p: &Path, size: u64, folder: &str) -> Result<Option<DeliverOutcome>, String> {
        let ext = formats::ext_of(name);
        if (ext != "epub" && ext != "pdf") || size > MAX_DIRECT_BYTES || !self.xochitl.library_dir().is_dir() {
            return Ok(None);
        }
        let stem = name.strip_suffix(&format!(".{ext}")).unwrap_or(name);
        let (placeholder, content_type, pages) = if ext == "epub" {
            // 显示名：有卷标记用规范名（与文件名一致），否则沿用书自己的 dc:title。
            let title = bookconv::naming::has_volume_marker(stem).then(|| bookconv::naming::canonical_book_name(stem));
            match bookconv::placeholder::epub_placeholder(p, title.as_deref()) {
                Ok(b) => (b, "application/epub+zip", None),
                Err(_) => return Ok(None),
            }
        } else {
            let pages = match bookconv::convert::pdfwrite::PdfFileReader::open(p).and_then(|mut r| r.page_count()) {
                Ok(n) => n,
                Err(_) => return Ok(None),
            };
            match bookconv::placeholder::pdf_placeholder() {
                Ok(b) => (b, "application/pdf", Some(pages)),
                Err(_) => return Ok(None),
            }
        };
        let uuid = self.xochitl.upload_large_file(p, name, content_type, folder, &placeholder, pages)?;
        if ext == "epub" && self.comic_margin_eligible(p) {
            self.register_comic_margins(&uuid, name);
        }
        let _ = self.mark_delivered(name, Reader::Native);
        // 渲染记录也写上，让"加入 xochitl"的书在列表里都有统一的渲染徽章（此前直接投入的书没有）：
        // - PDF：页数就是我们写进 `.content` 的真页数 → 直接 ok；
        // - EPUB：xochitl 要**首次打开**才渲染，此刻 `.content` 里是占位的页数。记 `onopen` + 占位页数，`list()` 之后每次
        //   读该文档 `.content` 的 pageCount，一变（用户打开过、xochitl 渲染完）就自动显示成真页数。
        let rc = match pages {
            Some(n) => sidecar::RenderCheck { uuid: uuid.clone(), pages: n as u64, expected: 0, status: "ok".into(), at: rmsvc_core::clock::now_secs() },
            None => sidecar::RenderCheck {
                uuid: uuid.clone(),
                pages: rmsvc_core::xochitl::page_count(self.xochitl.library_dir(), &uuid).unwrap_or(0),
                expected: 0,
                status: "onopen".into(),
                at: rmsvc_core::clock::now_secs(),
            },
        };
        let _ = self.set_render(name, rc);
        Ok(Some(DeliverOutcome {
            message: format!("《{stem}》{} MB 超过网页上传上限，已直接写入 xochitl 书库（未分卷）；首次打开需重新渲染，请稍候", size >> 20),
            render: None,
        }))
    }

    /// 落库前确保目标文件夹真的存在（2026-09-19，用户反馈"文件夹里写了名字依然不会创建文件夹"）：
    /// 已经存在（或本来就是空串＝书库根）直接放行；不存在就往 `mkdir` 队列扔一个"建文件夹"请求，
    /// 同步等 `shelf-mkdir-agent.qmd`（MainView 注入，长轮询 `/mkdir/pending?wait=`，唯一合法的建文件夹路径，
    /// 外部进程不能直接写 xochitl 书库的 `.metadata`）真的建出来再放行。等不到就超时放弃——不是
    /// 新错误，[`rmsvc_core::xochitl::Xochitl::upload`] 本来就有"文件夹名找不到就落书库根"的
    /// best-effort 兜底，改动前就是这个行为，这里只是尽量把"真建出来"这条更好的结果多等一会。
    pub(super) fn ensure_folder(&self, folder: &str, mkdir: &MkdirQueue) {
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
            // 成功/失败落定后进度条意义不大（`status` 本身就是终态），不保留最后一次的
            // `progress`——避免网页刷新时短暂显示一条"3/8"却又同时是 ok/failed 的矛盾态。
            let (status, message) = final_status(result.as_ref().map(|o| o.message.as_str()).map_err(String::as_str));
            let _ = this.set_deliver_check(name, sidecar::DeliverCheck { status, message, at, progress: None });
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
    pub(super) fn try_deliver_split(&self, name: &str, p: &Path, folder: &str, bus: &rmsvc_core::events::EventBus) -> Result<Option<DeliverOutcome>, String> {
        let stem = name.strip_suffix(".epub").unwrap_or(name).to_string();
        let native_limit = self.native_limit;
        self.deliver_pieces(name, &stem, "application/epub+zip", folder, bus, |upload| bookconv::comic_split::deliver_split_streaming(p, native_limit, upload))
    }

    /// 超预算漫画 PDF 的拆分投递——[`Self::try_deliver_split`] 的 PDF 版本。不是"我们自己产出的
    /// 漫画 PDF"（没有书签目录，比如用户自己上传的原生大部头 PDF）/ 整本已在预算内 →
    /// `Ok(None)`，调用方退回改动前的整本拒绝。
    ///
    /// 2026-09-19 改走流式 `comic_pdf::deliver_split_pdf_streaming`——真机 245MB/600页 样本坐实
    /// 过前身版本（一次性 `extract_pages` 把全书图片攒成 `Vec<PdfImage>`）`VmHWM` 峰值到过
    /// 525MB；现在逐份读逐份传逐份丢，峰值只有"一份的体积"，跟 EPUB 那条 [`Self::try_deliver_
    /// split`] 是同一套纪律。单页体积本身超预算这种边界情况这里没有单独处理——上游 `imgopt::
    /// downscale_for_epub_comic` 已经把每张图钳制在 954×1696 像素以内，JPEG 质量 95 下单页实际
    /// 不可能逼近 90MB 量级的预算，这个假设不成立时（比如以后画质/尺寸上限调高很多）需要回来
    /// 重新评估。
    pub(super) fn try_deliver_split_pdf(&self, name: &str, p: &Path, folder: &str, bus: &rmsvc_core::events::EventBus) -> Result<Option<DeliverOutcome>, String> {
        let stem = name.strip_suffix(".pdf").unwrap_or(name).to_string();
        let native_limit = self.native_limit;
        self.deliver_pieces(name, &stem, "application/pdf", folder, bus, |upload| bookconv::comic_pdf::deliver_split_pdf_streaming(p, native_limit, upload))
    }

    /// EPUB / PDF 两条拆分投递（[`Self::try_deliver_split`] / [`Self::try_deliver_split_pdf`]）共用的"逐份上传"外壳
    /// （2026-09-20 代码质量审计：两处约 40 行逐字重复，只差"谁来切"和 mime）。`split` 拿到一个"上传一份"的回调，
    /// 调 `bookconv` 里对应的流式拆分函数把它传进去；这里负责取消检查 → 上传 → 等 xochitl 渲染确认 → 写进度 → 汇总回执。
    /// 返回 `Ok(None)` ＝不适用拆分（调用方退回整本拒绝）。
    pub(super) fn deliver_pieces(
        &self,
        name: &str,
        stem: &str,
        mime: &str,
        folder: &str,
        bus: &rmsvc_core::events::EventBus,
        split: impl FnOnce(&mut dyn FnMut(&str, &[u8], usize, usize) -> Result<(), String>) -> Result<Option<bookconv::comic_split::StreamSplitOutcome>, String>,
    ) -> Result<Option<DeliverOutcome>, String> {
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
        self.mark_cancellable(name);
        let outcome = split(&mut |piece_name, bytes, idx, total| {
            if self.is_cancelled(name) {
                return Err(optimize::CANCELLED_MSG.to_string()); // 份与份之间是安全的中断点
            }
            let since_ms = rmsvc_core::clock::now_ms();
            self.xochitl.upload(bytes, piece_name, mime, folder).map(|_| ())?;
            let plan = RenderPlan { name: piece_name.to_string(), title: None, expected: 0, since_ms, comic: false };
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
        let Some(outcome) = outcome else { return Ok(None) }; // 不是漫画/没超预算——退回原来的整本流程
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
    /// 认到落库 uuid 时顺带按这本书的阅读方向设置同步手动清单（设了方向就不必等重新优化，见 `staging/direction.rs`）。
    pub fn set_render(&self, name: &str, rc: RenderCheck) -> Result<(), String> {
        let p = self.existing(name)?;
        let uuid = rc.uuid.clone();
        sidecar::update(&p, |d| d.render = Some(rc))?;
        if !uuid.is_empty() {
            if let Some(Err(e)) = self.sync_override(&uuid, self.direction_pref(&p), false) {
                println!("[book-serve] 《{name}》同步阅读方向手动清单失败（不影响投书）: {e}");
            }
        }
        Ok(())
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
}
