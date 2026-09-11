//! e-ink 提对比与字体锁：内联/独立 css 里的灰字→纯黑、细字重→400；`strip_font_locks` 剥字号/字体锁。
use super::*;

// ===== 字体解锁 =====
pub(super) fn style_attr_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)\s*style="([^"]*)""#).unwrap())
}
pub(super) fn font_decl_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // font-family / font-size / font 简写声明（连同其后分号一并吃掉）。
    // 值里可能含 HTML 实体如 &#39;（内含分号），故值用 `实体 | 非分号字符` 序列匹配，
    // 避免在实体的分号处提前截断。
    R.get_or_init(|| Regex::new(r#"(?i)font(?:-family|-size)?\s*:(?:&#?\w+;|[^;"])*;?"#).unwrap())
}

/// 剥掉内联 style 里的 font-family/font-size(及 font 简写)声明——第三方 EPUB 常内联硬写死
/// 字体/字号，覆盖掉 xochitl 的阅读设置致"改不动字体"。删这些声明后 xochitl 设置即生效。
/// style 因此清空则连整个 style 属性一并删掉；其他声明(颜色/对齐/缩进等)原样保留。
pub fn strip_font_locks(html: &str) -> String {
    style_attr_re()
        .replace_all(html, |c: &regex::Captures| {
            let inner = c.get(1).unwrap().as_str();
            let cleaned = font_decl_re().replace_all(inner, "");
            let cleaned = cleaned.trim();
            if cleaned.is_empty() {
                String::new() // 整个 style 属性删掉(连前导空格)
            } else {
                format!(" style=\"{cleaned}\"")
            }
        })
        .into_owned()
}

// ── ② 按 e-ink 特性提正文对比（设备优化）───────────────────────────────────────────────
// reMarkable Move 是 Gallery 3 彩色墨水屏：灰字发虚、细字重笔画消失（白皮书 §9.2「正文必须纯黑」）。
// 大量第三方 EPUB 的 CSS 把正文设成灰色(color:#333)或细体(font-weight:300)，在这块屏上糊成一片。
// 优化器把**灰色文字**强制纯黑、**细字重**提到 400——只碰 `color`/`font-weight`，不碰彩色文字
// （Gallery 3 有色能显）、不碰 background/border-color。作用域：style 属性 + <style> 块 + .css 文件。

/// 解析 CSS 颜色值 → (r,g,b)。支持 #rgb/#rrggbb、rgb()/rgba()（含 % 则放弃）、常见灰系命名色。
/// 其余（彩色命名/关键字/currentColor 等）返回 None（不动）。
pub(super) fn parse_css_color(value: &str) -> Option<(u8, u8, u8)> {
    let v = value.trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        let h = hex.trim();
        if h.len() == 3 && h.chars().all(|c| c.is_ascii_hexdigit()) {
            let d = |c: char| u8::from_str_radix(&c.to_string(), 16).unwrap() * 17;
            let mut it = h.chars();
            return Some((d(it.next()?), d(it.next()?), d(it.next()?)));
        }
        if h.len() == 6 && h.chars().all(|c| c.is_ascii_hexdigit()) {
            let p = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).unwrap();
            return Some((p(0), p(2), p(4)));
        }
        return None;
    }
    if let Some(inner) = v.strip_prefix("rgb(").or_else(|| v.strip_prefix("rgba(")) {
        let inner = inner.trim_end_matches(')');
        if inner.contains('%') {
            return None; // 百分比形式少见，稳妥不碰
        }
        let nums: Vec<u8> =
            inner.split(',').take(3).filter_map(|p| p.trim().parse::<u8>().ok()).collect();
        if nums.len() == 3 {
            return Some((nums[0], nums[1], nums[2]));
        }
        return None;
    }
    // 灰系命名色（映射到中值即可，只用于走 achromatic_dark 判据）
    let g = |x: u8| Some((x, x, x));
    match v.as_str() {
        "gray" | "grey" => g(128),
        "dimgray" | "dimgrey" => g(105),
        "darkgray" | "darkgrey" => g(169),
        "lightgray" | "lightgrey" => g(211),
        "silver" => g(192),
        "gainsboro" => g(220),
        "slategray" | "slategrey" => g(112),
        "darkslategray" | "darkslategrey" => g(47),
        "lightslategray" | "lightslategrey" => g(119),
        _ => None,
    }
}

/// 是否是"暗到中"的无彩色（灰）——R≈G≈B 且不是纯黑、也不是近白。这类文字在 e-ink 上发虚，强制纯黑。
/// 近白(≥240)排除：可能是浅色背景/有意的白底反白，压黑会毁掉白字设计。
pub fn achromatic_dark(r: u8, g: u8, b: u8) -> bool {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    mx.saturating_sub(mn) <= 24 && (1..=239).contains(&mx)
}

/// font-weight 值是否"细到该提"（<400 或 lighter）——e-ink 上细笔画消失，提到 400 常规体。
pub(super) fn is_thin_weight(value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    if v == "lighter" {
        return true;
    }
    v.parse::<u32>().map(|n| n < 400).unwrap_or(false)
}

/// 对一段 CSS 声明文本：灰色 `color` → `#000000`、细 `font-weight` → `400`。只认属性名本身
/// （`color` 前必须是 起始/空白/`;`/`{`/引号，从而排除 background-color/border-color 等 `-color`）。
pub(super) fn darken_css_decls(css: &str) -> String {
    use std::sync::OnceLock;
    static COLOR_RE: OnceLock<Regex> = OnceLock::new();
    static WEIGHT_RE: OnceLock<Regex> = OnceLock::new();
    let color_re = COLOR_RE
        .get_or_init(|| Regex::new(r#"(?i)(^|[\s;{"'])color(\s*:\s*)([^;}"']+)"#).unwrap());
    let weight_re = WEIGHT_RE
        .get_or_init(|| Regex::new(r#"(?i)(^|[\s;{"'])font-weight(\s*:\s*)([^;}"']+)"#).unwrap());
    let s = color_re.replace_all(css, |c: &regex::Captures| {
        let (lead, colon, val) = (&c[1], &c[2], &c[3]);
        match parse_css_color(val) {
            Some((r, g, b)) if achromatic_dark(r, g, b) => format!("{lead}color{colon}#000000"),
            _ => c[0].to_string(),
        }
    });
    weight_re
        .replace_all(&s, |c: &regex::Captures| {
            let (lead, colon, val) = (&c[1], &c[2], &c[3]);
            if is_thin_weight(val) {
                format!("{lead}font-weight{colon}400")
            } else {
                c[0].to_string()
            }
        })
        .into_owned()
}

/// 对 (x)html：把 `style="..."` 属性与 `<style>…</style>` 块里的灰字/细字重按 e-ink 提对比。
pub fn boost_text_contrast(html: &str) -> String {
    use std::sync::OnceLock;
    static ATTR_RE: OnceLock<Regex> = OnceLock::new();
    static BLOCK_RE: OnceLock<Regex> = OnceLock::new();
    let attr_re = ATTR_RE.get_or_init(|| Regex::new(r#"(?i)style="([^"]*)""#).unwrap());
    let block_re = BLOCK_RE.get_or_init(|| Regex::new(r#"(?is)(<style\b[^>]*>)(.*?)(</style>)"#).unwrap());
    let s = attr_re.replace_all(html, |c: &regex::Captures| {
        format!(r#"style="{}""#, darken_css_decls(&c[1]))
    });
    block_re
        .replace_all(&s, |c: &regex::Captures| format!("{}{}{}", &c[1], darken_css_decls(&c[2]), &c[3]))
        .into_owned()
}

/// 对独立 `.css` 文件：整文件按 e-ink 提对比（灰字→纯黑、细字重→400）。
pub fn boost_contrast_css(css: &str) -> String {
    darken_css_decls(css)
}

#[cfg(test)]
mod font_lock_tests {
    use super::strip_font_locks;
    #[test]
    fn strips_full_font_style() {
        // 导入赎罪真实形态：整个 style 都是 font → 连 style 属性一起删
        let html = r#"<p style="font-size:16px;font-family:&#39;PingFang SC&#39;;">正文</p>"#;
        assert_eq!(strip_font_locks(html), "<p>正文</p>");
    }
    #[test]
    fn keeps_non_font_decls() {
        let html = r#"<p style="color:red;font-size:16px;text-align:center;">x</p>"#;
        let out = strip_font_locks(html);
        assert!(!out.contains("font-size"), "font-size 未删: {out}");
        assert!(out.contains("color:red"), "color 被误删: {out}");
        assert!(out.contains("text-align:center"), "text-align 被误删: {out}");
    }
    #[test]
    fn no_style_untouched() {
        assert_eq!(strip_font_locks("<p>纯文本</p>"), "<p>纯文本</p>");
    }
}

#[cfg(test)]
mod contrast_tests {
    use super::*;

    #[test]
    fn gray_hex_and_named_forced_black_color_only() {
        // 灰字→黑；彩色字不动；background/border-color 不碰；纯黑/近白不动。
        let css = "p{color:#333;background-color:#eee}a{color:red}h1{color:gray}\
                   .x{color:#000;border-color:#888}.w{color:#f5f5f5}";
        let out = darken_css_decls(css);
        assert!(out.contains("p{color:#000000;background-color:#eee}"), "灰字→黑、bg不碰: {out}");
        assert!(out.contains("a{color:red}"), "彩色字不动: {out}");
        assert!(out.contains("h1{color:#000000}"), "命名灰→黑: {out}");
        assert!(out.contains(".x{color:#000;border-color:#888}"), "纯黑不动、border-color不碰: {out}");
        assert!(out.contains(".w{color:#f5f5f5}"), "近白不动: {out}");
    }

    #[test]
    fn thin_weight_bumped_to_400_keep_bold() {
        let css = "a{font-weight:300}b{font-weight:lighter}c{font-weight:700}d{font-weight:normal}";
        let out = darken_css_decls(css);
        assert!(out.contains("a{font-weight:400}"), "300→400: {out}");
        assert!(out.contains("b{font-weight:400}"), "lighter→400: {out}");
        assert!(out.contains("c{font-weight:700}"), "bold 保留: {out}");
        assert!(out.contains("d{font-weight:normal}"), "normal 保留: {out}");
    }

    #[test]
    fn boost_html_handles_style_attr_and_block() {
        let html = r#"<style>.n{color:#444}</style><p style="color:#666;font-weight:200">文</p>"#;
        let out = boost_text_contrast(html);
        assert!(out.contains("<style>.n{color:#000000}</style>"), "<style>块: {out}");
        assert!(out.contains(r#"style="color:#000000;font-weight:400""#), "style属性: {out}");
    }

    #[test]
    fn rgb_gray_forced_colored_rgb_kept() {
        let css = "p{color:rgb(80,80,80)}q{color:rgb(200,20,20)}";
        let out = darken_css_decls(css);
        assert!(out.contains("p{color:#000000}"), "rgb 灰→黑: {out}");
        assert!(out.contains("q{color:rgb(200,20,20)}"), "rgb 彩色不动: {out}");
    }
}
