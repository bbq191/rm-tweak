//! EPUB 漫画识别：给 `optimize_epub_with` 用，决定该书走"漫画路"（画质/裁边保留原图）还是"文字书路"
//! （常规降采样）。判定逻辑当初是从 host 侧 `shelf_cli/comic.py::epub_image_stats()` 移植过来的
//! （2026-09-14 定案的阈值：图 ≥20 张且平均每张图配的文字 <40 字），host 那份原来继续给 `shelf push`
//! 的路由分流（转 CBZ）用——**2026-09-18 host 整条线（含这份 Python 原版）已砍**，不再使用 PC 端，
//! 现在这份 Rust 实现是唯一在用的版本，不用再顾虑"改一边忘改另一边"。

use crate::wash::{parse_opf, Entry};
use regex::Regex;
use std::sync::OnceLock;

/// 判定漫画的最小图片数（跟 comic.py 的 `MIN_PAGES` 同值——历史命名按"页"，这里语义是"图"）。
pub const MIN_IMAGES: usize = 20;
/// 判定漫画的"平均每张图配的文字数"上限。
pub const TEXT_PER_IMAGE: f64 = 40.0;

fn strip_noise_tags(html: &str) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    // regex crate 不支持反向引用，三种标签各写一条 alternation。
    RE.get_or_init(|| Regex::new(r#"(?is)<script\b.*?</script>|<style\b.*?</style>|<head\b.*?</head>"#).unwrap()).replace_all(html, "").into_owned()
}

fn count_images(html: &str) -> usize {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?i)<(?:img|image)\b"#).unwrap()).find_iter(html).count()
}

/// (spine 页里 `<img>`/`<image>` 总数, 可见文字总字数)。沿 OPF spine 遍历——跟 comic.py 同一套算法。
pub fn epub_image_stats(entries: &[Entry]) -> (usize, usize) {
    let Some(opf) = parse_opf(entries) else { return (0, 0) };
    let mut images = 0usize;
    let mut text = 0usize;
    for p in &opf.spine {
        let low = p.to_ascii_lowercase();
        if low.ends_with(".jpg") || low.ends_with(".jpeg") || low.ends_with(".png") || low.ends_with(".gif") || low.ends_with(".webp") {
            images += 1; // 少数畸形 EPUB 把图片文件直接列进 spine
            continue;
        }
        let Some(e) = entries.iter().find(|e| &e.name == p) else { continue };
        let Ok(html) = std::str::from_utf8(&e.data) else { continue };
        images += count_images(html);
        let body = strip_noise_tags(html);
        text += crate::wash::plain_text(&body).chars().filter(|c| !c.is_whitespace()).count();
    }
    (images, text)
}

/// 判定：图 ≥[`MIN_IMAGES`] 张且平均每张图配的文字 <[`TEXT_PER_IMAGE`] 字。
pub fn is_comic(entries: &[Entry]) -> bool {
    let (images, text) = epub_image_stats(entries);
    images >= MIN_IMAGES && (text as f64) < TEXT_PER_IMAGE * images as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, data: &str) -> Entry {
        Entry { name: name.into(), data: data.as_bytes().to_vec() }
    }

    fn opf(items: &str, spine: &str) -> Entry {
        e("content.opf", &format!(r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest>{items}</manifest><spine>{spine}</spine></package>"#))
    }

    #[test]
    fn text_book_with_scattered_illustrations_is_not_comic() {
        let mut v = vec![opf(
            r#"<item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="c1"/>"#,
        )];
        let long_text = "正".repeat(500);
        v.push(e("c1.xhtml", &format!("<html><body><p>{long_text}</p><img src=\"deco.png\"/></body></html>")));
        assert!(!is_comic(&v), "一张插图配几百字，不该判漫画");
    }

    #[test]
    fn image_dense_epub_with_almost_no_text_is_comic() {
        let items: String = (1..=25).map(|i| format!(r#"<item id="c{i}" href="c{i}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
        let spine: String = (1..=25).map(|i| format!(r#"<itemref idref="c{i}"/>"#)).collect();
        let mut v = vec![opf(&items, &spine)];
        for i in 1..=25 {
            v.push(e(&format!("c{i}.xhtml"), &format!(r#"<html><body><img src="p{i}.jpg"/></body></html>"#)));
        }
        assert!(is_comic(&v), "25 张纯图片页、几乎无字，应判漫画");
    }

    #[test]
    fn below_min_images_threshold_is_not_comic() {
        let items: String = (1..=10).map(|i| format!(r#"<item id="c{i}" href="c{i}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
        let spine: String = (1..=10).map(|i| format!(r#"<itemref idref="c{i}"/>"#)).collect();
        let mut v = vec![opf(&items, &spine)];
        for i in 1..=10 {
            v.push(e(&format!("c{i}.xhtml"), &format!(r#"<html><body><img src="p{i}.jpg"/></body></html>"#)));
        }
        assert!(!is_comic(&v), "只有 10 张图，没到 MIN_IMAGES 阈值");
    }

    #[test]
    fn script_and_style_text_excluded_from_count() {
        let mut v = vec![opf(
            r#"<item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/>"#,
            r#"<itemref idref="c1"/>"#,
        )];
        v.push(e("c1.xhtml", r#"<html><head><style>body{color:red}</style><script>var x=1;</script></head><body><img src="p1.jpg"/></body></html>"#));
        let (images, text) = epub_image_stats(&v);
        assert_eq!(images, 1);
        assert_eq!(text, 0, "script/style 内容不该计入可见文字: got {text}");
    }
}
