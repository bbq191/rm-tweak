//! 优化：EPUB/PDF 原地优化（同步 + 后台线程版）、回执文案。
use super::*;

/// 优化进度回调节流：`optimize_epub_file_streaming` 阶段二可能有几百个条目（大漫画），每条目都
/// 写 sidecar+推 SSE 事件在这台设备的存储上是真实开销（原子写=开临时文件+写+改名，不是内存操作）；
/// 每 N 条目才落一次盘/推一次事件，首尾两条（第 1 条、最后一条）永远落，保证 UI 能看到"刚开始动"
/// 和"到 100% 了"，中间稀疏一点不影响"看着在动"这个体验目标。
pub(super) const OPTIMIZE_PROGRESS_STRIDE: usize = 5;
/// 两次进度上报之间的最短间隔（与 [`OPTIMIZE_PROGRESS_STRIDE`] 同时满足才报，首尾不受限）。只按条目数节流时，
/// 文字书的条目处理得飞快（几百个 xhtml 几秒跑完），一秒内能报十几次；而每次上报 = 一次边车原子写 + 一条 SSE 事件，
/// 网页每收到一条事件就把母版库页的 6 个接口整套重拉一遍（经网关 TLS），整台设备被频繁唤醒。进度条一秒动一次
/// 已经足够"看着在动"（2026-09-24 审计）。
pub(super) const OPTIMIZE_PROGRESS_MIN_GAP: std::time::Duration = std::time::Duration::from_secs(1);

/// 进度上报节流器：首条、末条必报；中间要同时满足"距上次至少 `stride` 条"和"距上次至少 `min_gap`"。
pub(super) struct ProgressThrottle {
    stride: usize,
    min_gap: std::time::Duration,
    last_done: usize,
    last_at: Option<std::time::Instant>,
}

impl ProgressThrottle {
    pub(super) fn new(stride: usize, min_gap: std::time::Duration) -> ProgressThrottle {
        ProgressThrottle { stride, min_gap, last_done: 0, last_at: None }
    }

    /// 这一步（`done`/`total`，`now` 为当前时刻）该不该上报；该报就同时记下这次。
    pub(super) fn should_report(&mut self, done: usize, total: usize, now: std::time::Instant) -> bool {
        let due = done == total
            || done == 1
            || self.last_at.is_none()
            || (done.saturating_sub(self.last_done) >= self.stride && self.last_at.is_some_and(|t| now.saturating_duration_since(t) >= self.min_gap));
        if due {
            self.last_done = done;
            self.last_at = Some(now);
        }
        due
    }
}

/// 优化回执尾注：`（N 章，前→后 字节，剥伪 DRM…）`。
pub(super) fn optimize_note(rep: &optimize::Report) -> String {
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

impl Staging {
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
        let ext = formats::ext_of(name);
        if ext != "epub" && ext != "pdf" {
            return Err("只有 EPUB/PDF 能优化".into());
        }
        let p = self.existing(name)?;
        if ext == "pdf" {
            return self.optimize_pdf(name, &p, on_progress);
        }
        // 按书设置的阅读方向（边车 `direction`，见 `staging/direction.rs`）。已完整优化过、只差方向的书只改 OPF——
        // 再跑一遍完整优化会让每张 JPEG 多一代有损（"别二次优化已优化产物"），几百 MB 的漫画也要几分钟。
        let page_direction = self.direction_pref(&p);
        if let Some(dir) = page_direction {
            if library::probe_level(&p, "epub").0 == "full" && direction::is_stale(Some(dir), bookconv::direction::spine_direction_file(&p)) {
                on_progress(0, 1);
                let msg = self.rewrite_direction_only(name, &p, dir)?;
                on_progress(1, 1);
                return Ok(msg);
            }
        }
        // 漫画 EPUB **保持 EPUB**（2026-09-20 用户拍板：统一"优化不改格式"，文字/目录/内容原样保留）。
        // 此前一度改产出 PDF 以拿到 0% 左右留白，但 PDF 一图一页会丢掉漫画里夹带的文字页；EPUB 的固定内边距
        // 是 xochitl 渲染引擎硬限制，接受它，换取"不变动书籍内容"。图片走 `imgopt::prepare_comic_page_for_epub` 单趟处理。
        // 点前缀隐藏名——真机 552MB《镖人》全集坐实优化能跑到分钟级（流式虽然不再吃内存，但大书
        // 图片多、逐张处理仍要时间），这份临时产物会在目录里存在相当一段时间；`list()` 本来就按
        // `.` 前缀跳过 sidecar，不带点前缀的话这份半成品会被当成一条离谱的"母版库条目"混进列表
        // （2026-09-19 真机复现：真显示过一条 `format:"other"` 的 `....epub.optimizing.tmp`）。
        let tmp = p.with_file_name(format!(".{}.optimizing.tmp", name));
        // 有卷标记的书：把 EPUB 自己的 dc:title 也改成规范名（设备显示名取 dc:title）。
        let stem = name.strip_suffix(".epub").unwrap_or(name);
        let canon_title = bookconv::naming::has_volume_marker(stem).then(|| bookconv::naming::canonical_book_name(stem));
        self.mark_cancellable(name);
        let opts = OptimizeOpts { wash: Some(WashOpts::default()), footnote: FootnoteMode::Anchor, comic_frame: self.comic_frame(), page_direction };
        let cancel = || self.is_cancelled(name);
        // 产出到点前缀临时文件、成功才改名覆盖；出错清掉半成品，不留垃圾在母版库目录。质量门
        // （`check_epub_file`）在改名覆盖**之前**、对着这份临时文件跑——2026-09-23 真机坐实的教训：
        // 门校验不通过就该当成"优化失败"处理，原书留在母版库原样不动，不能让一份带断链引用的
        // 半成品覆盖掉用户原来能正常读的书。`check_epub_file` 走 skeleton（图片留空），不会把
        // 大漫画整本读回内存、不重蹈流式优化本来要避开的 OOM。
        let rep = bookconv::util::produce_then_replace(&tmp, &p, |t| {
            let rep = optimize::StreamingOptimize::new(&p, t, &opts).title(canon_title.as_deref()).cancel(&cancel).run(&mut on_progress)?;
            let check = bookconv::check::check_epub_file(t).map_err(|e| format!("质量门校验失败: {e}"))?;
            if !check.ok {
                return Err(format!("优化产物未通过质量门，{}", check.errors.join("；")));
            }
            Ok(rep)
        })?;
        // 已有的长下载名在这里一并规范成 `书名 - N卷`；目标已存在（重复的同一卷）就保持原名，不覆盖。
        let canon = canonical_staged_name(name);
        let land = self.land_guard();
        let shown = if canon != name && !self.dir.join(&canon).exists() && std::fs::rename(&p, self.dir.join(&canon)).is_ok() {
            let (old_car, new_car) = (sidecar::path_for(&p), sidecar::path_for(&self.dir.join(&canon)));
            if old_car.exists() {
                let _ = std::fs::rename(old_car, new_car);
            }
            canon
        } else {
            name.to_string()
        };
        drop(land);
        Ok(format!("已优化《{shown}》{}", optimize_note(&rep)))
    }

    /// 入库 PDF 的「优化」分支：`bookconv::pdf_ingest::classify_pdf` 先判漫画/无文字层/有文字层——
    /// 漫画或无文字层只裁边（格式不变，原地覆盖，对齐文字 EPUB 优化那条"原地覆盖"路径）；有文字层
    /// 转 EPUB（产出 `<stem>.epub`，成功后原 `.pdf` 挪进隐藏备份 `.pdf-originals/` 保留 7 天，照抄"先写点前缀临时文件、成功才落地"的结构
    /// 的"改名删原文件"结构，只是方向相反）。详见 `bookconv::pdf_ingest` 模块文档（分类阈值、
    /// 三个新依赖的分工、已知的公式区域边界粗粒度限制）。
    pub(super) fn optimize_pdf(&self, name: &str, p: &Path, mut on_progress: impl FnMut(usize, usize)) -> Result<String, String> {
        use bookconv::pdf_ingest::{self, PdfKind};
        match pdf_ingest::classify_pdf(p) {
            PdfKind::Comic | PdfKind::NoTextLayer => {
                let tmp = p.with_file_name(format!(".{name}.optimizing.tmp"));
                let rep = bookconv::util::produce_then_replace(&tmp, p, |t| pdf_ingest::optimize_pdf_trim_only(p, t, &mut on_progress))?;
                Ok(format!("已优化《{name}》（裁边，{} 页）", rep.pages))
            }
            PdfKind::TextLayer => {
                let stem = name.strip_suffix(".pdf").unwrap_or(name);
                let epub_path = p.with_file_name(format!("{stem}.epub"));
                // 同名 EPUB 已在母版库 → 停下不转（2026-09-24 审查：此前 rename 直接覆盖掉那份书，
                // 忙锁也只锁着 `.pdf` 这个名字，覆盖掉的书找不回来）。
                if epub_path.exists() {
                    return Err(format!("母版库里已有《{stem}.epub》，为免覆盖已停止转换；请先删除或改名那一份再优化"));
                }
                let tmp = p.with_file_name(format!(".{stem}.epub.optimizing.tmp"));
                let (mut book, rep, color_css) = pdf_ingest::optimize_pdf_to_epub(p, &mut on_progress)?;
                let bytes = match bookconv::epub::assemble_pdf_derived(&mut book, &color_css) {
                    Ok(b) => b,
                    Err(e) => return Err(format!("组装 EPUB 失败: {e}")),
                };
                // 质量门在改名覆盖之前对临时文件跑——2026-09-23 真机《移动互联软件安装使用手册》
                // 坐实的那个 bug（图片路径多写一层 `../`，图片一张都不显示）就是这条门要拦的形状。
                // 落地前在落名临界区里再查一次同名 EPUB：转换可能要几分钟，开头那次检查之后别的入库路径
                // （上传/抓网文/inbox）可能已经落下同名书——它们都在临界区里挑名落地，这里同样在临界区里
                // 复查 + rename，就不会把那本覆盖掉（2026-09-24 第三轮审计补）。
                let landed = (|| {
                    std::fs::write(&tmp, &bytes).map_err(|e| format!("写出临时文件失败: {e}"))?;
                    let check = bookconv::check::check_epub_file(&tmp).map_err(|e| format!("质量门校验失败: {e}"))?;
                    if !check.ok {
                        return Err(format!("优化产物未通过质量门，{}", check.errors.join("；")));
                    }
                    let _land = self.land_guard();
                    if epub_path.exists() {
                        return Err(format!("转换期间母版库里出现了同名《{stem}.epub》，为免覆盖已放弃这次转换；原 PDF 未动"));
                    }
                    std::fs::rename(&tmp, &epub_path).map_err(|e| format!("回写母版库失败: {e}"))
                })();
                if let Err(e) = landed {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(e);
                }
                // 原 PDF 不直接删，挪进隐藏备份目录保留 [`PDF_ORIGINALS_KEEP_SECS`]——转坏了（多栏/表格类
                // PDF 重排效果差）还能找回原件（2026-09-24 审查：此前 `remove_file` 删了就没了）。
                self.backup_pdf_original(name, p)?;
                Ok(format!(
                    "已优化《{stem}》（PDF→EPUB，{} 页，{} 章，{} 张图，{} 处公式；原 PDF 在备份里保留 {} 天）",
                    rep.pages,
                    rep.chapters,
                    rep.images,
                    rep.formula_blocks,
                    PDF_ORIGINALS_KEEP_SECS / 86_400
                ))
            }
        }
    }

    /// [`Self::optimize`] 的异步版：先做零耗时的格式/存在性校验（错误立即回给调用方，不用等后台线程），
    /// 校验过了才加忙锁、起后台线程跑真正耗时的部分。成功返回后 HTTP 层应立即回"已开始"，真正结果
    /// 通过 `bus` 的 `books`/`staging` 事件 + `GET /staging` 列表里这条目的 `delivered.optimize`
    /// （[`sidecar::OptimizeCheck`]）异步呈现。`catch_unwind` 兜底优化过程中的 panic（如损坏文件触发
    /// 库内部意外崩溃）——绝不能让忙锁卡死在 true 再也清不掉、这条目从此删不掉优化不了。
    pub fn spawn_optimize(&self, name: &str, bus: Arc<rmsvc_core::events::EventBus>) -> Result<(), String> {
        let ext = formats::ext_of(name);
        if ext != "epub" && ext != "pdf" {
            return Err("只有 EPUB/PDF 能优化".into());
        }
        self.existing(name)?;
        if !self.try_start_busy(name) {
            return Err(busy_err(name, ""));
        }
        let now = rmsvc_core::clock::now_secs();
        let _ = self.set_optimize_check(name, sidecar::OptimizeCheck { status: "pending".into(), message: String::new(), at: now, progress: None });
        self.spawn_bg(name, bus, |this, name, bus| {
            let mut throttle = ProgressThrottle::new(OPTIMIZE_PROGRESS_STRIDE, OPTIMIZE_PROGRESS_MIN_GAP);
            let on_progress = |done: usize, total: usize| {
                if throttle.should_report(done, total, std::time::Instant::now()) {
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
            let (status, message) = final_status(result.as_deref().map_err(String::as_str));
            let oc = sidecar::OptimizeCheck { status, message, at, progress: None };
            // 优化成功后条目可能改了名（长下载名规范成 `书名 - N卷`；有文字层 PDF 转成同名 `.epub` 并把原 `.pdf` 挪进备份）——
            // sidecar 是按条目名找文件的（`existing()`），原名这时候已经找不到文件，终态写会静默失败。这里探测一下
            // 有没有发生改名，写去正确的新名字（`on_progress` 那些中途写的进度还是按旧名字写，那时候文件确实
            // 还在原名下，没问题；只有这最后一次终态写需要跟着改名走）。
            let target = this.resolved_optimize_target(name);
            let _ = this.set_optimize_check(&target, oc);
        });
        Ok(())
    }

    /// 优化过程中同一个母版库条目可能被改名（不是新增）：长下载名规范成 `书名 - N卷`（同格式），或有文字层
    /// PDF 转成 `<stem>.epub`（格式变了、原 `.pdf` 已挪进备份）。`name` 是异步操作发起时的原名，改名后原名找不到
    /// 文件，返回改名后的新名字给 sidecar 写终态用；其余情况（没改名、失败）原样返回 `name`。
    /// （漫画 EPUB 优化保持 EPUB，不会再变成 `.pdf`——2026-09-20 起。）
    pub(super) fn resolved_optimize_target(&self, name: &str) -> String {
        let canon = canonical_staged_name(name);
        if canon != name && self.existing(name).is_err() && self.existing(&canon).is_ok() {
            return canon;
        }
        if let Some(stem) = name.strip_suffix(".pdf") {
            let candidate = format!("{stem}.epub");
            if self.existing(name).is_err() && self.existing(&candidate).is_ok() {
                return candidate;
            }
        }
        name.to_string()
    }

    /// 写异步优化结果到边车（书已从母版库删除 → Err，调用方只记日志/静默丢弃，不阻断别的流程）。
    pub fn set_optimize_check(&self, name: &str, oc: sidecar::OptimizeCheck) -> Result<(), String> {
        let p = self.existing(name)?;
        sidecar::update(&p, |d| d.optimize = Some(oc))
    }
}
