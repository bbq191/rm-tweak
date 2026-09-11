//! EPUB 质量门（对标 host `check_output.py` 的 EPUB 检查项，2026-09-03 移植；白皮书 §03i）。
//! 只读、不改书。硬失败（`ok=false`）应拦下落库/推送：
//! 1. 真 DRM：`META-INF/encryption.xml` 加密了非字体项（仅字体混淆是合法的，告警不拦）。
//! 2. 目录 href 文件命中率 < 80%（目录指向不存在的文件 = xochitl TOC 面板点不动）。
//! 3. 单标签双 `id=` 属性（非法 XHTML，xochitl 严格 XML 解析整章白屏，《消失的爱人》7 页事故）。
//! 告警（不拦）：无 nav/ncx 或零条目（`require_toc` 时升为失败）；目录锚点丢失（xochitl 退化到文件级跳转）。
//! PDF 门（pymupdf）不移植：PDF 定稿只在 host 产出，门留 host。
use crate::wash::{count_dup_id_tags, dir_of, href_re, is_html, is_toc_file, percent_decode, resolve, Entry};
use regex::Regex;
use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::sync::OnceLock;
use zip::ZipArchive;

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
}

impl CheckReport {
    /// 一行摘要（回执用）。
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("目录 {} 条", self.toc_entries)];
        if self.frag_total > 0 && self.frag_hit < self.frag_total {
            parts.push(format!("锚点丢失 {}/{}", self.frag_total - self.frag_hit, self.frag_total));
        }
        if !self.warnings.is_empty() {
            parts.push(format!("告警 {}", self.warnings.len()));
        }
        parts.join("，")
    }
}

/// 从 zip 字节读条目表（目录项剔除）。优化器与质量门共用。
pub fn read_entries(epub: &[u8]) -> Result<Vec<Entry>, String> {
    let mut archive = ZipArchive::new(Cursor::new(epub)).map_err(|e| format!("解 EPUB(非 zip?): {e}"))?;
    let mut out = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| format!("读 EPUB 条目 {i}: {e}"))?;
        if f.is_dir() {
            continue;
        }
        let mut data = Vec::new();
        f.read_to_end(&mut data).map_err(|e| e.to_string())?;
        out.push(Entry { name: f.name().to_string(), data });
    }
    Ok(out)
}

pub fn check_epub(epub: &[u8], require_toc: bool) -> Result<CheckReport, String> {
    Ok(check_entries(&read_entries(epub)?, require_toc))
}

pub fn check_entries(entries: &[Entry], require_toc: bool) -> CheckReport {
    let mut rep = CheckReport::default();
    let names: HashMap<&str, &Entry> = entries.iter().map(|e| (e.name.as_str(), e)).collect();

    // 1. DRM
    if let Some(enc) = names.get("META-INF/encryption.xml") {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r#"CipherReference\s+URI="([^"]+)""#).unwrap());
        let t = String::from_utf8_lossy(&enc.data);
        let targets: Vec<String> = re.captures_iter(&t).map(|c| c[1].to_string()).collect();
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
    let mut cache: HashMap<&str, String> = HashMap::new();
    for (t, frag) in targets.iter().filter(|(t, f)| !f.is_empty() && names.contains_key(t.as_str())) {
        rep.frag_total += 1;
        let html = cache.entry(t.as_str()).or_insert_with(|| String::from_utf8_lossy(&names[t.as_str()].data).into_owned());
        let re = Regex::new(&format!(r#"(?:id|name)="{}""#, regex::escape(frag))).unwrap();
        if re.is_match(html) {
            rep.frag_hit += 1;
        }
    }
    if rep.frag_total > 0 && rep.frag_hit < rep.frag_total {
        rep.warnings.push(format!("目录锚点丢失 {}/{}（xochitl 退化到文件级跳转）", rep.frag_total - rep.frag_hit, rep.frag_total));
    }

    // 3. 双 id
    for e in entries.iter().filter(|e| is_html(&e.name)) {
        rep.html_files += 1;
        rep.dup_id_tags += count_dup_id_tags(&String::from_utf8_lossy(&e.data));
    }
    if rep.dup_id_tags > 0 {
        rep.errors.push(format!("{} 个标签带双 id 属性（非法 XHTML，xochitl 整章白屏）", rep.dup_id_tags));
    }
    rep.ok = rep.errors.is_empty();
    rep
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
        // 命中率低 + 锚点丢 + 双 id + DRM
        let bad = vec![
            e("META-INF/encryption.xml", r#"<CipherReference URI="OEBPS/text/c1.xhtml"/>"#),
            e("OEBPS/nav.xhtml", r#"<html><body><nav><a href="text/c1.xhtml#zz">1</a><a href="text/nope1.xhtml">2</a><a href="text/nope2.xhtml">3</a><a href="text/nope3.xhtml">4</a></nav></body></html>"#),
            e("OEBPS/text/c1.xhtml", r#"<html><body><p id="a" id="b">x</p></body></html>"#),
        ];
        let r = check_entries(&bad, false);
        assert!(!r.ok);
        assert_eq!(r.errors.len(), 3, "{:?}", r.errors);
        assert!(r.errors[0].contains("DRM") && r.errors[1].contains("命中率过低 1/4") && r.errors[2].contains("双 id"));
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
}
