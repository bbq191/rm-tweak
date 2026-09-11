//! 全书正文统计：字符数 / 主语言 / 书名。用途：投原生后的**渲染页数自检**（book-serve）——xochitl 导入 EPUB
//! 渲染完会在 `<uuid>.content` 写 `pageCount`，拿它与"正文字符数 ÷ 每页字符数"的期望比，远低于期望＝整章渲染失败
//! （同一标签双 id 等，《消失的爱人》只 7 页那种）的症状，不用等用户翻到才发现。
//! 每页字符数真机标定（3.28.0.172，缺省字号 / 边距 56 / 行距 100，2026-09-06）：
//! 《人骨拼圖》241 444 字 → 523 页 ≈ 462 字/页；《Tell Me Your Dreams》339 052 字符 → 352 页 ≈ 963 字符/页。
//! 自检在导入当下跑，xochitl 用缺省字号/边距渲染，页数只随文字密度浮动（真书 0.99、随机词探针 0.86）；阈值见
//! book-serve `render_check::WARN_RATIO`（50%）。
use crate::epubzip::is_html;
use crate::wash::{is_toc_file, plain_text, LangMode};
use regex::Regex;
use std::io::{Read, Seek};
use std::path::Path;
use std::sync::OnceLock;
use zip::ZipArchive;

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

/// OPF/HTML 正文条目名判据——在解压前就跳过图片等无关条目（内存版与文件版共用 [`text_profile_zip`]）。
fn wants_entry(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".opf") || (is_html(name) && !is_toc_file(name))
}

/// 单个条目（OPF 或正文 HTML）累加进统计。
fn accumulate(p: &mut TextProfile, name: &str, data: &[u8]) {
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    static TITLE: OnceLock<Regex> = OnceLock::new();
    let block = BLOCK.get_or_init(|| Regex::new(r#"(?is)<(script|style)[^>]*>.*?</(script|style)>"#).unwrap());
    let title_re = TITLE.get_or_init(|| Regex::new(r#"(?s)<dc:title[^>]*>(.*?)</dc:title>"#).unwrap());
    let Ok(t) = std::str::from_utf8(data) else { return };
    if name.to_ascii_lowercase().ends_with(".opf") {
        if p.title.is_none() {
            // `plain_text` 已还原字符引用：这个书名要跟 xochitl 的 visibleName（已解码的 `A & B`）比对（book-serve `render_check::pick`）。
            p.title = title_re.captures(t).map(|c| plain_text(&c[1])).filter(|s| !s.is_empty());
        }
        return;
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

/// 解 EPUB 统计正文。非 zip / 无正文都按 Err 报，调用方决定要不要自检。整本已经在内存里时用这个；
/// 只有磁盘路径、不想先把整本读进 `Vec<u8>` 用 [`text_profile_file`]。
pub fn text_profile(epub: &[u8]) -> Result<TextProfile, String> {
    text_profile_zip(std::io::Cursor::new(epub))
}

/// [`text_profile`] 的文件版：直接开文件当 zip 按条目遍历，从不要求整本先进内存（落库自检用，2026-09-19 OOM 审计）。
pub fn text_profile_file(path: &Path) -> Result<TextProfile, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    text_profile_zip(std::io::BufReader::new(file))
}

/// 两个入口的共同实现：按条目遍历，**图片等非 OPF/HTML 条目连解压都不做**（`by_index` 先看条目名，值得要的条目
/// 才 `read_to_end`）。此前内存版走 `read_entries` 把全部条目（含图片）解压一遍再筛，两版各写一套循环。
fn text_profile_zip<R: Read + Seek>(reader: R) -> Result<TextProfile, String> {
    let mut archive = ZipArchive::new(reader).map_err(|e| format!("解 EPUB(非 zip?): {e}"))?;
    let mut p = TextProfile::default();
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| format!("读 EPUB 条目 {i}: {e}"))?;
        if f.is_dir() || !wants_entry(f.name()) {
            continue;
        }
        let name = f.name().to_string();
        let size = f.size();
        let data = crate::epubzip::read_all(&mut f, size)?;
        accumulate(&mut p, &name, &data);
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
    fn title_char_refs_are_decoded_to_match_visible_name() {
        let b = epub(&[("content.opf", "<package><metadata><dc:title>Tom &amp; Jerry &#20013;</dc:title></metadata></package>")]);
        assert_eq!(text_profile(&b).unwrap().title.as_deref(), Some("Tom & Jerry 中"));
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

    /// 2026-09-19 OOM 审计：`text_profile_file`（流式，不整本读进内存）跟 `text_profile`（内存版）
    /// 必须给出完全一致的结果——差分测试，不能只测其中一个就当两条路径等价。
    #[test]
    fn text_profile_file_matches_in_memory_text_profile() {
        let han = "汉".repeat(500);
        let b = epub(&[
            ("content.opf", "<package><metadata><dc:title>飘 <i>上册</i></dc:title></metadata></package>"),
            ("c1.xhtml", &format!("<html><body><p>{han}</p></body></html>")),
            ("c2.xhtml", "<html><body><p>Hello world, second chapter</p></body></html>"),
            ("nav.xhtml", "<html><body><nav><p>目录目录目录</p></nav></body></html>"),
            // 非文本条目：两个入口都应该跳过、不计入统计。
            ("images/cover.jpg", "假装是二进制图片数据，反正不是 HTML/OPF 就不该被计入统计"),
        ]);
        let t = tempfile::tempdir().unwrap();
        let path = t.path().join("book.epub");
        std::fs::write(&path, &b).unwrap();
        let mem = text_profile(&b).unwrap();
        let file = text_profile_file(&path).unwrap();
        assert_eq!(mem, file);
        assert_eq!(file.title.as_deref(), Some("飘 上册"));
        assert!(file.chars > 0);
        assert!(text_profile_file(&t.path().join("no-such.epub")).is_err());
    }
}
