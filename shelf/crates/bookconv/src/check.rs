//! EPUB 质量门（对标 host `check_output.py` 的 EPUB 检查项，2026-09-03 移植；白皮书 §03i）。
//! 只读、不改书。硬失败（`ok=false`）应拦下落库/推送：
//! 1. 真 DRM：`META-INF/encryption.xml` 加密了非字体项（仅字体混淆是合法的，告警不拦）。
//! 2. 目录 href 文件命中率 < 80%（目录指向不存在的文件 = xochitl TOC 面板点不动）。
//! 3. 单标签双 `id=` 属性（非法 XHTML，xochitl 严格 XML 解析整章白屏，《消失的爱人》7 页事故）。
//! 4. 正文资源引用（`<img src>`/`<link href>` 等）命中率 < 80%（2026-09-23 真机坐实：PDF→EPUB
//!    路径拼错多写一层 `../`，图片在包里打包正确、标签也在，但引用指向压缩包里不存在的位置——
//!    xochitl 渲染出来一张图都没有，且此前没有任何检查能发现，只能真机肉眼看出来）。
//!
//! 5. OPF 不是合法 XML（非法控制字符/结构错误；xochitl 严格解析，整本读不出来）。正文章节不合法只告警。
//!
//! 告警（不拦）：无 nav/ncx 或零条目（`require_toc` 时升为失败）；目录锚点丢失（xochitl 退化到文件级跳转）。
//! PDF 门（pymupdf）不移植：PDF 定稿只在 host 产出，门留 host。
use crate::epubzip::{dir_of, is_html_entry, percent_decode, resolve, Entry};
use crate::wash::{count_dup_id_tags, href_re, is_toc_file};
use regex::Regex;
use std::collections::HashMap;

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub ok: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub toc_files: Vec<String>,
    pub toc_entries: usize,
    pub href_file_hit: usize,
    pub frag_hit: usize,
    pub frag_total: usize,
    pub dup_id_tags: usize,
    pub html_files: usize,
    pub resource_refs_total: usize,
    pub resource_refs_hit: usize,
}

impl CheckReport {
    /// 一行摘要（回执用）。
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("目录 {} 条", self.toc_entries)];
        if self.frag_total > 0 && self.frag_hit < self.frag_total {
            parts.push(format!("锚点丢失 {}/{}", self.frag_total - self.frag_hit, self.frag_total));
        }
        if self.resource_refs_total > 0 && self.resource_refs_hit < self.resource_refs_total {
            parts.push(format!("资源引用丢失 {}/{}", self.resource_refs_total - self.resource_refs_hit, self.resource_refs_total));
        }
        if !self.warnings.is_empty() {
            parts.push(format!("告警 {}", self.warnings.len()));
        }
        parts.join("，")
    }
}

// `read_entries` 已迁到 `epubzip`（与 Entry/路径工具同处）；re-export 保住 `check::read_entries` 旧路径。
pub use crate::epubzip::read_entries;

pub fn check_epub(epub: &[u8], require_toc: bool) -> Result<CheckReport, String> {
    Ok(check_entries(&read_entries(epub)?, require_toc))
}

/// [`check_epub`] 的按路径、省内存变体：用 [`crate::epubzip::read_skeleton`]（图片条目留空占位，
/// 只有非图片的真实字节整份读入）而不是 [`read_entries`] 整本读进内存——book-serve 优化产物落盘
/// 后要跑一遍质量门，大漫画整本读回内存会重蹈流式优化本来要避开的 OOM 老路（见 book-serve
/// `Staging::optimize` 头注引用的真机 552MB 全集实测）。质量门这几条规则（双 id/href 命中率/正文
/// 资源引用）都只看 html/toc 文本内容，不看图片字节，省下来的这份内存对检查结果没有任何影响。
pub fn check_epub_file(path: &std::path::Path) -> Result<CheckReport, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开待校验文件失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| format!("解 EPUB(非 zip?): {e}"))?;
    let sk = crate::epubzip::read_skeleton(&mut zip)?;
    Ok(check_entries(&sk.entries, false))
}

pub fn check_entries(entries: &[Entry], require_toc: bool) -> CheckReport {
    let mut rep = CheckReport::default();
    let names: HashMap<&str, &Entry> = entries.iter().map(|e| (e.name.as_str(), e)).collect();

    // 1. DRM
    if let Some(enc) = names.get("META-INF/encryption.xml") {
        let t = String::from_utf8_lossy(&enc.data);
        let targets: Vec<String> = crate::wash::cipher_reference_re().captures_iter(&t).map(|c| c[1].to_string()).collect();
        let non_font: Vec<&String> = targets.iter().filter(|x| { let l = x.to_ascii_lowercase(); !(l.ends_with(".ttf") || l.ends_with(".otf") || l.ends_with(".woff") || l.ends_with(".woff2")) }).collect();
        if !non_font.is_empty() {
            rep.errors.push(format!("加密 EPUB（DRM，加密了 {} 等），xochitl/KOReader 都读不了", non_font.iter().take(3).map(|s| s.as_str()).collect::<Vec<_>>().join("、")));
        } else {
            rep.warnings.push(format!("仅字体混淆（{} 个字体文件，非 DRM，可读）", targets.len()));
        }
    }

    // 2. 目录
    rep.toc_files = entries.iter().filter(|e| is_toc_file(&e.name)).map(|e| e.name.clone()).collect();
    let mut targets: Vec<(String, String)> = Vec::new(); // (zip 路径, frag)
    for tf in &rep.toc_files {
        let base = dir_of(tf);
        let t = String::from_utf8_lossy(&names[tf.as_str()].data);
        for c in href_re().captures_iter(&t) {
            let raw = &c[2];
            if raw.starts_with("http://") || raw.starts_with("https://") {
                continue;
            }
            let frag = c.get(3).map(|m| percent_decode(m.as_str().trim_start_matches('#'))).unwrap_or_default();
            targets.push((resolve(base, &percent_decode(raw)), frag));
        }
    }
    rep.toc_entries = targets.len();
    if rep.toc_files.is_empty() || targets.is_empty() {
        let msg = "无 nav/ncx 或目录零条目".to_string();
        if require_toc {
            rep.errors.push(msg);
        } else {
            rep.warnings.push(msg);
        }
    }
    rep.href_file_hit = targets.iter().filter(|(t, _)| names.contains_key(t.as_str())).count();
    if !targets.is_empty() && (rep.href_file_hit as f64) / (targets.len() as f64) < 0.8 {
        rep.errors.push(format!("目录 href 文件命中率过低 {}/{}", rep.href_file_hit, targets.len()));
    }
    // 每个目标页只扫一遍收集全部 id/name 值，再按集合判命中（此前每个带锚点的目录项各编译一个正则、各扫一遍整页）。
    static ANCHOR: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let anchor_re = ANCHOR.get_or_init(|| Regex::new(r#"(?:id|name)="([^"]*)""#).unwrap());
    let mut cache: HashMap<&str, std::collections::HashSet<String>> = HashMap::new();
    for (t, frag) in targets.iter().filter(|(t, f)| !f.is_empty() && names.contains_key(t.as_str())) {
        rep.frag_total += 1;
        let anchors = cache.entry(t.as_str()).or_insert_with(|| {
            let html = String::from_utf8_lossy(&names[t.as_str()].data);
            anchor_re.captures_iter(&html).map(|c| c[1].to_string()).collect()
        });
        if anchors.contains(frag) {
            rep.frag_hit += 1;
        }
    }
    if rep.frag_total > 0 && rep.frag_hit < rep.frag_total {
        rep.warnings.push(format!("目录锚点丢失 {}/{}（xochitl 退化到文件级跳转）", rep.frag_total - rep.frag_hit, rep.frag_total));
    }

    // 3. 双 id
    for e in entries.iter().filter(|e| is_html_entry(&e.name, &e.data)) {
        rep.html_files += 1;
        rep.dup_id_tags += count_dup_id_tags(&String::from_utf8_lossy(&e.data));
    }
    if rep.dup_id_tags > 0 {
        rep.errors.push(format!("{} 个标签带双 id 属性（非法 XHTML，xochitl 整章白屏）", rep.dup_id_tags));
    }

    // 4. 正文资源引用（img src / link href 等，`href_re` 同一条正则，跟目录用的那节区别只是扫的文件
    // 不是 toc 而是每章正文自己）：跳过远程 URL 与 data: 内联，跳过纯同文件锚点（href_re 的 group2
    // 要求 # 前至少一个字符，`href="#frag"` 天然不落进来）。命中率阈值跟目录那节一致，同一份"到底
    // 该拦还是该忍"判断标准，不搞两套。
    let mut res_examples: Vec<String> = Vec::new();
    for e in entries.iter().filter(|e| is_html_entry(&e.name, &e.data)) {
        let base = dir_of(&e.name);
        let t = String::from_utf8_lossy(&e.data);
        for c in href_re().captures_iter(&t) {
            let raw = &c[2];
            if raw.starts_with("http://") || raw.starts_with("https://") || raw.starts_with("data:") || raw.starts_with("mailto:") {
                continue;
            }
            rep.resource_refs_total += 1;
            let target = resolve(base, &percent_decode(raw));
            if names.contains_key(target.as_str()) {
                rep.resource_refs_hit += 1;
            } else if res_examples.len() < 3 {
                res_examples.push(format!("{}→{target}", e.name));
            }
        }
    }
    if rep.resource_refs_total > 0 && (rep.resource_refs_hit as f64) / (rep.resource_refs_total as f64) < 0.8 {
        rep.errors.push(format!(
            "正文资源引用命中率过低 {}/{}（如 {}），图片/样式在包里但引用路径指向不存在的位置，真机会整个不显示",
            rep.resource_refs_hit,
            rep.resource_refs_total,
            res_examples.join("、")
        ));
    }

    // 5. XML 合法性：xochitl 是严格 XML 解析器。OPF 不合法＝整本书读不出来（2026-09-23 真机《T.E.双语》
    // `dc:title` 混进 `\0`，只渲染出 1 页）→ 硬失败；正文章节不合法＝那一章被吞（《消失的爱人》双 id
    // 先例）→ 告警，不拦——原书本来就这样的话，拦了也换不来更好的结果。
    for e in entries.iter().filter(|e| e.name.ends_with(".opf")) {
        if let Some(why) = xml_problem(&e.data) {
            rep.errors.push(format!("{} 不是合法 XML（{why}），xochitl 会整本读不出来", e.name));
        }
    }
    let bad_chapters: Vec<String> = entries.iter().filter(|e| is_html_entry(&e.name, &e.data)).filter_map(|e| xml_problem(&e.data).map(|why| format!("{}（{why}）", e.name))).collect();
    if !bad_chapters.is_empty() {
        rep.warnings.push(format!("{} 个正文文件不是合法 XML，xochitl 可能整章不显示：{}", bad_chapters.len(), bad_chapters.iter().take(3).cloned().collect::<Vec<_>>().join("、")));
    }

    rep.ok = rep.errors.is_empty();
    rep
}

/// 不是合法 XML 的第一个原因；合法返回 `None`。先查 XML 1.0 不允许的字符（quick-xml 不管字符集，
/// `\0` 之类照单全收），再过一遍 quick-xml 查结构（标签配对等）。
fn xml_problem(data: &[u8]) -> Option<String> {
    let text = match std::str::from_utf8(data) {
        Ok(t) => t,
        Err(e) => return Some(format!("第 {} 字节起不是 UTF-8", e.valid_up_to())),
    };
    if let Some((i, c)) = text.char_indices().find(|(_, c)| !crate::util::is_xml_char(*c)) {
        return Some(format!("第 {} 字节有非法字符 U+{:04X}", i, c as u32));
    }
    let mut r = quick_xml::Reader::from_str(text);
    loop {
        match r.read_event() {
            Ok(quick_xml::events::Event::Eof) => return None,
            Ok(_) => {}
            Err(e) => return Some(format!("第 {} 字节附近：{e}", r.buffer_position())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn e(name: &str, data: &str) -> Entry {
        Entry { name: name.into(), data: data.as_bytes().to_vec() }
    }

    #[test]
    fn gate_rules() {
        // 健康书
        let good = vec![
            e("OEBPS/toc.ncx", r#"<ncx><content src="text/c1.xhtml#a"/><content src="text/c2%20x.xhtml"/></ncx>"#),
            e("OEBPS/text/c1.xhtml", r#"<html><body><h1 id="a">x</h1></body></html>"#),
            e("OEBPS/text/c2 x.xhtml", "<html><body/></html>"),
        ];
        let r = check_entries(&good, true);
        assert!(r.ok, "{:?}", r.errors);
        assert_eq!((r.toc_entries, r.href_file_hit, r.frag_hit, r.frag_total), (2, 2, 1, 1));
        assert_eq!(r.summary(), "目录 2 条");
        // 命中率低 + 锚点丢 + 双 id + DRM + 正文资源引用命中率低（nav.xhtml 本身也是合法 html，它那 4 条
        // href 被目录那节（2）与正文资源引用那节（4）各扫一遍——同一份 href_re 结果，两节各自独立判命中率，
        // 不是重复 bug）。
        let bad = vec![
            e("META-INF/encryption.xml", r#"<CipherReference URI="OEBPS/text/c1.xhtml"/>"#),
            e("OEBPS/nav.xhtml", r#"<html><body><nav><a href="text/c1.xhtml#zz">1</a><a href="text/nope1.xhtml">2</a><a href="text/nope2.xhtml">3</a><a href="text/nope3.xhtml">4</a></nav></body></html>"#),
            e("OEBPS/text/c1.xhtml", r#"<html><body><p id="a" id="b">x</p></body></html>"#),
        ];
        let r = check_entries(&bad, false);
        assert!(!r.ok);
        assert_eq!(r.errors.len(), 4, "{:?}", r.errors);
        assert!(r.errors[0].contains("DRM") && r.errors[1].contains("命中率过低 1/4") && r.errors[2].contains("双 id"));
        assert!(r.errors[3].contains("正文资源引用命中率过低 1/4"), "{:?}", r.errors[3]);
        assert!(r.warnings.iter().any(|w| w.contains("锚点丢失 1/1")));
        // 无目录：告警 / require_toc 失败；字体混淆只告警
        let none = vec![e("META-INF/encryption.xml", r#"<CipherReference URI="f.ttf"/>"#), e("c.xhtml", "<html/>")];
        assert!(check_entries(&none, false).ok);
        let r = check_entries(&none, true);
        assert!(!r.ok && r.warnings.iter().any(|w| w.contains("字体混淆")));
    }

    #[test]
    fn reads_zip() {
        assert!(check_epub(b"notazip", false).is_err());
    }

    /// `check_epub_file` 走 `read_skeleton`（图片留空）而不是整本读入——这条测试确认图片留空不影响
    /// 判断结果：图片条目本身不是 html/toc，判断只看文本条目，空字节不会被误判成"引用了不存在的文件"
    /// （图片条目自己不会被当成正文去扫 href，也不会被当成 img 引用目标之外的东西）。
    #[test]
    fn check_epub_file_skips_image_bytes_but_still_catches_broken_ref() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("book.epub");
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let o = zip::write::SimpleFileOptions::default();
            z.start_file::<_, ()>("OEBPS/chap_0001.xhtml", o).unwrap();
            std::io::Write::write_all(&mut z, br#"<html><body><img src="../images/pic.png"/></body></html>"#).unwrap();
            z.start_file::<_, ()>("OEBPS/images/pic.png", o).unwrap();
            std::io::Write::write_all(&mut z, &[0u8; 5000]).unwrap();
            z.finish().unwrap();
        }
        std::fs::write(&p, &buf).unwrap();
        let rep = check_epub_file(&p).unwrap();
        assert!(!rep.ok, "路径拼错应该被拦下来: {:?}", rep.errors);
        assert_eq!((rep.resource_refs_total, rep.resource_refs_hit), (1, 0));
    }

    /// 复现 2026-09-23 真机 bug 的最小形态：图片确实打包进 EPUB，但 `<img src>` 多写一层 `../`，
    /// 章节文件跟图片其实同级——这条门必须拦下来，不能等真机肉眼才发现。
    #[test]
    fn resource_ref_gate_catches_broken_relative_image_path() {
        let broken = vec![
            e("OEBPS/chap_0001.xhtml", r#"<html><body><p><img src="../images/pic.png"/></p></body></html>"#),
            e("OEBPS/images/pic.png", "fake-png-bytes"),
        ];
        let r = check_entries(&broken, false);
        assert!(!r.ok, "多写一层 ../ 指到包里不存在的位置，这条门应该拦下来");
        assert!(r.errors.iter().any(|w| w.contains("正文资源引用命中率过低")), "{:?}", r.errors);
        assert_eq!((r.resource_refs_total, r.resource_refs_hit), (1, 0));

        let fixed = vec![
            e("OEBPS/chap_0001.xhtml", r#"<html><body><p><img src="images/pic.png"/></p></body></html>"#),
            e("OEBPS/images/pic.png", "fake-png-bytes"),
        ];
        let r2 = check_entries(&fixed, false);
        assert!(r2.ok, "{:?}", r2.errors);
        assert_eq!((r2.resource_refs_total, r2.resource_refs_hit), (1, 1));
    }

    /// 远程图/内联 data: 图不该被当成"引用了包内不存在的文件"误判——这两种压根不指向包内资源。
    #[test]
    fn resource_ref_gate_ignores_remote_and_data_uri_images() {
        let e1 = e("OEBPS/c.xhtml", r#"<html><body><img src="https://example.com/a.png"/><img src="data:image/png;base64,AAAA"/></body></html>"#);
        let r = check_entries(&[e1], false);
        assert!(r.ok, "{:?}", r.errors);
        assert_eq!(r.resource_refs_total, 0, "远程/data: 都不该计入正文资源引用统计");
    }

    #[test]
    fn opf_with_illegal_control_char_is_hard_error_but_bad_chapter_only_warns() {
        let good_opf = "<?xml version=\"1.0\"?><package><metadata><dc:title xmlns:dc=\"x\">T</dc:title></metadata></package>";
        let bad_opf = "<?xml version=\"1.0\"?><package><metadata><dc:title xmlns:dc=\"x\">T\u{0}E</dc:title></metadata></package>";
        let chap = |body: &str| format!("<?xml version=\"1.0\"?><html xmlns=\"http://www.w3.org/1999/xhtml\"><body>{body}</body></html>");
        let mk = |opf: &str, ch: &str| vec![Entry { name: "OEBPS/content.opf".into(), data: opf.as_bytes().to_vec() }, Entry { name: "OEBPS/c.xhtml".into(), data: ch.as_bytes().to_vec() }];

        let rep = check_entries(&mk(bad_opf, &chap("<p>x</p>")), false);
        assert!(!rep.ok && rep.errors.iter().any(|e| e.contains("content.opf") && e.contains("U+0000")), "{:?}", rep.errors);

        let rep = check_entries(&mk(good_opf, &chap("<p>x</div>")), false);
        assert!(rep.errors.iter().all(|e| !e.contains("合法 XML")), "正文不合法不拦: {:?}", rep.errors);
        assert!(rep.warnings.iter().any(|w| w.contains("c.xhtml")), "但要告警: {:?}", rep.warnings);

        let rep = check_entries(&mk(good_opf, &chap("<p>x</p>")), false);
        assert!(rep.errors.iter().chain(rep.warnings.iter()).all(|m| !m.contains("合法 XML")), "{:?} {:?}", rep.errors, rep.warnings);
    }

}
