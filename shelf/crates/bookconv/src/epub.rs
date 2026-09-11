//! 最小合规 EPUB3 组装 —— 移植自 protocol/epub.py。mimetype 首个 STORED。
//! 本设备版所有条目走 STORED（不压缩，免 C 依赖；设备空间充足）。

use crate::htmlproc::fix_internal_links;
use std::io::Write;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

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
}

/// 对齐 xml.sax.saxutils.escape：只转 & < >（不动引号）。
use crate::util::xml_escape as xesc;

pub(crate) fn chapter_filename(i: usize) -> String {
    format!("chap_{:04}.xhtml", i + 1)
}

fn chapter_doc(ch: &Chapter) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<html xmlns=\"http://www.w3.org/1999/xhtml\" xml:lang=\"zh\">\n<head><title>{}</title></head>\n<body>{}</body>\n</html>\n",
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
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"pub-id\">\n  <metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n    <dc:identifier id=\"pub-id\">weread:{}</dc:identifier>\n    <dc:title>{}</dc:title>\n    <dc:language>{}</dc:language>{}{}{}\n  </metadata>\n  <manifest>\n{}\n  </manifest>\n  <spine>\n{}\n  </spine>\n</package>\n",
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
    let visible: Vec<(usize, &Chapter)> =
        book.chapters.iter().enumerate().filter(|(_, c)| !c.title.is_empty()).collect();
    let mut out = String::new();
    let mut sub_open = false; // 是否有未闭合的子 <ol>
    for (idx, (i, ch)) in visible.iter().enumerate() {
        let link = format!("<a href=\"{}\">{}</a>", chapter_filename(*i), xesc(&ch.title));
        if ch.level <= 1 {
            if sub_open {
                out.push_str("        </ol>\n      </li>\n");
                sub_open = false;
            }
            let has_child = visible.get(idx + 1).map_or(false, |(_, n)| n.level >= 2);
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
        };
        let nav = nav_body(&book);
        assert!(!nav.contains("<ol>"), "全顶层不应有子 ol: {nav}");
    }
}

/// 把 Book 打包成 EPUB 字节。组装前对每章：先 fix_internal_links（脚注同文件锚点规整），
/// 再 break_footnote_cycles（拆双向脚注互指对——reMarkable 索引器遇互指对会整对丢弃致点不动）。
pub fn assemble(book: &mut Book) -> Result<Vec<u8>, String> {
    if book.chapters.is_empty() {
        return Err("EPUB 至少要有一章".into());
    }
    for ch in book.chapters.iter_mut() {
        ch.html_body = fix_internal_links(&ch.html_body);
        ch.html_body = crate::htmlproc::break_footnote_cycles(&ch.html_body);
    }
    let mut buf: Vec<u8> = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut buf);
        let mut z = zip::ZipWriter::new(cursor);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        // mimetype 必须首个、STORED
        z.start_file("mimetype", stored).map_err(|e| e.to_string())?;
        z.write_all(b"application/epub+zip").map_err(|e| e.to_string())?;
        z.start_file("META-INF/container.xml", stored).map_err(|e| e.to_string())?;
        z.write_all(container_xml().as_bytes()).map_err(|e| e.to_string())?;
        z.start_file("OEBPS/content.opf", stored).map_err(|e| e.to_string())?;
        z.write_all(content_opf(book).as_bytes()).map_err(|e| e.to_string())?;
        z.start_file("OEBPS/nav.xhtml", stored).map_err(|e| e.to_string())?;
        z.write_all(nav_xhtml(book).as_bytes()).map_err(|e| e.to_string())?;
        if let Some(cover) = &book.meta.cover {
            z.start_file(format!("OEBPS/cover.{}", book.meta.cover_ext), stored)
                .map_err(|e| e.to_string())?;
            z.write_all(cover).map_err(|e| e.to_string())?;
            z.start_file("OEBPS/cover.xhtml", stored).map_err(|e| e.to_string())?;
            z.write_all(cover_xhtml(&book.meta).as_bytes()).map_err(|e| e.to_string())?;
        }
        for (i, ch) in book.chapters.iter().enumerate() {
            z.start_file(format!("OEBPS/{}", chapter_filename(i)), stored)
                .map_err(|e| e.to_string())?;
            z.write_all(chapter_doc(ch).as_bytes()).map_err(|e| e.to_string())?;
        }
        for r in &book.resources {
            z.start_file(format!("OEBPS/{}", r.path), stored)
                .map_err(|e| e.to_string())?;
            z.write_all(&r.bytes).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
}
