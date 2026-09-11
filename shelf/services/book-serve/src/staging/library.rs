//! 母版库的查 / 删 / 列表与启动期维护（孤儿边车、被中断记录恢复）。
use super::*;

/// 判定一本母版库文件的优化等级（`full`/`core`/`old`/`none`）与是否 PDF 转出的 EPUB——要开 zip / 读文件头尾，
/// 结果由 `Staging::list` 按（大小, 修改时间）缓存。
pub(super) fn probe_level(path: &Path, format: &str) -> (&'static str, bool) {
    let pdf_source = format == "epub" && bookconv::pdf_ingest::looks_like_pdf_derived_epub(path);
    let level = if format == "pdf" {
        if bookconv::convert::pdfwrite::looks_like_own_bookconv_pdf(path) { "full" } else { "none" }
    } else if pdf_source {
        "full"
    } else if format != "epub" {
        "none"
    } else {
        match path.to_str().and_then(optimize::optimized_version_file) {
            Some(v) if v == optimize::OPTIMIZE_VERSION => "full",
            Some(v) if v.ends_with("-core") => "core",
            Some(_) => "old",
            None => "none",
        }
    };
    (level, pdf_source)
}

/// `path` 所在文件系统的可用字节数（`f_bavail × f_frsize`）。
pub(super) fn free_bytes_of(path: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` 是合法的 NUL 结尾 C 字符串；`st` 是足够大的零初始化 statvfs，成功返回后由内核填好。
    if unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statvfs 返回 0，结构体已被内核写入。
    let st = unsafe { st.assume_init() };
    st.f_bavail.checked_mul(st.f_frsize)
}

#[derive(Clone)]
pub(super) struct ProbeCache {
    pub(super) len: u64,
    pub(super) modified: Option<std::time::SystemTime>,
    pub(super) level: &'static str,
    pub(super) pdf_source: bool,
    /// EPUB OPF 里写明的翻页方向（判"方向设置待优化"用；非 EPUB 恒 `None`）。
    pub(super) spine: Option<bookconv::direction::PageDirection>,
}

impl Staging {
    /// 清理没有对应书的落库边车（`.<书名>.delivered`）：书早已删除/被外部清掉，边车成了孤儿。留着不仅占目录，
    /// 更会让**同名新书**误继承旧的"已加入/渲染"记录。返回清掉的个数。
    pub fn gc_orphan_sidecars(&self) -> usize {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return 0 };
        let mut n = 0;
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Some(book) = name.strip_prefix('.').and_then(|s| s.strip_suffix(".delivered")) else { continue };
            if !self.dir.join(book).is_file() && std::fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
        n
    }
    /// 启动时修正上一个进程被打断留下的状态（崩溃 / OOM / 被 systemd 杀 / 断电）：
    /// - 边车里停在 `pending` 的优化 / 落库记录 → 改成 `failed`（否则界面永远显示"处理中"，而实际早没有线程在跑）；
    /// - 渲染自检停在 `pending` → `timeout`（自检线程随进程没了；xochitl 可能延后渲染，打开一次就有页数）；
    /// - `.<书名>.optimizing.tmp` 半成品、跨分区入库的 `.<…>.landing.tmp`（点前缀，列表看不见，可达数百 MB）→ 删除。
    ///
    /// 只在启动时调用（此时不可能有操作在跑）。返回 (修正的记录数, 清掉的半成品数)。
    pub fn recover_interrupted(&self) -> (usize, usize) {
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return (0, 0) };
        let now = rmsvc_core::clock::now_secs();
        let (mut fixed, mut tmps) = (0, 0);
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name.starts_with('.') && (name.ends_with(".optimizing.tmp") || name.ends_with(super::intake::LANDING_TMP_SUFFIX)) {
                if std::fs::remove_file(e.path()).is_ok() {
                    tmps += 1;
                }
                continue;
            }
            let Some(book) = name.strip_prefix('.').and_then(|s| s.strip_suffix(".delivered")) else { continue };
            let book_path = self.dir.join(book);
            let stale = |st: &str| st == "pending";
            let Some(d) = sidecar::read(&book_path) else { continue };
            let needs = d.optimize.as_ref().is_some_and(|o| stale(&o.status))
                || d.deliver.as_ref().is_some_and(|o| stale(&o.status))
                || d.render.as_ref().is_some_and(|o| stale(&o.status));
            if !needs {
                continue;
            }
            let ok = sidecar::update(&book_path, |d| {
                if let Some(o) = d.optimize.as_mut().filter(|o| stale(&o.status)) {
                    *o = sidecar::OptimizeCheck { status: "failed".into(), message: "服务重启，上次优化被中断，可重新点「优化」".into(), at: now, progress: None };
                }
                if let Some(o) = d.deliver.as_mut().filter(|o| stale(&o.status)) {
                    *o = sidecar::DeliverCheck { status: "failed".into(), message: "服务重启，上次加入被中断，可重新加入".into(), at: now, progress: None };
                }
                if let Some(r) = d.render.as_mut().filter(|r| stale(&r.status)) {
                    r.status = "timeout".into();
                    r.at = now;
                }
            })
            .is_ok();
            if ok {
                fixed += 1;
            }
        }
        (fixed, tmps)
    }
    /// 把转换完的原 PDF 挪进 [`PDF_ORIGINALS_DIR`]（同分区 rename，不拷贝），顺手清过期备份。
    /// 同名旧备份直接被新的替换。它的落库边车留给 [`Self::gc_orphan_sidecars`] 按孤儿清。
    pub(super) fn backup_pdf_original(&self, name: &str, p: &Path) -> Result<(), String> {
        let dir = self.dir.join(PDF_ORIGINALS_DIR);
        std::fs::create_dir_all(&dir).map_err(|e| format!("建原 PDF 备份目录失败: {e}"))?;
        std::fs::rename(p, dir.join(name)).map_err(|e| format!("备份原 PDF 失败: {e}"))?;
        self.gc_pdf_originals(PDF_ORIGINALS_KEEP_SECS);
        Ok(())
    }

    /// 删掉挪进备份目录已超过 `keep_secs` 的原 PDF。按 ctime 算而不是 mtime：rename 不改 mtime
    /// （那是 PDF 入库的时间，按它算会删得过早），但会刷新 inode 的 ctime。返回清掉的个数。
    pub fn gc_pdf_originals(&self, keep_secs: u64) -> usize {
        use std::os::unix::fs::MetadataExt;
        let Ok(rd) = std::fs::read_dir(self.dir.join(PDF_ORIGINALS_DIR)) else { return 0 };
        let now = rmsvc_core::clock::now_secs();
        let mut n = 0;
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            let ctime = u64::try_from(md.ctime()).unwrap_or(0);
            if md.is_file() && now.saturating_sub(ctime) >= keep_secs && std::fs::remove_file(e.path()).is_ok() {
                n += 1;
            }
        }
        n
    }
    /// 原 PDF 备份列表（新的在前）。`expiresAt` = 挪进来的时间（ctime）+ 保留期。
    pub fn list_originals(&self) -> Vec<OriginalEntry> {
        use std::os::unix::fs::MetadataExt;
        let Ok(rd) = std::fs::read_dir(self.dir.join(PDF_ORIGINALS_DIR)) else { return vec![] };
        let mut out: Vec<OriginalEntry> = rd
            .flatten()
            .filter_map(|e| {
                let md = e.metadata().ok().filter(|m| m.is_file())?;
                let name = e.file_name().to_str()?.to_string();
                let at = u64::try_from(md.ctime()).unwrap_or(0);
                Some(OriginalEntry { name, bytes: md.len(), backed_up_at: at, expires_at: at + PDF_ORIGINALS_KEEP_SECS })
            })
            .collect();
        out.sort_by(|a, b| b.backed_up_at.cmp(&a.backed_up_at).then_with(|| a.name.cmp(&b.name)));
        out
    }

    fn original_path(&self, name: &str) -> Result<PathBuf, String> {
        let p = self.dir.join(PDF_ORIGINALS_DIR).join(plain_name(name)?);
        if !p.is_file() {
            return Err("备份里没有这份 PDF（可能已过期被清掉）".into());
        }
        Ok(p)
    }

    /// 把备份里的原 PDF 挪回母版库（同名条目已在 → 拒绝，不覆盖）。它转出来的 EPUB 不动，要不要删由用户决定。
    pub fn restore_original(&self, name: &str) -> Result<(), String> {
        let src = self.original_path(name)?;
        let dst = self.path_of(name)?;
        if !self.try_start_busy(name) {
            return Err(busy_err(name, "再恢复"));
        }
        let land = self.land_guard();
        let r = if dst.exists() {
            Err(format!("母版库里已有《{name}》，为免覆盖没有恢复；先删除或改名那一份"))
        } else {
            std::fs::rename(&src, &dst).map_err(|e| format!("恢复失败: {e}"))
        };
        drop(land);
        self.end_busy(name);
        r
    }

    /// 提前删掉一份原 PDF 备份（不等 7 天过期）。
    pub fn delete_original(&self, name: &str) -> Result<(), String> {
        std::fs::remove_file(self.original_path(name)?).map_err(|e| format!("删除失败: {e}"))
    }

    /// 母版库条目改名：只改文件名（不改书内的书名/作者），格式不能变——新名字不带扩展名就沿用原扩展名，
    /// 带了别的扩展名则拒绝。新名已存在 / 任一名字正在处理中 → 拒绝。落库边车跟着改名。返回新名字。
    pub fn rename(&self, name: &str, new_name: &str) -> Result<String, String> {
        let src = self.existing(name)?;
        let ext = formats::ext_of(name);
        let new_name = new_name.trim();
        let new_name = if formats::ext_of(new_name) == ext { new_name.to_string() } else { format!("{new_name}.{ext}") };
        let stem_ok = new_name.strip_suffix(&format!(".{ext}")).is_some_and(|s| !s.trim().is_empty());
        if !stem_ok {
            return Err("新名字不能为空".into());
        }
        let dst = self.path_of(&new_name)?;
        if new_name == name {
            return Ok(new_name);
        }
        if !self.try_start_busy(name) {
            return Err(busy_err(name, "再改名"));
        }
        if !self.try_start_busy(&new_name) {
            self.end_busy(name);
            return Err(busy_err(&new_name, "再改名"));
        }
        let r = (|| {
            let _land = self.land_guard();
            if dst.exists() {
                return Err(format!("母版库里已有《{new_name}》"));
            }
            std::fs::rename(&src, &dst).map_err(|e| format!("改名失败: {e}"))?;
            let (old_car, new_car) = (sidecar::path_for(&src), sidecar::path_for(&dst));
            if old_car.exists() {
                let _ = std::fs::rename(old_car, new_car);
            }
            Ok(new_name.clone())
        })();
        self.end_busy(&new_name);
        self.end_busy(name);
        r
    }

    /// 打开母版库条目供下载：`(文件, 字节数)`。
    pub fn open_for_download(&self, name: &str) -> Result<(std::fs::File, u64), String> {
        let p = self.existing(name)?;
        let f = std::fs::File::open(&p).map_err(|e| format!("打开失败: {e}"))?;
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        Ok((f, len))
    }
    // ───────────── 查 / 删 ─────────────

    /// 删除一本书（连同落库边车）。删除期间**占着忙锁**：此前只是先查"忙不忙"再删，查完到删之间别的请求可能刚好
    /// 开始优化/落库/改名，后台线程随即对着一个已删的文件跑（2026-09-25 第四轮审计）。
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let p = self.path_of(name)?;
        if !self.try_start_busy(name) {
            return Err(busy_err(name, "再删除"));
        }
        sidecar::remove(&p);
        let r = std::fs::remove_file(&p).map_err(|e| format!("删除失败: {e}"));
        self.end_busy(name);
        r
    }

    /// 所在分区剩余空间（字节，非特权用户可用的那部分，与 `df` 的 Available 同义）；查不到 None。
    /// 走 `statvfs(2)`：`GET /staging` 每次网页刷新都调，此前每次 fork+exec 一个 `df -k` 再解析文本
    /// （busybox 设备名过长时还要拍平换行的输出），现在一次系统调用，无子进程。
    pub fn free_bytes(&self) -> Option<u64> {
        free_bytes_of(&self.dir)
    }

    /// 列母版库，最新入库在前（同秒按名）。隐藏文件（sidecar / 半成品）不列。
    pub fn list(&self) -> Vec<StagingEntry> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.dir) else { return out };
        let mut seen = std::collections::HashSet::new();
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
            // 优化状态对 EPUB 有意义；PDF 里"我们自己优化产出的产物"（漫画 EPUB 分卷投递的 PDF 件或入库 PDF 裁边）
            // 也算已优化（靠书签目录或 Producer 标记廉价识别，见 `pdfwrite.rs::looks_like_own_
            // bookconv_pdf` 文档注释——用户自己上传的原生 PDF 没有这俩标记，维持 none）。
            // 入库 PDF 转出来的 EPUB（`pdf_source`）视为一次性产物已经完成，直接报 full，不进
            // 常规文字 EPUB 那条 full/core/old/none 优化阶梯——真跑一遍 `optimized_version_file`
            // 只会白白报 none（这类 EPUB 从没被 `optimize_epub_file_streaming` 处理过，没有那个
            // 内嵌版本标记），误导前端以为它"未优化"、显示可以点「优化」，见 `StagingEntry::
            // pdf_source` 文档。
            let modified = md.modified().ok();
            let cached = rmsvc_core::sync::lock(&self.probes)
                .get(&name)
                .filter(|c| c.len == md.len() && c.modified == modified)
                .cloned();
            let (level, pdf_source, spine) = match cached {
                Some(c) => (c.level, c.pdf_source, c.spine),
                None => {
                    let (level, pdf_source) = probe_level(&e.path(), format);
                    let spine = if format == "epub" { bookconv::direction::spine_direction_file(&e.path()) } else { None };
                    rmsvc_core::sync::lock(&self.probes).insert(name.clone(), ProbeCache { len: md.len(), modified, level, pdf_source, spine });
                    (level, pdf_source, spine)
                }
            };
            seen.insert(name.clone());
            let mtime = md.modified().ok().map(rmsvc_core::clock::secs_of).unwrap_or(0);
            let busy = self.is_busy(&name);
            let mut delivered = sidecar::read(&e.path());
            if let Some(rc) = delivered.as_mut().and_then(|d| d.render.as_mut()) {
                // 直接投入的 EPUB 首次打开才渲染：xochitl 渲染完会把 `.content` 的 pageCount 改成真页数，跟记录里的占位页数不同
                // 就说明已经渲染过 → 升级成 ok。没打开过 → 保持 onopen。
                if rc.status == "onopen" {
                    if let Some(n) = rmsvc_core::xochitl::page_count(self.xochitl.library_dir(), &rc.uuid) {
                        if n != rc.pages {
                            rc.status = "ok".into();
                            rc.pages = n;
                            // 升级结果写回边车：此前只改返回给网页的这份拷贝，边车里永远是 onopen，于是之后每次列表
                            // （网页每收一条事件就拉一次）都要再去 xochitl 书库读一遍这本的 `.content`，读到天荒地老。
                            let uuid = rc.uuid.clone();
                            let _ = sidecar::update(&e.path(), |d| {
                                if let Some(r) = d.render.as_mut().filter(|r| r.status == "onopen" && r.uuid == uuid) {
                                    r.status = "ok".into();
                                    r.pages = n;
                                }
                            });
                        }
                    }
                }
            }
            let pref = if format == "epub" { super::direction::parse_pref(delivered.as_ref().and_then(|d| d.direction.as_deref())) } else { None };
            let direction_stale = super::direction::is_stale(pref, spine);
            out.push(StagingEntry {
                name,
                bytes: md.len(),
                format,
                optimized: level == "full" && !direction_stale,
                level,
                mtime,
                delivered,
                busy,
                pdf_source,
                direction: super::direction::pref_label(pref),
                direction_stale,
            });
        }
        // 已被删除/改名的条目从缓存清掉，避免缓存无限增长
        rmsvc_core::sync::lock(&self.probes).retain(|k, _| seen.contains(k));
        out.sort_by(|a, b| b.mtime.cmp(&a.mtime).then_with(|| a.name.cmp(&b.name)));
        out
    }
}
