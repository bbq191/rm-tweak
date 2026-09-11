//! FB2 → EPUB。fb2 crate（serde 模型）经 quick-xml 反序列化，映射到 `epub::Book`。
//! 本模块只管「FB2 结构 → epub::{Book,Chapter,Resource}」；EPUB 字节组装 + 通用优化
//! 全交给 epub.rs / optimize.rs（与墨香下书同一条 assemble→optimize 路，不另造轮子）。

use super::common;
use crate::epub::{Book, BookMeta, Chapter, Resource};
use base64::Engine;
use fb2::{
    Author, CiteElement, FictionBook, Image, InlineImage, Poem, PoemStanza, Section, SectionPart,
    StyleElement, StyleLinkElement, Table, TableCellElement, Title, TitleElement,
};
use std::collections::HashMap;

/// FB2 字节 → (优化后的 EPUB 字节, 书名)。
pub fn fb2_to_epub(data: &[u8]) -> Result<(Vec<u8>, String), String> {
    let fb: FictionBook =
        quick_xml::de::from_reader(std::io::BufReader::new(data)).map_err(|e| format!("FB2 解析: {e}"))?;
    let ti = &fb.description.title_info;
    let title = {
        let t = ti.book_title.value.trim();
        if t.is_empty() { "未命名".to_string() } else { t.to_string() }
    };

    // 二进制资源：base64 解码为文件，建 id→OEBPS 相对路径映射供图片引用解析。
    let mut id_to_path: HashMap<String, String> = HashMap::new();
    let mut resources: Vec<Resource> = Vec::new();
    for b in &fb.binaries {
        let bytes = decode_b64(&b.content)?;
        let path = format!("images/{}.{}", common::sanitize_id(&b.id), ext_for_media(&b.content_type));
        id_to_path.insert(b.id.clone(), path.clone());
        resources.push(Resource { path, media_type: b.content_type.clone(), bytes });
    }
    let ctx = Ctx { id_to_path: &id_to_path };

    // 封面：coverpage 首图 href="#id" → 对应二进制字节 + 类型
    let cover_bin = ti
        .cover_page
        .as_ref()
        .and_then(|c| c.images.first())
        .and_then(|im| im.href.as_deref())
        .map(|h| h.trim_start_matches('#'))
        .and_then(|id| fb.binaries.iter().find(|b| b.id == id));
    let (cover, cover_ext, cover_media_type) = match cover_bin {
        Some(b) => (
            decode_b64(&b.content).ok(),
            ext_for_media(&b.content_type).to_string(),
            b.content_type.clone(),
        ),
        None => (None, "jpg".to_string(), "image/jpeg".to_string()),
    };

    // 章节：主 body（第一个，通常无 name；带 name 的是脚注等附属 body，不进正文流）递归展开。
    let mut chapters: Vec<Chapter> = Vec::new();
    if let Some(main) = fb.bodies.first() {
        if let Some(t) = &main.title {
            // 整书大标题作首章
            let text = title_plain(t);
            if !text.is_empty() {
                chapters.push(Chapter { title: text, html_body: String::new(), level: 1 });
            }
        }
        for sec in &main.sections {
            walk_section(sec, 1, &ctx, &mut chapters);
        }
    }
    if chapters.iter().all(|c| c.html_body.trim().is_empty()) {
        return Err("FB2 无可读正文".into());
    }

    let author = authors_to_string(&ti.authors);
    let mut book = Book {
        meta: BookMeta {
            book_id: format!("fb2:{}", common::sanitize_id(&title)),
            title: title.clone(),
            author,
            language: if ti.lang.trim().is_empty() { "zh".into() } else { ti.lang.clone() },
            publisher: String::new(),
            cover,
            cover_ext,
            cover_media_type,
        },
        chapters,
        resources,
    };
    let optimized = common::assemble_optimized(&mut book)?;
    Ok((optimized, title))
}

/// 图片 href（`#binid`）→ OEBPS 相对路径解析上下文。
struct Ctx<'a> {
    id_to_path: &'a HashMap<String, String>,
}
impl Ctx<'_> {
    fn resolve(&self, href: &str) -> Option<&str> {
        self.id_to_path.get(href.trim_start_matches('#')).map(|s| s.as_str())
    }
}

/// 递归展开一个 section 成章节：本节内容成一章，嵌套子节成后续更深层级的章。
fn walk_section(sec: &Section, depth: i64, ctx: &Ctx, out: &mut Vec<Chapter>) {
    let mut title = String::new();
    let mut html = String::new();
    if let Some(content) = &sec.content {
        if let Some(t) = &content.title {
            title = title_plain(t);
        }
        if let Some(img) = &content.image {
            render_image(img, ctx, &mut html);
        }
        for part in &content.content {
            render_section_part(part, ctx, &mut html);
        }
        // 至少产出一章（即便本节只有子节：留个带标题的占位章，spine/nav 才连贯）
        if !html.trim().is_empty() || !title.is_empty() {
            out.push(Chapter { title, html_body: html, level: depth });
        }
        for sub in &content.sections {
            walk_section(sub, depth + 1, ctx, out);
        }
    }
}

fn render_section_part(p: &SectionPart, ctx: &Ctx, out: &mut String) {
    match p {
        SectionPart::Paragraph(par) => {
            out.push_str("<p>");
            render_style(&par.elements, ctx, out);
            out.push_str("</p>\n");
        }
        SectionPart::Subtitle(par) => {
            out.push_str("<h3>");
            render_style(&par.elements, ctx, out);
            out.push_str("</h3>\n");
        }
        SectionPart::Image(img) => render_image(img, ctx, out),
        SectionPart::EmptyLine => out.push_str("<br/>\n"),
        SectionPart::Cite(c) => {
            out.push_str("<blockquote>");
            for e in &c.elements {
                render_cite_element(e, ctx, out);
            }
            out.push_str("</blockquote>\n");
        }
        SectionPart::Poem(poem) => render_poem(poem, ctx, out),
        SectionPart::Table(t) => render_table(t, ctx, out),
    }
}

fn render_cite_element(e: &CiteElement, ctx: &Ctx, out: &mut String) {
    match e {
        CiteElement::Paragraph(p) => {
            out.push_str("<p>");
            render_style(&p.elements, ctx, out);
            out.push_str("</p>\n");
        }
        CiteElement::Subtitle(p) => {
            out.push_str("<h4>");
            render_style(&p.elements, ctx, out);
            out.push_str("</h4>\n");
        }
        CiteElement::Poem(poem) => render_poem(poem, ctx, out),
        CiteElement::Table(t) => render_table(t, ctx, out),
        CiteElement::EmptyLine => out.push_str("<br/>\n"),
    }
}

fn render_poem(poem: &Poem, ctx: &Ctx, out: &mut String) {
    out.push_str("<div class=\"poem\">\n");
    for st in &poem.stanzas {
        match st {
            PoemStanza::Subtitle(p) => {
                out.push_str("<h4>");
                render_style(&p.elements, ctx, out);
                out.push_str("</h4>\n");
            }
            PoemStanza::Stanza(s) => {
                for line in &s.lines {
                    out.push_str("<p>");
                    render_style(&line.elements, ctx, out);
                    out.push_str("</p>\n");
                }
            }
        }
    }
    out.push_str("</div>\n");
}

fn render_table(t: &Table, ctx: &Ctx, out: &mut String) {
    out.push_str("<table>\n");
    for row in &t.rows {
        out.push_str("<tr>");
        for cell in &row.cells {
            let (tag, c) = match cell {
                TableCellElement::Head(c) => ("th", c),
                TableCellElement::Data(c) => ("td", c),
            };
            out.push('<');
            out.push_str(tag);
            out.push('>');
            render_style(&c.elements, ctx, out);
            out.push_str("</");
            out.push_str(tag);
            out.push('>');
        }
        out.push_str("</tr>\n");
    }
    out.push_str("</table>\n");
}

/// 内联样式树 → XHTML。命名样式(Style)透明穿透；内部链接(Link，多为脚注锚)渲成纯文本避免死链。
fn render_style(elems: &[StyleElement], ctx: &Ctx, out: &mut String) {
    for e in elems {
        match e {
            StyleElement::Strong(s) => wrap(out, "strong", &s.elements, ctx),
            StyleElement::Emphasis(s) => wrap(out, "em", &s.elements, ctx),
            StyleElement::Strikethrough(s) => wrap(out, "s", &s.elements, ctx),
            StyleElement::Subscript(s) => wrap(out, "sub", &s.elements, ctx),
            StyleElement::Superscript(s) => wrap(out, "sup", &s.elements, ctx),
            StyleElement::Code(s) => wrap(out, "code", &s.elements, ctx),
            StyleElement::Style(ns) => render_style(&ns.elements, ctx, out),
            StyleElement::Link(l) => render_link(&l.elements, out),
            StyleElement::Image(im) => render_inline_image(im, ctx, out),
            StyleElement::Text(t) => out.push_str(&xesc(t)),
        }
    }
}

fn wrap(out: &mut String, tag: &str, elems: &[StyleElement], ctx: &Ctx) {
    out.push('<');
    out.push_str(tag);
    out.push('>');
    render_style(elems, ctx, out);
    out.push_str("</");
    out.push_str(tag);
    out.push('>');
}

/// 链接内元素只取文本（reMarkable 弹注不可用、内部锚多为脚注，渲成文本最稳）。
fn render_link(elems: &[StyleLinkElement], out: &mut String) {
    for e in elems {
        match e {
            StyleLinkElement::Strong { elements }
            | StyleLinkElement::Emphasis { elements }
            | StyleLinkElement::Style { elements }
            | StyleLinkElement::Strikethrough { elements }
            | StyleLinkElement::Subscript { elements }
            | StyleLinkElement::Superscript { elements }
            | StyleLinkElement::Code { elements } => render_link(elements, out),
            StyleLinkElement::Image(_) => {}
            StyleLinkElement::Text(t) => out.push_str(&xesc(t)),
        }
    }
}

fn render_image(img: &Image, ctx: &Ctx, out: &mut String) {
    if let Some(src) = img.href.as_deref().and_then(|h| ctx.resolve(h)) {
        let alt = img.alt.clone().unwrap_or_default();
        out.push_str(&format!("<p><img src=\"{}\" alt=\"{}\"/></p>\n", src, xesc(&alt)));
    }
}

fn render_inline_image(img: &InlineImage, ctx: &Ctx, out: &mut String) {
    if let Some(src) = img.href.as_deref().and_then(|h| ctx.resolve(h)) {
        let alt = img.alt.clone().unwrap_or_default();
        out.push_str(&format!("<img src=\"{}\" alt=\"{}\"/>", src, xesc(&alt)));
    }
}

fn title_plain(t: &Title) -> String {
    let mut s = String::new();
    for e in &t.elements {
        if let TitleElement::Paragraph(p) = e {
            let mut line = String::new();
            style_plain(&p.elements, &mut line);
            let line = line.trim();
            if !line.is_empty() {
                if !s.is_empty() {
                    s.push(' ');
                }
                s.push_str(line);
            }
        }
    }
    s
}

/// 只抽纯文本（用于标题：进 <title>/nav，不要标签）。
fn style_plain(elems: &[StyleElement], out: &mut String) {
    for e in elems {
        match e {
            StyleElement::Strong(s)
            | StyleElement::Emphasis(s)
            | StyleElement::Strikethrough(s)
            | StyleElement::Subscript(s)
            | StyleElement::Superscript(s)
            | StyleElement::Code(s) => style_plain(&s.elements, out),
            StyleElement::Style(ns) => style_plain(&ns.elements, out),
            StyleElement::Link(l) => render_link(l.elements.as_slice(), out),
            StyleElement::Image(_) => {}
            StyleElement::Text(t) => out.push_str(t),
        }
    }
}

fn authors_to_string(authors: &[Author]) -> String {
    let names: Vec<String> = authors
        .iter()
        .filter_map(|a| match a {
            Author::Verbose(v) => {
                let f = v.first_name.value.trim();
                let l = v.last_name.value.trim();
                let n = format!("{f} {l}");
                let n = n.trim().to_string();
                if n.is_empty() {
                    None
                } else {
                    Some(n)
                }
            }
            Author::Anonymous(a) => a.nickname.as_ref().map(|n| n.value.trim().to_string()).filter(|s| !s.is_empty()),
        })
        .collect();
    names.join("、")
}

/// 对齐 epub.rs 的 xesc：只转 & < >。
use crate::util::xml_escape as xesc;

fn decode_b64(content: &str) -> Result<Vec<u8>, String> {
    // FB2 base64 常含换行/空白，先剥净
    let cleaned: String = content.chars().filter(|c| !c.is_whitespace()).collect();
    base64::engine::general_purpose::STANDARD
        .decode(cleaned.as_bytes())
        .map_err(|e| format!("FB2 二进制 base64 解码: {e}"))
}

fn ext_for_media(mt: &str) -> &'static str {
    match mt.to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        _ => "img",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 最小 FB2：标题 + 作者 + 一张 1x1 PNG 封面(#cover) + 一节含一段带内联图。
    const RED_PNG_B64: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVQI12P4z8AAAAMBAQAY3Y2wAAAAAElFTkSuQmCC";

    fn sample_fb2() -> String {
        format!(
            r##"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0" xmlns:l="http://www.w3.org/1999/xlink">
<description><title-info>
<book-title>测试书</book-title>
<author><first-name>张</first-name><last-name>三</last-name></author>
<coverpage><image l:href="#cover"/></coverpage>
<lang>zh</lang>
</title-info></description>
<body>
<section>
<title><p>第一章</p></title>
<p>正文<emphasis>强调</emphasis>结束。</p>
<p><image l:href="#pic1"/></p>
</section>
</body>
<binary id="cover" content-type="image/png">{png}</binary>
<binary id="pic1" content-type="image/png">{png}</binary>
</FictionBook>"##,
            png = RED_PNG_B64
        )
    }

    #[test]
    fn parses_and_builds_epub() {
        let (epub, title) = fb2_to_epub(sample_fb2().as_bytes()).unwrap();
        assert_eq!(title, "测试书");
        // 是合法 zip/EPUB
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(epub)).unwrap();
        // 章节与资源都在
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "OEBPS/images/pic1.png"), "缺内联图资源: {names:?}");
        assert!(names.iter().any(|n| n == "OEBPS/images/cover.png"), "缺封面二进制资源: {names:?}");
        assert!(names.iter().any(|n| n.ends_with("cover.png")), "缺封面: {names:?}");
        // 章节 html 含强调 + 图引用
        use std::io::Read;
        let mut chap = String::new();
        zip.by_name("OEBPS/chap_0001.xhtml").unwrap().read_to_string(&mut chap).unwrap();
        assert!(chap.contains("<em>强调</em>"), "缺内联样式: {chap}");
        assert!(chap.contains("src=\"images/pic1.png\""), "缺内联图: {chap}");
    }
}
