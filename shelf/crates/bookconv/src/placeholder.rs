//! 大文件"占位 + 替换"投原生用的占位文档，以及 OPF 书名改写。
//!
//! xochitl 的网页上传接口有约 100MB 的硬上限（超了直接断连，2026-09-19 真机实测）。绕开办法（2026-09-20
//! 真机验证过 PDF 154MB/349 页、EPUB 153MB 都能打开）：先用网页上传一个几 KB 的**占位文档**让 xochitl
//! 建好条目，再由设备上的 book-serve 把磁盘上那个文件原子替换成真文件。
//!
//! 占位必须带**真书的书名和封面**：xochitl 用占位 EPUB 的 `dc:title` 当显示名、在导入时生成 `cover.png`，
//! 替换成大文件后它不会补生成封面、也不会改名（真机踩过：显示成"上传限制实验-EPUB"且没封面）。
//! PDF 的显示名取上传文件名，缩略图打开时才按页生成，占位不需要带内容。

use regex::Regex;
use std::path::Path;
use std::sync::OnceLock;

fn re(cell: &'static OnceLock<Regex>, pat: &str) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).unwrap())
}

/// 改写 OPF 里的 `<dc:title>`（没有就原样返回）。
pub fn set_opf_title(opf: &str, title: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = re(&R, r#"(?s)(<dc:title\b[^>]*>).*?(</dc:title>)"#);
    let esc = crate::util::xml_escape(title);
    r.replace(opf, |c: &regex::Captures| format!("{}{}{}", &c[1], esc, &c[2])).into_owned()
}

type Zip = zip::ZipArchive<std::io::BufReader<std::fs::File>>;

/// 读条目全部字节；不存在或读失败都是 `None`（占位/封面都是尽力而为）。
fn read_entry(zip: &mut Zip, name: &str) -> Option<Vec<u8>> {
    crate::epubzip::read_by_name_opt(zip, name).ok().flatten()
}

/// 条目按文本读（非 UTF-8 字节按 lossy 替换）。
fn read_text(zip: &mut Zip, name: &str) -> Option<String> {
    read_entry(zip, name).map(|b| String::from_utf8(b).unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned()))
}

/// 打开 EPUB 并读出 OPF：`(zip, OPF 在 zip 里的路径, OPF 文本)`。只读 container.xml 和 OPF 两个条目，不解压整本
/// （`cover_image_of`/`epub_is_rtl`/`epub_placeholder` 三处共用，此前各抄一份）。
pub(crate) fn open_opf(epub: &Path) -> Result<(Zip, String, String), String> {
    let file = std::fs::File::open(epub).map_err(|e| format!("打开 {} 失败: {e}", epub.display()))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| format!("解 EPUB 失败: {e}"))?;
    let container = read_text(&mut zip, "META-INF/container.xml").ok_or("缺 META-INF/container.xml")?;
    let opf_path = crate::wash::tag_attr(&container, "full-path").ok_or("container.xml 里没有 full-path")?.to_string();
    let opf = read_text(&mut zip, &opf_path).ok_or("读不到 OPF")?;
    Ok((zip, opf_path, opf))
}

/// 从真 EPUB 里找封面图：OPF `<meta name="cover">` → manifest；`properties="cover-image"`；都没有就取
/// 第一个 spine 页里的第一张 `<img>`。只读需要的几个条目，不解压整本。
fn find_cover(zip: &mut Zip, opf_path: &str, opf: &str) -> Option<(String, Vec<u8>)> {
    let dir = crate::epubzip::dir_of(opf_path);
    let items = crate::wash::manifest_items(opf);
    let mut candidate: Option<&str> = None;
    if let Some(id) = crate::wash::cover_meta_re().find(opf).and_then(|m| crate::wash::tag_attr(m.as_str(), "content")) {
        candidate = items.iter().find(|i| i.id == id).map(|i| i.href);
    }
    if candidate.is_none() {
        candidate = items.iter().find(|i| i.properties.contains("cover-image") || i.media_type.contains("cover-image")).map(|i| i.href);
    }
    // 声明必须真指向图片：Calibre 产物常见 `<meta name="cover" content="cover.txt"/>` 指向 txt，直接拿来当封面
    // 会得到一个不是图片的"封面"，xochitl 取不到封面缩略图（2026-09-20 真机日志 `null cover image`）。
    if let Some(href) = candidate.filter(|h| crate::util::is_image_ext(h)) {
        let path = crate::epubzip::resolve(dir, &crate::epubzip::percent_decode(href));
        return Some((crate::util::image_ext_of(&path), read_entry(zip, &path)?));
    }
    // 第一个 spine 页里的第一张图。
    static SPINE: OnceLock<Regex> = OnceLock::new();
    let first_ref = re(&SPINE, r#"<itemref\b[^>]*\bidref="([^"]+)""#).captures(opf)?;
    let first = items.iter().find(|i| i.id == &first_ref[1])?;
    let page = crate::epubzip::resolve(dir, &crate::epubzip::percent_decode(first.href));
    let html = read_text(zip, &page)?;
    static IMG: OnceLock<Regex> = OnceLock::new();
    let c = re(&IMG, r#"(?is)<(?:img|image)\b[^>]*?(?:src|xlink:href|href)\s*=\s*"([^"]+)""#).captures(&html)?;
    let path = crate::epubzip::resolve(crate::epubzip::dir_of(&page), &crate::epubzip::percent_decode(&c[1]));
    Some((crate::util::image_ext_of(&path), read_entry(zip, &path)?))
}

/// 读出一本 EPUB 的封面图（扩展名, 字节）：OPF 声明的有效封面，否则第一个 spine 页里的第一张图（同占位构造的规则）。
/// 给"给已有文档补封面缩略图"的小工具用；找不到返回 `None`。
pub fn cover_image_of(epub: &Path) -> Option<(String, Vec<u8>)> {
    let (mut zip, opf_path, opf) = open_opf(epub).ok()?;
    find_cover(&mut zip, &opf_path, &opf)
}

/// 这本 EPUB 是不是"从右往左"翻页：OPF `<spine page-progression-direction="rtl">`（日漫常见）。只读
/// container.xml 和 OPF 两个条目，不解压整本（漫画一卷可达数百 MB）。读不到/不是 EPUB 一律 `false`。
/// 给 xochitl 阅读器的"日漫从右往左翻页"用（book-serve `GET /reading-direction/{uuid}`，2026-09-24）。
/// 判据与写入共用 [`crate::direction`]（2026-09-25 母版库可按书指定方向后收拢到那里）。
pub fn epub_is_rtl(epub: &Path) -> bool {
    crate::direction::spine_direction_file(epub) == Some(crate::direction::PageDirection::Rtl)
}

/// 造占位 EPUB：显示名 = `title`（`None` 取真书自己的 `dc:title`），封面 = 真书的封面（找不到就没有封面页，
/// 只有标题）。体积通常几十到几百 KB。
pub fn epub_placeholder(real_epub: &Path, title: Option<&str>) -> Result<Vec<u8>, String> {
    let (mut zip, opf_path, opf) = open_opf(real_epub)?;
    let cover = find_cover(&mut zip, &opf_path, &opf);
    static TITLE: OnceLock<Regex> = OnceLock::new();
    // OPF 里读出的是转义过的 XML 文本：`plain_text` 还原字符引用后下面统一转义，否则 `A &amp; B` 会被写成 `A &amp;amp; B`（设备显示名带字面 `&amp;`）。
    let from_opf = |c: &regex::Captures| crate::wash::plain_text(&c[1]).trim().to_string();
    let real_title = re(&TITLE, r#"(?s)<dc:title\b[^>]*>(.*?)</dc:title>"#).captures(&opf).map(|c| from_opf(&c)).filter(|t| !t.is_empty());
    let title: String = title.map(str::to_string).or(real_title).unwrap_or_else(|| "未命名".into());
    let title = title.as_str();
    static CREATOR: OnceLock<Regex> = OnceLock::new();
    // 作者/语言同样按"读出→还原→转义"处理：此前原样拼进占位 OPF，原书里若是 CDATA、嵌套标签或非法字符，占位 OPF 就不是
    // 合法 XML（xochitl 严格解析，占位导入失败）。
    let creator = re(&CREATOR, r#"(?s)<dc:creator\b[^>]*>(.*?)</dc:creator>"#).captures(&opf).map(|c| from_opf(&c)).unwrap_or_default();
    static LANG: OnceLock<Regex> = OnceLock::new();
    let lang = re(&LANG, r#"(?s)<dc:language\b[^>]*>(.*?)</dc:language>"#).captures(&opf).map(|c| from_opf(&c)).filter(|l| !l.is_empty()).unwrap_or_else(|| "zh".into());
    let (creator, lang) = (crate::util::xml_escape(&creator), crate::util::xml_escape(&lang));

    let t = crate::util::xml_escape(title);
    let (cover_item, cover_meta, page_body) = match &cover {
        Some((ext, _)) => (
            format!(r#"<item id="cover-img" href="cover.{ext}" media-type="{}" properties="cover-image"/>"#, crate::util::image_media_type_of_ext(ext)),
            r#"<meta name="cover" content="cover-img"/>"#.to_string(),
            format!(r#"<div><img src="cover.{ext}" alt="cover"/></div>"#),
        ),
        None => (String::new(), String::new(), format!("<p>{t}</p>")),
    };
    let creator_xml = if creator.is_empty() { String::new() } else { format!("<dc:creator>{creator}</dc:creator>") };
    let opf_out = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><package xmlns="http://www.idpf.org/2007/opf" version="2.0" unique-identifier="id"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf"><dc:title>{t}</dc:title>{creator_xml}<dc:language>{lang}</dc:language><dc:identifier id="id">urn:cangjie:placeholder:{}</dc:identifier>{cover_meta}</metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>{cover_item}</manifest><spine toc="ncx"><itemref idref="c1"/></spine></package>"#,
        title.len()
    );
    let ncx = format!(
        r#"<?xml version="1.0"?><ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head/><docTitle><text>{t}</text></docTitle><navMap><navPoint id="n1" playOrder="1"><navLabel><text>{t}</text></navLabel><content src="c1.xhtml"/></navPoint></navMap></ncx>"#
    );
    let page = format!(r#"<?xml version="1.0" encoding="utf-8"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>{t}</title></head><body>{page_body}</body></html>"#);

    let mut buf = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let (stored, deflated) = (crate::epubzip::stored(), crate::epubzip::deflated());
        let mut put = |name: &str, data: &[u8], o| crate::epubzip::put_entry(&mut z, name, o, data);
        put("mimetype", b"application/epub+zip", stored)?;
        put("META-INF/container.xml", br#"<?xml version="1.0"?><container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>"#, deflated)?;
        put("content.opf", opf_out.as_bytes(), deflated)?;
        put("toc.ncx", ncx.as_bytes(), deflated)?;
        put("c1.xhtml", page.as_bytes(), deflated)?;
        if let Some((ext, data)) = &cover {
            put(&format!("cover.{ext}"), data, stored)?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

/// 造占位 PDF：一页空白（设备页面尺寸）。显示名取上传文件名，缩略图打开时才按页生成，所以不用带内容。
pub fn pdf_placeholder() -> Result<Vec<u8>, String> {
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma8(image::GrayImage::from_pixel(8, 14, image::Luma([255])))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|e| e.to_string())?;
    let img = crate::convert::pdfwrite::image_from_bytes(&png)?;
    crate::convert::pdfwrite::images_to_pdf_with_toc(&[img], &[])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::io::Write;

    fn real_epub(dir: &Path, cover_meta: bool) -> std::path::PathBuf {
        let p = dir.join("real.epub");
        let f = std::fs::File::create(&p).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("mimetype", o).unwrap();
        z.write_all(b"application/epub+zip").unwrap();
        z.start_file("META-INF/container.xml", o).unwrap();
        z.write_all(br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#).unwrap();
        let meta = if cover_meta { r#"<meta name="cover" content="cv"/>"# } else { "" };
        z.start_file("OEBPS/content.opf", o).unwrap();
        z.write_all(format!(r#"<package><metadata><dc:title>Unknown</dc:title><dc:creator>许先哲</dc:creator><dc:language>zh-CN</dc:language>{meta}</metadata><manifest><item id="cv" href="images/cv.jpg" media-type="image/jpeg"/><item id="c1" href="Text/p1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#).as_bytes()).unwrap();
        z.start_file("OEBPS/Text/p1.xhtml", o).unwrap();
        z.write_all(br#"<html><body><img src="../images/cv.jpg"/></body></html>"#).unwrap();
        z.start_file("OEBPS/images/cv.jpg", o).unwrap();
        z.write_all(b"\xFF\xD8COVERBYTES\xFF\xD9").unwrap();
        z.finish().unwrap();
        p
    }

    fn entries_of(bytes: &[u8]) -> std::collections::HashMap<String, Vec<u8>> {
        let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        (0..z.len()).map(|i| { let mut f = z.by_index(i).unwrap(); let mut v = Vec::new(); f.read_to_end(&mut v).unwrap(); (f.name().to_string(), v) }).collect()
    }

    #[test]
    fn placeholder_carries_real_title_author_and_cover_via_meta() {
        let d = tempfile::tempdir().unwrap();
        let out = epub_placeholder(&real_epub(d.path(), true), Some("鏢人 - 02卷")).unwrap();
        let e = entries_of(&out);
        let opf = String::from_utf8_lossy(&e["content.opf"]).to_string();
        assert!(opf.contains("<dc:title>鏢人 - 02卷</dc:title>") && opf.contains("<dc:creator>许先哲</dc:creator>") && opf.contains(r#"properties="cover-image""#));
        assert_eq!(e["cover.jpg"], b"\xFF\xD8COVERBYTES\xFF\xD9", "封面字节必须是真书的封面");
        assert!(out.len() < 5000, "占位应很小: {}", out.len());
        assert!(crate::check::check_epub(&out, true).unwrap().ok, "占位本身要是合法 EPUB（有目录）");
    }

    #[test]
    fn cover_falls_back_to_first_image_of_first_spine_page() {
        let d = tempfile::tempdir().unwrap();
        let out = epub_placeholder(&real_epub(d.path(), false), Some("书")).unwrap();
        assert_eq!(entries_of(&out)["cover.jpg"], b"\xFF\xD8COVERBYTES\xFF\xD9");
    }

    #[test]
    fn placeholder_defaults_to_real_dc_title_when_none_given() {
        let d = tempfile::tempdir().unwrap();
        let out = epub_placeholder(&real_epub(d.path(), true), None).unwrap();
        assert!(String::from_utf8_lossy(&entries_of(&out)["content.opf"]).contains("<dc:title>Unknown</dc:title>"));
    }

    #[test]
    fn cover_declared_as_non_image_falls_back_to_first_page_image() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("calibre.epub");
        let f = std::fs::File::create(&p).unwrap();
        let mut z = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("mimetype", o).unwrap();
        z.write_all(b"application/epub+zip").unwrap();
        z.start_file("META-INF/container.xml", o).unwrap();
        z.write_all(br#"<container><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#).unwrap();
        z.start_file("content.opf", o).unwrap();
        z.write_all(br#"<package><metadata><dc:title>t</dc:title><meta name="cover" content="cover.txt"/></metadata><manifest><item id="cover.txt" href="cover.txt" media-type="text/plain"/><item id="c1" href="p1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#).unwrap();
        z.start_file("cover.txt", o).unwrap();
        z.write_all(b"not an image").unwrap();
        z.start_file("p1.xhtml", o).unwrap();
        z.write_all(br#"<html><body><img src="real.jpg"/></body></html>"#).unwrap();
        z.start_file("real.jpg", o).unwrap();
        z.write_all(b"\xFF\xD8REALCOVER\xFF\xD9").unwrap();
        z.finish().unwrap();
        let out = epub_placeholder(&p, Some("t")).unwrap();
        let e = entries_of(&out);
        assert_eq!(e["cover.jpg"], b"\xFF\xD8REALCOVER\xFF\xD9", "必须回退到第一页的真图片，而不是 txt");
        assert!(!e.contains_key("cover.txt"));
    }

    /// 书名/作者带 `&` 等：OPF 里是 `&amp;`，占位里必须还是 `&amp;`（此前书名被写成 `&amp;amp;`，设备显示名多出字面 `&amp;`）。
    #[test]
    fn placeholder_does_not_double_escape_title_or_creator() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("amp.epub");
        let mut z = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
        let o = zip::write::SimpleFileOptions::default();
        z.start_file("META-INF/container.xml", o).unwrap();
        z.write_all(br#"<container><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#).unwrap();
        z.start_file("content.opf", o).unwrap();
        z.write_all(br#"<package><metadata><dc:title>Tom &amp; Jerry</dc:title><dc:creator>A &lt;B&gt; &amp; C</dc:creator></metadata><manifest><item id="c1" href="p1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#).unwrap();
        z.start_file("p1.xhtml", o).unwrap();
        z.write_all(b"<html><body><p>x</p></body></html>").unwrap();
        z.finish().unwrap();
        let out = epub_placeholder(&p, None).unwrap();
        let opf = String::from_utf8(entries_of(&out)["content.opf"].clone()).unwrap();
        assert!(opf.contains("<dc:title>Tom &amp; Jerry</dc:title>"), "{opf}");
        assert!(opf.contains("<dc:creator>A &lt;B&gt; &amp; C</dc:creator>"), "{opf}");
        assert!(crate::check::check_epub(&out, true).unwrap().ok);
    }

    #[test]
    fn set_opf_title_rewrites_and_escapes() {
        let opf = r#"<metadata><dc:title id="t">Unknown</dc:title></metadata>"#;
        assert_eq!(set_opf_title(opf, "A & B - 2卷"), r#"<metadata><dc:title id="t">A &amp; B - 2卷</dc:title></metadata>"#);
        assert_eq!(set_opf_title("<metadata/>", "x"), "<metadata/>");
    }

    #[test]
    fn pdf_placeholder_is_one_valid_page() {
        let pdf = pdf_placeholder().unwrap();
        assert!(pdf.starts_with(b"%PDF"));
        assert_eq!(crate::convert::pdfwrite::page_count(&pdf).unwrap(), 1);
        assert!(pdf.len() < 4000);
    }

    #[test]
    fn epub_is_rtl_reads_spine_direction_only() {
        let d = tempfile::tempdir().unwrap();
        let mk = |name: &str, spine: &str| {
            let p = d.path().join(name);
            let mut z = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
            let o: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
            z.start_file("META-INF/container.xml", o).unwrap();
            z.write_all(br#"<container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#).unwrap();
            z.start_file("OEBPS/content.opf", o).unwrap();
            z.write_all(format!(r#"<package><manifest/>{spine}</package>"#).as_bytes()).unwrap();
            z.finish().unwrap();
            p
        };
        assert!(epub_is_rtl(&mk("r.epub", r#"<spine toc="ncx" page-progression-direction="rtl"><itemref idref="a"/></spine>"#)));
        assert!(!epub_is_rtl(&mk("l.epub", r#"<spine page-progression-direction="ltr"><itemref idref="a"/></spine>"#)));
        assert!(!epub_is_rtl(&mk("n.epub", r#"<spine toc="ncx"><itemref idref="a"/></spine>"#)));
        assert!(!epub_is_rtl(&d.path().join("missing.epub")));
        std::fs::write(d.path().join("bad.epub"), b"not a zip").unwrap();
        assert!(!epub_is_rtl(&d.path().join("bad.epub")));
    }
}
