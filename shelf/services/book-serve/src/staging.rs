//! 母版库（中间层暂存池）领域模块——三层架构（内容源 → **母版库** → 读器）的交汇点。
//! 三个正交动作各一个方法：**入库**（`stage_new` / `stage_from_path` / [`StagingStore`] 上传模板 / `fetch_article`）、
//! **优化**（`optimize`，只对 EPUB）、**落库**（`deliver` 投 xochitl；KOReader 由 koreader-serve 从同一目录 adopt，
//! 之后前端调 `mark_delivered` 记一笔）。落库＝纯复制母版字节（两读器同字节可对照），母版默认保留可反复落库。
//! 目录 `$XDG_STATE_HOME/shelf/books/staging/`（/home 分区，重启/OTA 不丢；**不套 LRU 淘汰**，留住用户还没落库的书）。
//! 落库记录是同目录隐藏 sidecar `.<name>.delivered`（`sidecar` 模块管读写；本模块只在落库/删书时调它）。
use bookconv::optimize::{self, FootnoteMode, OptimizeOpts};
use crate::sidecar::{self, Delivered, RenderCheck};
use bookconv::wash::WashOpts;
use serde::Serialize;
use rmsvc_core::asset::{AssetItem, AssetStore};
use rmsvc_core::formats::{self, BOOK_EXTS};
use rmsvc_core::fs::{plain_name, unique_path, write_atomic};
use rmsvc_core::xochitl::{Delivery, Xochitl};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// 「优化」档位：`auto`（清洗+优化，缺省）/ `keep-spacing`（清洗但保留段距）/ `plain`（只优化不清洗）。
/// 与 host `epub-optimize --keep-spacing` / `--no-wash` 一一对应。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OptimizeMode {
    #[default]
    Auto,
    KeepSpacing,
    Plain,
}

impl OptimizeMode {
    pub fn parse(s: &str) -> OptimizeMode {
        match s {
            "plain" => OptimizeMode::Plain,
            "keep-spacing" | "keep_spacing" => OptimizeMode::KeepSpacing,
            _ => OptimizeMode::Auto,
        }
    }
    /// 档位 → 清洗层选项（`Plain` 不清洗）。
    pub fn wash(self) -> Option<WashOpts> {
        match self {
            OptimizeMode::Auto => Some(WashOpts::default()),
            OptimizeMode::KeepSpacing => Some(WashOpts { keep_para_spacing: true, ..Default::default() }),
            OptimizeMode::Plain => None,
        }
    }
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
    /// 投原生时未指定文件夹的缺省（配置 `libraryFolder`）。
    library_folder: String,
    /// 投原生体积门（字节，0=不拦）：xochitl `/upload` 超限会直接断连，先拦下来给指引。
    native_limit: u64,
}

impl Staging {
    pub fn new(dir: PathBuf, xochitl: Arc<Xochitl>, library_folder: String, native_limit: u64) -> Staging {
        Staging { dir, xochitl, library_folder, native_limit }
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
    /// 跑一遍跟「母版库→优化」按钮同一个 `optimize()`（Auto 档），不用用户再手动点一次——`article.rs` 的属性
    /// 白名单本来就不留 class/style，正文没有任何 CSS，不经 wash 层的边距/段距归零会在 xochitl 上按默认段距
    /// 渲染出大片留空（真机反馈）。`optimize=false` 保留原行为：core 级落库，用户按需再点。同步优化失败不影响
    /// 入库结果（已经抓到的文章不因为这一步失败就整个丢掉），失败原因原样带回给调用方决定怎么措辞。
    pub fn fetch_article(&self, url: &str, optimize: bool) -> Result<FetchArticleOutcome, String> {
        let (epub, title) = bookconv::article::build_article_epub(url)?;
        let fname = format!("{}.epub", bookconv::util::sanitize_filename(&title, "article"));
        let name = self.stage_new(&fname, &epub)?;
        let (optimized, optimize_error) = if optimize {
            match self.optimize(&name, OptimizeMode::Auto) {
                Ok(_) => (true, None),
                Err(e) => (false, Some(e)),
            }
        } else {
            (false, None)
        };
        Ok(FetchArticleOutcome { name, title, optimized, optimize_error })
    }

    // ───────────── 优化 ─────────────

    /// 对母版库里的 EPUB 跑通用优化（Inline 脚注 + 外链 css 缩进：两读器都能显示），原子回写。返回回执文案。
    pub fn optimize(&self, name: &str, mode: OptimizeMode) -> Result<String, String> {
        if formats::ext_of(name) != "epub" {
            return Err("只有 EPUB 能优化（PDF 重排请在电脑用 shelf push）".into());
        }
        let p = self.existing(name)?;
        let data = std::fs::read(&p).map_err(|e| format!("读母版库文件失败: {e}"))?;
        let (out, rep) = optimize::optimize_epub_with(&data, &OptimizeOpts { wash: mode.wash(), footnote: FootnoteMode::Inline })?;
        write_atomic(&p, &out).map_err(|e| format!("回写母版库失败: {e}"))?;
        Ok(format!("已优化《{name}》{}", optimize_note(&rep)))
    }

    // ───────────── 落库 ─────────────

    /// 投入 xochitl 书库：纯复制原字节（不再优化）。原生阅读器只读 EPUB/PDF（CBZ 漫画不投原生，用户定）。`folder` 空＝配置缺省；
    /// `keep=false` 投完从母版库删除。返回回执文案 + EPUB 的渲染自检计划（调用方起线程跑 `render_check::run`）。
    pub fn deliver(&self, name: &str, folder: &str, keep: bool) -> Result<DeliverOutcome, String> {
        let ct = bookconv::convert::direct_content_type(name)
            .ok_or("原生阅读器只读 EPUB / PDF；此格式请「加入 KOReader」，或在电脑用 shelf push 转成 EPUB")?;
        let p = self.existing(name)?;
        let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        if self.native_limit > 0 && size > self.native_limit {
            return Err(format!(
                "《{name}》{} MB 超过原生阅读器上传上限（{} MB），xochitl 会直接断连。PDF 请在电脑用 shelf push 重推（自动按 60MB 分卷）；EPUB 无法分卷，用 KOReader 读",
                size >> 20,
                self.native_limit >> 20
            ));
        }
        let data = std::fs::read(&p).map_err(|e| format!("读母版库文件失败: {e}"))?;
        let folder = if folder.trim().is_empty() { self.library_folder.as_str() } else { folder.trim() };
        // 自检计划在上传前算好（投书时刻要早于 xochitl 给文档的 createdTime）；统计失败就不自检，不影响投书。
        let render = if formats::ext_of(name) == "epub" {
            bookconv::stats::text_profile(&data).ok().map(|prof| RenderPlan { name: name.to_string(), title: prof.title.clone(), expected: prof.expected_pages(), since_ms: rmsvc_core::clock::now_ms() })
        } else {
            None
        };
        let message = match self.xochitl.upload(&data, name, ct.mime(), folder)? {
            Delivery::Delivered(_) => format!("已投入原生书库《{name}》"),
            Delivery::LikelyDelivered(_) => format!("已投入原生书库《{name}》（设备处理较慢，稍候刷新书库）"),
        };
        let _ = self.mark_delivered(name, Reader::Native);
        if !keep {
            let _ = self.remove(name);
        }
        Ok(DeliverOutcome { message, render })
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
            out.push(StagingEntry { name, bytes: md.len(), format, optimized: level == "full", level, mtime, delivered: sidecar::read(&e.path()) });
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
        let s = Staging::new(t.path().join("staging"), x, "library".into(), 1024 * 1024);
        s.ensure().unwrap();
        s
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
    fn optimize_levels_and_gates() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        let opf = r#"<package version="2.0"><metadata><dc:title>t</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#;
        let epub = mini_epub(&[("content.opf", opf), ("c1.xhtml", r#"<html><head></head><body><h1>第一章</h1><p style="font-size:9px;margin:1em">正文</p></body></html>"#)]);
        s.stage_new("x.epub", &epub).unwrap();
        s.stage_new("p.pdf", b"%PDF").unwrap();
        assert!(s.optimize("p.pdf", OptimizeMode::Auto).unwrap_err().contains("只有 EPUB"));
        assert!(s.optimize("none.epub", OptimizeMode::Auto).is_err());
        let msg = s.optimize("x.epub", OptimizeMode::Plain).unwrap();
        assert!(msg.contains("只优化") && msg.contains("1 章"), "{msg}");
        assert_eq!(s.list().iter().find(|e| e.name == "x.epub").unwrap().level, "core", "不清洗只算核心遍");
        let msg = s.optimize("x.epub", OptimizeMode::KeepSpacing).unwrap();
        assert!(msg.contains("清洗+优化") && msg.contains("自动目录 1 条"), "{msg}");
        let e = s.list().into_iter().find(|e| e.name == "x.epub").unwrap();
        assert!(e.optimized && e.level == "full");
        assert_eq!(OptimizeMode::parse("keep-spacing").wash().unwrap().keep_para_spacing, true);
        assert!(OptimizeMode::parse("plain").wash().is_none() && OptimizeMode::parse("").wash().is_some());
    }

    #[test]
    fn deliver_gates_format_before_touching_xochitl() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        s.stage_new("c.cbz", b"PK").unwrap();
        s.stage_new("d.pdf", b"%PDF").unwrap();
        assert_eq!(s.list().iter().find(|e| e.name == "c.cbz").unwrap().format, "cbz");
        assert!(s.deliver("c.cbz", "", true).unwrap_err().contains("只读 EPUB / PDF"));
        // 体积门：超过 native_limit（测试设 1MB）不碰 xochitl，回执指引分卷
        s.stage_new("huge.pdf", &vec![b'%'; 2 * 1024 * 1024]).unwrap();
        let e = s.deliver("huge.pdf", "", true).unwrap_err();
        assert!(e.contains("超过原生阅读器上传上限") && e.contains("分卷"), "{e}");
        // PDF 走到 xochitl 才失败（不可达），母版仍在、无落库记录
        assert!(s.deliver("d.pdf", "", true).is_err());
        assert!(s.list().iter().any(|e| e.name == "d.pdf" && e.delivered.is_none()));
    }

    #[test]
    fn upload_flow_lands_books_and_rejects_non_books() {
        let t = tempfile::tempdir().unwrap();
        let s = staging(&t);
        let work = t.path().join(".work");
        let mut body = Vec::new();
        for (f, d) in [("../中文 名.azw3", "内容"), ("pic.jpg", "x"), ("e.epub", ""), ("中文 名.azw3", "again")] {
            body.extend_from_slice(format!("--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{f}\"\r\n\r\n{d}\r\n").as_bytes());
        }
        body.extend_from_slice(b"--B--\r\n");
        let out = AssetUploadFlow::in_dir(work.clone()).run(&StagingStore(&s), &body[..], "B").unwrap();
        assert!(out[0].ok && out[0].name == "中文 名.azw3" && out[0].message == "已入母版库");
        assert!(!out[1].ok && out[1].message.contains("不是书籍格式") && out[1].message.contains(".epub"));
        assert!(!out[2].ok && out[2].message == "空文件");
        assert!(out[3].ok && out[3].name == "1_中文 名.azw3" && out[3].message.contains("存为 1_中文 名.azw3"));
        assert_eq!(std::fs::read(s.dir().join("中文 名.azw3")).unwrap(), "内容".as_bytes());
        assert!(std::fs::read_dir(&work).unwrap().next().is_none(), "暂存 .work 应清空");
    }
}
