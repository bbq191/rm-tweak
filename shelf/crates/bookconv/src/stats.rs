//! 全书正文统计：字符数 / 主语言 / 书名。用途：投原生后的**渲染页数自检**（book-serve）——xochitl 导入 EPUB
//! 渲染完会在 `<uuid>.content` 写 `pageCount`，拿它与"正文字符数 ÷ 每页字符数"的期望比，远低于期望＝整章渲染失败
//! （同一标签双 id 等，《消失的爱人》只 7 页那种）的症状，不用等用户翻到才发现。
//! 每页字符数真机标定（3.28.0.172，缺省字号 / 边距 56 / 行距 100，2026-09-06）：
//! 《人骨拼圖》241 444 字 → 523 页 ≈ 462 字/页；《Tell Me Your Dreams》339 052 字符 → 352 页 ≈ 963 字符/页。
//! 自检在导入当下跑，xochitl 用缺省字号/边距渲染，页数只随文字密度浮动（真书 0.99、随机词探针 0.86）；阈值见
//! book-serve `render_check::WARN_RATIO`（50%）。
use crate::check::read_entries;
use crate::wash::{is_html, is_toc_file, plain_text, LangMode};
use regex::Regex;
use std::sync::OnceLock;

pub const CJK_CHARS_PER_PAGE: u64 = 460;
pub const LATIN_CHARS_PER_PAGE: u64 = 960;

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextProfile {
    /// 正文非空白字符总数（去标签、去 script/style、目录页不计）。
    pub chars: u64,
    pub han: u64,
    pub latin: u64,
    /// OPF `dc:title`（xochitl 进库后的 visibleName 来源）。
    pub title: Option<String>,
}

impl TextProfile {
    pub fn lang(&self) -> LangMode {
        if self.han >= self.latin {
            LangMode::Cjk
        } else {
            LangMode::Latin
        }
    }

    /// 缺省阅读设置下的期望页数（向上取整，至少 1）。
    pub fn expected_pages(&self) -> u64 {
        let cpp = match self.lang() {
            LangMode::Cjk => CJK_CHARS_PER_PAGE,
            _ => LATIN_CHARS_PER_PAGE,
        };
        self.chars.div_ceil(cpp).max(1)
    }
}

/// 解 EPUB 统计正文。非 zip / 无正文都按 Err 报，调用方决定要不要自检。
pub fn text_profile(epub: &[u8]) -> Result<TextProfile, String> {
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    static TITLE: OnceLock<Regex> = OnceLock::new();
    let block = BLOCK.get_or_init(|| Regex::new(r#"(?is)<(script|style)[^>]*>.*?</(script|style)>"#).unwrap());
    let title_re = TITLE.get_or_init(|| Regex::new(r#"(?s)<dc:title[^>]*>(.*?)</dc:title>"#).unwrap());
    let entries = read_entries(epub)?;
    let mut p = TextProfile::default();
    for e in &entries {
        let Ok(t) = std::str::from_utf8(&e.data) else { continue };
        if e.name.to_ascii_lowercase().ends_with(".opf") && p.title.is_none() {
            p.title = title_re.captures(t).map(|c| plain_text(&c[1])).filter(|s| !s.is_empty());
            continue;
        }
        if !is_html(&e.name) || is_toc_file(&e.name) {
            continue;
        }
        for ch in plain_text(&block.replace_all(t, "")).chars() {
            if ch.is_whitespace() {
                continue;
            }
            p.chars += 1;
            if matches!(ch, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}') {
                p.han += 1;
            } else if ch.is_ascii_alphabetic() {
                p.latin += 1;
            }
        }
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn epub(files: &[(&str, &str)]) -> Vec<u8> {
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
    fn counts_text_skips_toc_and_style_and_reads_title() {
        let han = "汉".repeat(1000);
        let b = epub(&[
            ("content.opf", "<package><metadata><dc:title>人骨 <i>拼圖</i></dc:title></metadata></package>"),
            ("c1.xhtml", &format!("<html><head><style>p{{x:1}}</style></head><body><p>{han}</p><script>var a=1;</script></body></html>")),
            ("nav.xhtml", "<html><body><nav><p>目录目录目录</p></nav></body></html>"),
        ]);
        let p = text_profile(&b).unwrap();
        assert_eq!((p.chars, p.han, p.latin), (1000, 1000, 0));
        assert_eq!(p.title.as_deref(), Some("人骨 拼圖"));
        assert_eq!(p.lang(), LangMode::Cjk);
        assert_eq!(p.expected_pages(), 3, "1000/460 向上取整");
    }

    #[test]
    fn latin_profile_and_minimum_one_page() {
        let b = epub(&[("a.html", "<p>Hello world</p>")]);
        let p = text_profile(&b).unwrap();
        assert_eq!((p.chars, p.latin, p.han), (10, 10, 0));
        assert_eq!(p.lang(), LangMode::Latin);
        assert_eq!(p.expected_pages(), 1);
        assert!(text_profile(b"not a zip").is_err());
    }
}
