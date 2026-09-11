//! 无效引用清理：指向书内不存在文件的 `<img>` 与字体全缺的 `@font-face`。
use super::*;

// ───────────────────────── 无效引用清理 ─────────────────────────

/// 引用目标是否在书外（远程/内嵌数据/纯锚点）——不是"书内缺文件"，一律不判无效。
pub(super) fn is_external_ref(r: &str) -> bool {
    let l = r.trim().to_ascii_lowercase();
    l.is_empty() || l.starts_with('#') || l.starts_with("data:") || l.starts_with("http:") || l.starts_with("https:") || l.starts_with("//")
}

/// `rel`（相对 `base_dir`）指向的书内文件是否存在。大小写不同也算存在（别的阅读器可能容错，宁可留着）。
pub(super) fn ref_exists(exact: &HashSet<String>, lower: &HashSet<String>, base_dir: &str, rel: &str) -> bool {
    let rel = rel.split(['#', '?']).next().unwrap_or("");
    let target = resolve(base_dir, &percent_decode(rel));
    exact.contains(&target) || lower.contains(&target.to_ascii_lowercase())
}

/// 去掉 `<img>` 里 src 指向书内不存在文件的标签。alt 有实际内容（非空且不是我们自己封面转换写的 "cover"）的留着，
/// 这类图坏了阅读器还可能显示替代文字，不冒丢内容的风险。返回 (新文本, 去掉个数)。
pub(super) fn drop_dead_imgs(html: &str, base_dir: &str, exact: &HashSet<String>, lower: &HashSet<String>) -> (String, usize) {
    static IMG: OnceLock<Regex> = OnceLock::new();
    static SRC: OnceLock<Regex> = OnceLock::new();
    static ALT: OnceLock<Regex> = OnceLock::new();
    let img = IMG.get_or_init(|| Regex::new(r#"(?is)<img\b[^>]*>"#).unwrap());
    let src = SRC.get_or_init(|| Regex::new(r#"(?is)\bsrc\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());
    let alt = ALT.get_or_init(|| Regex::new(r#"(?is)\balt\s*=\s*(?:"([^"]*)"|'([^']*)')"#).unwrap());
    let mut n = 0;
    let out = img.replace_all(html, |c: &regex::Captures| {
        let tag = &c[0];
        let Some(sc) = src.captures(tag) else { return tag.to_string() };
        let r = sc.get(1).or_else(|| sc.get(2)).map(|m| m.as_str()).unwrap_or("");
        if is_external_ref(r) || ref_exists(exact, lower, base_dir, r) {
            return tag.to_string();
        }
        let alt_text = alt.captures(tag).and_then(|a| a.get(1).or_else(|| a.get(2))).map(|m| m.as_str().trim().to_string()).unwrap_or_default();
        if !alt_text.is_empty() && alt_text != "cover" {
            return tag.to_string();
        }
        n += 1;
        String::new()
    });
    (out.into_owned(), n)
}

/// 清理 `@font-face` 里必然读不到的字体来源：`url()` 指向书内不存在的文件，或设备路径（DuoKan 的
/// `res:///sdcard/...`、`res:///opt/sony/...`，任何阅读器都读不到）。
/// - 一条规则的 `url()` 全死且没有 `local()` 候选 → 整条删；
/// - 有 `local()` 候选或还有活的 `url()` → 只剔除死 `url()`（连同后面的 `format()` 和一个逗号），其余保留。
///
/// 外部（http/data）来源视为活。返回 (新 css, 改动的规则数)。
pub(super) fn drop_dead_font_faces(css: &str, base_dir: &str, exact: &HashSet<String>, lower: &HashSet<String>) -> (String, usize) {
    static FACE: OnceLock<Regex> = OnceLock::new();
    static URL: OnceLock<Regex> = OnceLock::new();
    let face = FACE.get_or_init(|| Regex::new(r#"(?is)@font-face\s*\{[^}]*\}"#).unwrap());
    let url = URL.get_or_init(|| Regex::new(r#"(?is)url\(\s*(?:"([^"]*)"|'([^']*)'|([^)\s]*))\s*\)(?:\s*format\([^)]*\))?"#).unwrap());
    let mut n = 0;
    let out = face.replace_all(css, |c: &regex::Captures| {
        let block = &c[0];
        let has_local = block.to_ascii_lowercase().contains("local(");
        let mut dead: Vec<(usize, usize)> = Vec::new();
        let mut total = 0;
        for u in url.captures_iter(block) {
            total += 1;
            let r = u.get(1).or_else(|| u.get(2)).or_else(|| u.get(3)).map(|m| m.as_str()).unwrap_or("");
            let device_path = r.trim().to_ascii_lowercase().starts_with("res:");
            if device_path || !(is_external_ref(r) || ref_exists(exact, lower, base_dir, r)) {
                let m = u.get(0).unwrap();
                dead.push((m.start(), m.end()));
            }
        }
        if dead.is_empty() {
            return block.to_string();
        }
        n += 1;
        if !has_local && dead.len() == total {
            return String::new();
        }
        let mut out = block.to_string();
        for (st, en) in dead.into_iter().rev() {
            // 连同一个逗号一起删：优先吃前面的 ","，没有就吃后面的
            let before = out[..st].trim_end();
            if before.ends_with(',') {
                let cut = before.len() - 1;
                out.replace_range(cut..en, "");
            } else {
                let after = out[en..].trim_start();
                let skip = if after.starts_with(',') { out.len() - after.len() + 1 } else { en };
                out.replace_range(st..skip, "");
            }
        }
        out
    });
    (out.into_owned(), n)
}

/// 清掉书里指向不存在文件的 `<img>` 和字体全缺的 `@font-face`。xochitl 遇到会逐次报 `Unable to find file`/
/// `Failed to open` 并白白尝试加载（真机日志 2026-09-20：一本书打开就 66 次 cover.jpg 找不到 + 一批 DuoKan 字体）。
/// 只删这两类**必然失效**的引用，不碰文字。远程/data 引用、大小写差异、带替代文字的图一律保留。
pub(super) fn drop_dead_refs(entries: &mut [Entry], rep: &mut WashReport) {
    let exact: HashSet<String> = entries.iter().map(|e| e.name.clone()).collect();
    let lower: HashSet<String> = exact.iter().map(|n| n.to_ascii_lowercase()).collect();
    let mut total = 0;
    for e in entries.iter_mut() {
        let is_css = e.name.to_ascii_lowercase().ends_with(".css");
        if !is_css && !is_html_entry(&e.name, &e.data) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&e.data) else { continue };
        let base = dir_of(&e.name).to_string();
        let (new, n) = if is_css {
            drop_dead_font_faces(text, &base, &exact, &lower)
        } else {
            let (t, a) = drop_dead_imgs(text, &base, &exact, &lower);
            let mut b = 0;
            let t = style_block_re().replace_all(&t, |c: &regex::Captures| {
                let (css, k) = drop_dead_font_faces(&c[2], &base, &exact, &lower);
                b += k;
                format!("{}{}{}", &c[1], css, &c[3])
            });
            (t.into_owned(), a + b)
        };
        if n > 0 {
            e.data = new.into_bytes();
            total += n;
        }
    }
    rep.dead_refs_removed = total;
}
