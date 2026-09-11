//! 最小合规 EPUB3 组装 —— 移植自 protocol/epub.py。mimetype 首个 STORED。
//! 本设备版所有条目走 STORED（不压缩，免 C 依赖；设备空间充足）。

use crate::htmlproc::fix_internal_links;

pub struct Chapter {
    pub title: String,
    pub html_body: String,
    /// 目录层级（1=部/篇顶层，>=2 为其下的章/节）。用于生成嵌套 nav。
    pub level: i64,
}

pub struct BookMeta {
    pub book_id: String,
    pub title: String,
    pub author: String,
    pub language: String,
    pub publisher: String,
    pub cover: Option<Vec<u8>>,
    pub cover_ext: String,
    pub cover_media_type: String,
}

/// 章节 HTML 引用的图片等资源（如 FB2/MOBI 内联插图）。
/// `path` 为 OEBPS 内相对路径（如 `images/img1.jpg`），章节 html 以此相对引用。
pub struct Resource {
    pub path: String,
    pub media_type: String,
    pub bytes: Vec<u8>,
}

pub struct Book {
    pub meta: BookMeta,
    pub chapters: Vec<Chapter>,
    /// 章节引用的嵌入资源；默认空（墨香下书路径不用）。FB2/MOBI 转换填插图。
    pub resources: Vec<Resource>,
    /// 显式目录。空＝沿用"每个有标题的章节文件一条目录"；非空＝按这里生成 nav，条目可以指向
    /// 文件内锚点、也可以多条指向同一个文件（PDF→EPUB 单文件模式/分组标题，见
    /// `pdf_ingest::to_epub`）。
    pub nav: Vec<NavEntry>,
}

/// 一条目录：`href` 是相对 `OEBPS/` 的章节文件名，可带 `#锚点`。
#[derive(Clone, Debug, PartialEq)]
pub struct NavEntry {
    pub title: String,
    pub level: i64,
    pub href: String,
}

/// 对齐 xml.sax.saxutils.escape：只转 & < >（不动引号）。
use crate::util::xml_escape as xesc;

/// `assemble` 写出的 OPF 在 zip 里的路径（`container.xml` 指向它；PDF 来源识别等也按这个路径读）。
pub(crate) const OPF_PATH: &str = "OEBPS/content.opf";
/// OPF `dc:identifier` 的前缀：`weread:{book_id}`。`pdf_ingest::looks_like_pdf_derived_epub` 靠
/// 它加 `book_id` 的 `pdf:` 前缀识别"PDF 转出的 EPUB"，两边必须同源。
pub(crate) const ID_SCHEME: &str = "weread:";

pub(crate) fn chapter_filename(i: usize) -> String {
    format!("chap_{:04}.xhtml", i + 1)
}

/// `head_extra` 原样插在 `</head>` 前（外链样式表 `<link>` 等），普通章节传 `""`。
fn chapter_doc(ch: &Chapter, head_extra: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"zh\">\n<head><title>{}</title>{head_extra}</head>\n<body>{}</body>\n</html>\n",
        xesc(&ch.title),
        ch.html_body
    )
}

fn container_xml() -> &'static str {
    "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\n  <rootfiles>\n    <rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/>\n  </rootfiles>\n</container>\n"
}

fn cover_xhtml(m: &BookMeta) -> String {
    // 封面**水平+垂直双居中**、限高一屏：早先只 text-align:center（仅水平居中、height:auto 垂直贴顶），
    // 宽高比偏方/偏宽的封面宽度撑满后高度不足就贴页顶 → 书库缩略图「偏上」。table/table-cell +
    // vertical-align:middle 是老渲染器（xochitl epub 引擎）也吃的垂直居中法；max-height:100vh 防超高。
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"zh\">\n<head><title>封面</title>\n<style>html,body{{margin:0;padding:0;height:100%;}} .cv{{display:table;width:100%;height:100vh;}} .cv-c{{display:table-cell;vertical-align:middle;text-align:center;}} .cv-c img{{max-width:100%;max-height:100vh;}}</style></head>\n<body>\n  <div class=\"cv\"><div class=\"cv-c\"><img src=\"cover.{}\" alt=\"{}\"/></div></div>\n</body>\n</html>\n",
        m.cover_ext,
        xesc(&m.title)
    )
}

fn content_opf(book: &Book) -> String {
    let m = &book.meta;
    let has_cover = m.cover.is_some();
    let mut manifest: Vec<String> = vec![
        "    <item id=\"nav\" href=\"nav.xhtml\" media-type=\"application/xhtml+xml\" properties=\"nav\"/>".to_string(),
    ];
    let mut spine: Vec<String> = Vec::new();
    if has_cover {
        manifest.push(format!(
            "    <item id=\"cover-image\" href=\"cover.{}\" media-type=\"{}\" properties=\"cover-image\"/>",
            m.cover_ext, m.cover_media_type
        ));
        manifest.push("    <item id=\"cover\" href=\"cover.xhtml\" media-type=\"application/xhtml+xml\"/>".to_string());
        spine.push("    <itemref idref=\"cover\"/>".to_string());
    }
    for i in 0..book.chapters.len() {
        let cid = format!("c{}", i + 1);
        let href = chapter_filename(i);
        manifest.push(format!(
            "    <item id=\"{cid}\" href=\"{href}\" media-type=\"application/xhtml+xml\"/>"
        ));
        spine.push(format!("    <itemref idref=\"{cid}\"/>"));
    }
    // 嵌入资源（插图等），只进 manifest、不进 spine
    for (i, r) in book.resources.iter().enumerate() {
        manifest.push(format!(
            "    <item id=\"res{}\" href=\"{}\" media-type=\"{}\"/>",
            i + 1,
            xesc(&r.path),
            xesc(&r.media_type)
        ));
    }
    let author = if m.author.is_empty() {
        String::new()
    } else {
        format!("\n    <dc:creator>{}</dc:creator>", xesc(&m.author))
    };
    let publisher = if m.publisher.is_empty() {
        String::new()
    } else {
        format!("\n    <dc:publisher>{}</dc:publisher>", xesc(&m.publisher))
    };
    let cover_meta = if has_cover {
        "\n    <meta name=\"cover\" content=\"cover-image\"/>"
    } else {
        ""
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"pub-id\">\n  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n    <dc:identifier id=\"pub-id\">{ID_SCHEME}{}</dc:identifier>\n    <dc:title>{}</dc:title>\n    <dc:language>{}</dc:language>{}{}{}\n  </metadata>\n  <manifest>\n{}\n  </manifest>\n  <spine>\n{}\n  </spine>\n</package>\n",
        xesc(&m.book_id),
        xesc(&m.title),
        xesc(&m.language),
        author,
        publisher,
        cover_meta,
        manifest.join("\n"),
        spine.join("\n")
    )
}

/// 生成嵌套 nav：level<=1 为顶层 <li>，level>=2 收进上一个顶层项的子 <ol>。
/// 只收非空标题章（整章一页后每章都带标题）。仅两层——微信读书目录最多部/章两级。
fn nav_body(book: &Book) -> String {
    let visible: Vec<NavEntry> = if book.nav.is_empty() {
        book.chapters
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.title.is_empty())
            .map(|(i, c)| NavEntry { title: c.title.clone(), level: c.level, href: chapter_filename(i) })
            .collect()
    } else {
        book.nav.iter().filter(|n| !n.title.is_empty()).cloned().collect()
    };
    let mut out = String::new();
    let mut sub_open = false; // 是否有未闭合的子 <ol>
    for (idx, ch) in visible.iter().enumerate() {
        let link = format!("<a href=\"{}\">{}</a>", xesc(&ch.href), xesc(&ch.title));
        if ch.level <= 1 {
            if sub_open {
                out.push_str("        </ol>\n      </li>\n");
                sub_open = false;
            }
            let has_child = visible.get(idx + 1).is_some_and(|n| n.level >= 2);
            if has_child {
                out.push_str(&format!("      <li>{link}\n        <ol>\n"));
                sub_open = true;
            } else {
                out.push_str(&format!("      <li>{link}</li>\n"));
            }
        } else if sub_open {
            out.push_str(&format!("          <li>{link}</li>\n"));
        } else {
            // 容错：level>=2 却无顶层父（书首即子节），平铺为顶层
            out.push_str(&format!("      <li>{link}</li>\n"));
        }
    }
    if sub_open {
        out.push_str("        </ol>\n      </li>\n");
    }
    out
}

fn nav_xhtml(book: &Book) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xmlns:epub=\"http://www.idpf.org/2007/ops\" xml:lang=\"zh\">\n<head><title>目录</title></head>\n<body>\n  <nav epub:type=\"toc\" id=\"toc\">\n    <ol>\n{}    </ol>\n  </nav>\n</body>\n</html>\n",
        nav_body(book)
    )
}


/// 组装时可选的外链共用样式表：写进 `OEBPS/<file>`、OPF manifest 补一项、**只给正文含 `<img` 的章节**挂 `<link>`
/// （纯文字页不该吃到 `body{margin:0}` 之类的清零规则）。漫画分卷（`comic_split::build_piece`）用——此前是
/// `assemble` 出整本 zip 后再整本读回内存、改条目、重打包一遍（峰值 3–4 份整卷体积），现在组装时一次写成。
pub(crate) struct SharedCss<'a> {
    /// `OEBPS/` 下的文件名（也是章节 `<link href>` 的值）。
    pub file: &'a str,
    /// manifest 项 id。
    pub id: &'a str,
    pub css: &'a str,
}

/// [`assemble`] 的内部变体选项。`Default` = 与 [`assemble`] 完全相同。
#[derive(Default)]
pub(crate) struct AssembleOpts<'a> {
    pub shared_css: Option<SharedCss<'a>>,
    /// 写完一份资源就释放它（组装后 `book.resources` 为空）：调用方不再用这批资源时，省掉"资源 + zip 缓冲"同时驻留的一整份体积。
    pub consume_resources: bool,
    /// 书是"从右往左"翻页（日漫）：OPF `<spine>` 写上 `page-progression-direction="rtl"`。按卷拆分时从原书继承，
    /// 否则分卷在 xochitl 里的"日漫从右往左翻页"认不出来（`reader-page-turn.qmd` 只看这个属性，2026-09-24）。
    pub rtl: bool,
}

/// 不区分大小写的 `<img` 探测（不为此分配整章小写副本）。
fn has_img_tag(html: &str) -> bool {
    html.as_bytes().windows(4).any(|w| w.eq_ignore_ascii_case(b"<img"))
}

/// PDF→EPUB 颜色 span 探测（`optimize_pdf_to_epub` 生成的 `class="cj-cN"`）——共享 CSS 现在除了
/// `pdf-img.css` 的图片尺寸规则，还可能带颜色规则（见 `assemble_pdf_derived`），纯文字页也可能
/// 用到颜色，不能再只按"有没有 `<img`"决定要不要挂 `<link>`（2026-09-23）。
fn has_color_span(html: &str) -> bool {
    html.contains("class=\"cj-c")
}

/// 把 Book 打包成 EPUB 字节。组装前对每章：先 fix_internal_links（脚注同文件锚点规整），
/// 再 break_footnote_cycles（拆双向脚注互指对——reMarkable 索引器遇互指对会整对丢弃致点不动）。
pub fn assemble(book: &mut Book) -> Result<Vec<u8>, String> {
    assemble_with(book, AssembleOpts::default())
}

/// PDF→EPUB 转出的书专用：`<img>` 没有 `width`/`height`（源自 PDF 页内嵌图，原始像素尺寸），也没有任何
/// 外链 CSS 撑住布局——跟 `comic_split` 早年撞过的坑同一个根因（`repack_with_comic_css` 头注）：xochitl
/// 原生阅读器走标准文档流，无 CSS 兜底的 `<img>` 不撑满、甚至整个不出现在渲染结果里（2026-09-23 真机
/// 投一本真实 PDF 手册核实：内部生成的预览 PDF 里 `pdfimages -list` 空，24 张图一张没有）。跟漫画
/// `COMIC_CSS`（`width:100%` 强撑满整页）不是一回事——PDF 里的图是跟正文混排的小插图/二维码，不该被
/// 拉伸到整页宽；用 `max-width:100%` 只封顶超宽图，不撑大本来就小的图，也不清零正文页边距。
/// `extra_css`：`optimize_pdf_to_epub` 返回的颜色 CSS（`.cj-cN{color:#rrggbb;}` 逐条，可能是空
/// 串——全书没解析出任何非黑颜色时就是空）。拼进同一份共享样式表而不是另起一个文件：颜色规则
/// 也得挂在"含 `<img` 或颜色 span 才 `<link>`"这同一条判定里，两份文件反而要维护两条判定逻辑。
/// **组装后 `book.resources` 被清空**（写一张释放一张）：PDF 转出的书图片可达上百 MB，不让"资源表 + zip 缓冲"
/// 同时各占一整份（2026-09-24 审计；调用方 book-serve 组装后不再用 `book`）。
pub fn assemble_pdf_derived(book: &mut Book, extra_css: &str) -> Result<Vec<u8>, String> {
    let css = format!("{PDF_IMG_CSS}{extra_css}");
    assemble_with(book, AssembleOpts { shared_css: Some(SharedCss { file: "pdf-img.css", id: "pdf-img-css", css: &css }), consume_resources: true, rtl: false })
}

const PDF_IMG_CSS: &str = "img{max-width:100%;height:auto;}\n";

pub(crate) fn assemble_with(book: &mut Book, opts: AssembleOpts) -> Result<Vec<u8>, String> {
    if book.chapters.is_empty() {
        return Err("EPUB 至少要有一章".into());
    }
    for ch in book.chapters.iter_mut() {
        ch.html_body = fix_internal_links(&ch.html_body);
        ch.html_body = crate::htmlproc::break_footnote_cycles(&ch.html_body);
    }
    let mut opf = content_opf(book);
    if opts.rtl {
        opf = opf.replacen("<spine>", "<spine page-progression-direction=\"rtl\">", 1);
    }
    if let Some(c) = &opts.shared_css {
        opf = opf.replacen("</manifest>", &format!("<item id=\"{}\" href=\"{}\" media-type=\"text/css\"/></manifest>", c.id, c.file), 1);
    }
    // 预留足够容量：全部 STORED，产物 ≈ 资源 + 章节文本 + 少量固定条目。Vec 倍增扩容会在峰值瞬间同时持有新旧两块。
    let cap = book.resources.iter().map(|r| r.bytes.len()).sum::<usize>()
        + book.chapters.iter().map(|c| c.html_body.len() + 512).sum::<usize>()
        + opf.len()
        + 16 * 1024;
    let mut buf: Vec<u8> = Vec::with_capacity(cap);
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut z = zip::ZipWriter::new(cursor);
        let stored = crate::epubzip::stored();
        let put = crate::epubzip::put_entry;
        // mimetype 必须首个、STORED
        put(&mut z, "mimetype", stored, b"application/epub+zip")?;
        put(&mut z, "META-INF/container.xml", stored, container_xml().as_bytes())?;
        put(&mut z, OPF_PATH, stored, opf.as_bytes())?;
        put(&mut z, "OEBPS/nav.xhtml", stored, nav_xhtml(book).as_bytes())?;
        if let Some(cover) = &book.meta.cover {
            put(&mut z, &format!("OEBPS/cover.{}", book.meta.cover_ext), stored, cover)?;
            put(&mut z, "OEBPS/cover.xhtml", stored, cover_xhtml(&book.meta).as_bytes())?;
        }
        for (i, ch) in book.chapters.iter().enumerate() {
            let link = match &opts.shared_css {
                Some(c) if has_img_tag(&ch.html_body) || has_color_span(&ch.html_body) => format!("<link rel=\"stylesheet\" type=\"text/css\" href=\"{}\"/>", c.file),
                _ => String::new(),
            };
            put(&mut z, &format!("OEBPS/{}", chapter_filename(i)), stored, chapter_doc(ch, &link).as_bytes())?;
        }
        if opts.consume_resources {
            for r in std::mem::take(&mut book.resources) {
                put(&mut z, &format!("OEBPS/{}", r.path), stored, &r.bytes)?; // r 在本次迭代结束即释放
            }
        } else {
            for r in &book.resources {
                put(&mut z, &format!("OEBPS/{}", r.path), stored, &r.bytes)?;
            }
        }
        if let Some(c) = &opts.shared_css {
            put(&mut z, &format!("OEBPS/{}", c.file), stored, c.css.as_bytes())?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

#[cfg(test)]
mod nav_tests {
    use super::*;
    fn ch(title: &str, level: i64) -> Chapter {
        Chapter { title: title.into(), html_body: "<p>x</p>".into(), level }
    }
    #[test]
    fn nests_level2_under_level1() {
        let book = Book {
            meta: BookMeta {
                book_id: "b".into(), title: "t".into(), author: "".into(), language: "zh".into(),
                publisher: "".into(), cover: None, cover_ext: "jpg".into(), cover_media_type: "image/jpeg".into(),
            },
            chapters: vec![ch("第一部", 1), ch("第一章", 2), ch("第二章", 2), ch("第二部", 1)],
            resources: vec![],
            nav: Vec::new(),
        };
        let nav = nav_body(&book);
        // 第一部带子 ol 包住两章，第二部无子节平铺
        assert!(nav.contains("第一部"), "{nav}");
        assert!(nav.matches("<ol>").count() == 1, "应恰有一个子 ol: {nav}");
        assert!(nav.matches("</ol>").count() == 1, "子 ol 未闭合: {nav}");
        let pos_part = nav.find("第一章").unwrap();
        let pos_ol = nav.find("<ol>").unwrap();
        assert!(pos_ol < pos_part, "第一章应在子 ol 内: {nav}");
    }
    #[test]
    fn embeds_resources_into_zip_and_manifest() {
        let mut book = Book {
            meta: BookMeta {
                book_id: "b".into(), title: "t".into(), author: "".into(), language: "zh".into(),
                publisher: "".into(), cover: None, cover_ext: "jpg".into(), cover_media_type: "image/jpeg".into(),
            },
            chapters: vec![Chapter { title: "章".into(), html_body: "<p><img src=\"images/a.png\"/></p>".into(), level: 1 }],
            resources: vec![Resource { path: "images/a.png".into(), media_type: "image/png".into(), bytes: vec![1, 2, 3, 4] }],
            nav: Vec::new(),
        };
        let opf = content_opf(&book);
        assert!(opf.contains("id=\"res1\" href=\"images/a.png\" media-type=\"image/png\""), "manifest 缺资源项: {opf}");
        let bytes = assemble(&mut book).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let mut f = zip.by_name("OEBPS/images/a.png").expect("zip 内应有资源文件");
        use std::io::Read;
        let mut got = Vec::new();
        f.read_to_end(&mut got).unwrap();
        assert_eq!(got, vec![1, 2, 3, 4]);
    }

    #[test]
    fn flat_when_all_level1() {
        let book = Book {
            meta: BookMeta {
                book_id: "b".into(), title: "t".into(), author: "".into(), language: "zh".into(),
                publisher: "".into(), cover: None, cover_ext: "jpg".into(), cover_media_type: "image/jpeg".into(),
            },
            chapters: vec![ch("一", 1), ch("二", 1)],
            resources: vec![],
            nav: Vec::new(),
        };
        let nav = nav_body(&book);
        assert!(!nav.contains("<ol>"), "全顶层不应有子 ol: {nav}");
    }
    fn two_chapter_book(with_img: bool) -> Book {
        Book {
            meta: BookMeta {
                book_id: "b".into(), title: "t".into(), author: "".into(), language: "zh".into(),
                publisher: "".into(), cover: None, cover_ext: "jpg".into(), cover_media_type: "image/jpeg".into(),
            },
            chapters: vec![
                Chapter { title: "图页".into(), html_body: if with_img { "<div><IMG src=\"images/a.png\"/></div>".into() } else { "<p>字</p>".into() }, level: 1 },
                Chapter { title: "字页".into(), html_body: "<p>纯文字</p>".into(), level: 1 },
            ],
            resources: vec![Resource { path: "images/a.png".into(), media_type: "image/png".into(), bytes: vec![9; 64] }],
            nav: Vec::new(),
        }
    }

    fn zip_names_and_text(bytes: Vec<u8>) -> Vec<(String, Vec<u8>)> {
        use std::io::Read;
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        (0..z.len()).map(|i| { let mut f = z.by_index(i).unwrap(); let mut v = Vec::new(); f.read_to_end(&mut v).unwrap(); (f.name().to_string(), v) }).collect()
    }

    #[test]
    fn assemble_with_consume_resources_is_byte_identical_and_empties_resources() {
        let plain = assemble(&mut two_chapter_book(true)).unwrap();
        let mut b = two_chapter_book(true);
        let consumed = assemble_with(&mut b, AssembleOpts { shared_css: None, consume_resources: true, rtl: false }).unwrap();
        assert_eq!(plain, consumed, "consume_resources 只影响内存，不影响产物");
        assert!(b.resources.is_empty(), "资源写完即释放");
    }

    #[test]
    fn shared_css_goes_last_and_links_only_chapters_with_img_case_insensitively() {
        let out = assemble_with(&mut two_chapter_book(true), AssembleOpts { shared_css: Some(SharedCss { file: "x.css", id: "xcss", css: "img{}" }), consume_resources: false, rtl: true }).unwrap();
        let entries = zip_names_and_text(out);
        assert_eq!(entries.last().unwrap().0, "OEBPS/x.css", "样式表条目排在资源之后");
        assert_eq!(entries.last().unwrap().1, b"img{}");
        let get = |n: &str| String::from_utf8(entries.iter().find(|(k, _)| k == n).unwrap().1.clone()).unwrap();
        assert!(get("OEBPS/chap_0001.xhtml").contains("<link rel=\"stylesheet\" type=\"text/css\" href=\"x.css\"/></head>"), "含 <IMG（大小写不敏感）的章要挂 link");
        assert!(!get("OEBPS/chap_0002.xhtml").contains("<link"), "纯文字章不挂");
        assert!(get("OEBPS/content.opf").contains("<item id=\"xcss\" href=\"x.css\" media-type=\"text/css\"/></manifest>"));
        assert!(get("OEBPS/content.opf").contains("<spine page-progression-direction=\"rtl\">"), "rtl 选项写进 spine");
    }

    /// `assemble_pdf_derived` 是 book-serve PDF→EPUB 路径实际调用的入口（2026-09-23 真机投一本真实 PDF
    /// 手册发现：没有这条 CSS 时图片在 xochitl 原生阅读器里完全不出现，见函数头注）。这里只确认它正确
    /// 接上了 `pdf-img.css`/`max-width`，不重复 `shared_css_goes_last...` 已经测过的通用机制。
    #[test]
    fn assemble_pdf_derived_links_max_width_css_to_image_chapters_only() {
        let out = assemble_pdf_derived(&mut two_chapter_book(true), "").unwrap();
        let entries = zip_names_and_text(out);
        let get = |n: &str| String::from_utf8(entries.iter().find(|(k, _)| k == n).unwrap().1.clone()).unwrap();
        assert_eq!(get("OEBPS/pdf-img.css"), "img{max-width:100%;height:auto;}\n");
        assert!(get("OEBPS/chap_0001.xhtml").contains("href=\"pdf-img.css\""), "含图的章要挂 pdf-img.css");
        assert!(!get("OEBPS/chap_0002.xhtml").contains("<link"), "纯文字章不挂");
        assert!(get("OEBPS/content.opf").contains("<item id=\"pdf-img-css\" href=\"pdf-img.css\" media-type=\"text/css\"/>"));
    }

    #[test]
    fn explicit_nav_entries_can_share_a_file_and_carry_fragments() {
        let mut b = two_chapter_book(false);
        b.nav = vec![
            NavEntry { title: "栏目".into(), level: 1, href: "chap_0001.xhtml".into() },
            NavEntry { title: "文章甲".into(), level: 2, href: "chap_0001.xhtml#a".into() },
            NavEntry { title: "文章乙".into(), level: 2, href: "chap_0002.xhtml#b".into() },
        ];
        let nav = nav_body(&b);
        assert!(nav.contains("<li><a href=\"chap_0001.xhtml\">栏目</a>\n        <ol>"), "{nav}");
        assert!(nav.contains("<li><a href=\"chap_0001.xhtml#a\">文章甲</a></li>") && nav.contains("<li><a href=\"chap_0002.xhtml#b\">文章乙</a></li>"), "{nav}");
        assert!(!nav.contains("图页") && !nav.contains("字页"), "有显式目录时不再按章节生成: {nav}");
    }

}
