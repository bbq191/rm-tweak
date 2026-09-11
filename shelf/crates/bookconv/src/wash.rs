//! 清洗层（对标 host `wash_epub.sh` 的 Calibre 规则，2026-09-03 移植；白皮书 §03i）。作用于**解包后的条目表**，
//! 由 `optimize::optimize_epub_with` 在优化器各遍之前调用，host CLI `epub-optimize` 与设备 book-serve 走同一份。
//!
//! 规则（与 Calibre 参数一一对应）：
//! 1. 伪 DRM 剥离（= `strip_pseudo_drm.py`）：`META-INF/encryption.xml` 只列样式/字体/脚本 → 丢弃这些文件 +
//!    encryption.xml + OPF manifest 项；列了正文/图片/导航 = 真 DRM → **报错停下**。
//! 2. CSS 锁剥离（**只剥字号/字体锁**：独立 .css、`<style>`、`style=""` 三处剥 `font-family`/`font-size`/`font`；
//!    补优化器 `strip_font_locks` 只剥内联字体锁的缺口）：2026-09-17 前 `color`/`background-color`/`text-align`
//!    也在剥离名单里，那是照抄 Calibre `--filter-css` 的通用参数、不是针对 xochitl 验证过的必要行为——EPUB 线
//!    原则明确要求保留原书颜色/加粗等元素样式，只解锁字号，故收窄。`background`/`background-image` 仍然剥
//!    （见规则⑧，xochitl 平铺背景图盖正文是坐实的渲染 bug，跟字号锁无关，不能一起放开）。
//! 3. 边距归零（= `--margin-* 0`）：body/html/@page 的 margin/padding 删掉，并注入 `html,body{margin:0;padding:0}`。
//! 4. 段距归零 + 首行缩进（= `--remove-paragraph-spacing --remove-paragraph-spacing-indent-size 2`）：p/div 的
//!    上下 margin/padding 归零（左右保留：blockquote/列表缩进不伤），`p{text-indent:2em}`；`keep_para_spacing` 时
//!    只注缩进（= `WASH_KEEP_PARA_SPACING=1`）。注入块带 `!important` 兜住类选择器（`.calibre1{margin:1em 0}`）。
//! 5. 空页清理：正文无文字无图（Calibre MOBI 转出的 `mbppagebreak` 独占页）→ 从 spine/manifest/zip 删除，
//!    目录里指向它的条目改指下一篇。
//! 6. 自动目录（= `--use-auto-toc --level1-toc //h:h1 --level2-toc //h:h2`）：缺省**仅在书无目录时**从 h1/h2 生成
//!    `toc.ncx` + `nav.xhtml`（xochitl 两者都认）；`AutoToc::Always` 强制重建（原目录坏掉的书）。
//! 7. 单标签重复 `id=` 折叠（`collapse_dup_id_attrs`）：非法 XHTML 会让 xochitl 整章白屏，这里先修、质量门再拦。
//!
//! 全部规则幂等：注入块带 `class="cj-wash"` 标记，重复过不再叠加。
use crate::htmlproc::collapse_dup_id_attrs;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// zip 条目（目录项已剔除）。
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoToc {
    Off,
    /// 书无目录（nav/ncx 缺失或零条目）时生成。
    IfMissing,
    Always,
}

/// 正文排版语言（决定首行缩进/段落习惯）。`Auto` 由 `wash_entries` 按全书 CJK/拉丁字符占比判定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LangMode {
    Auto,
    /// 中文习惯：首行缩进 2em（两个全角字）、段间无空。
    Cjk,
    /// 拉丁习惯：首行缩进 1.2em、标题后首段不缩进。
    Latin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WashOpts {
    /// 保留原书段间距（诗集/剧本靠空行分节）。
    pub keep_para_spacing: bool,
    pub auto_toc: AutoToc,
    /// 剥掉的 CSS 属性（小写）。缺省与 host `--filter-css` 一致。
    pub filter_props: Vec<String>,
    /// 正文排版语言（`Auto`=自动探测）。
    pub lang: LangMode,
}

impl Default for WashOpts {
    fn default() -> Self {
        WashOpts { keep_para_spacing: false, auto_toc: AutoToc::IfMissing, filter_props: DEFAULT_FILTER_PROPS.iter().map(|s| s.to_string()).collect(), lang: LangMode::Auto }
    }
}

/// 注入排版规则的外链 css 文件名（放 OPF 同目录）。真机坐实（2026-09-04《缩进诊断6》/《飘》）：
/// **xochitl 只认外链 `.css` 文件里的规则，完全无视内联 `<style>` 块和元素 `style=` 属性**——所以
/// 排版规则（首行缩进/边距）必须写成外链 css 才在 xochitl 生效（KOReader/crengine 两者都认）。
/// ⚠ xochitl 的 css 解析器很脆：**只用裸元素选择器**（`p`/`body`），一条类/复杂选择器就可能让整表失效
/// （《缩进诊断5》带 `.big` 类规则时整表不生效，diag6 纯 `p{}` 生效）。
const WASH_CSS_NAME: &str = "cangjie-wash.css";

// background / background-image：书常在 body/分卷页用 CSS 背景图（装饰纹样、分卷插画）。xochitl **无视
// no-repeat / background-size** → 把背景图**平铺**满页盖住正文（真机《飘》body.fen 的 `background:url() no-repeat`
// 被铺成多幅）。剥掉背景图声明即净页（章头 <img> 装饰不受影响，仍保留）。@font-face 的 src:url() 由 filter_css 豁免。
// ⚠ 2026-09-17 起不再剥 `color`/`background-color`/`text-align`——EPUB 线原则要求保留原书颜色/加粗等元素样式，
// 只解锁字号；这三项此前只是照抄 Calibre `--filter-css` 通用参数，没有真机验证过是必须剥的。放开后如果书里有
// "深底浅字"高亮块，Paper Pro Move 彩色 e-ink 屏在低对比场景下可能比剥离前更难读——`boost_text_contrast()`
// 目前只处理文字颜色/字重，不处理背景色对比度，真机验证时要专门挑一本带彩色底纹的书测。
pub const DEFAULT_FILTER_PROPS: &[&str] = &["font-family", "font-size", "font", "background-image", "background"];
/// 伪 DRM 允许加密的扩展名（= strip_pseudo_drm.py SAFE_EXTS）。
pub const PSEUDO_DRM_SAFE_EXTS: &[&str] = &[".css", ".ttf", ".otf", ".woff", ".woff2", ".js"];
const WASH_MARK: &str = "cj-wash";

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct WashReport {
    pub pseudo_drm_stripped: Vec<String>,
    pub css_files: usize,
    pub html_files: usize,
    pub empty_pages_removed: Vec<String>,
    pub toc_generated: usize,
    pub dup_id_tags_collapsed: usize,
    /// `toc.ncx` 的 `dtb:uid` 跟 OPF 标识符不一致、被改到一致（0 或 1——一本书只有一个 ncx）。
    pub ncx_uid_fixed: usize,
    /// 书自带的扁平目录（"第X部　编号　章名"排版惯例）被重建成两级后的条目数；0＝没检测到这种
    /// 惯例、原样没动。
    pub toc_parts_restructured: usize,
    /// `toc.ncx` 里指向外部 DTD 的 `<!DOCTYPE>` 声明被剥掉（0 或 1）。
    pub ncx_doctype_stripped: usize,
    /// manifest 里 NCX 条目的 `id` 被改成 `"ncx"`（0 或 1）。见 `fix_ncx_manifest_id`。
    pub ncx_manifest_id_fixed: usize,
}

pub fn is_html(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".xhtml") || l.ends_with(".html") || l.ends_with(".htm")
}

// ───────────────────────── 路径工具（zip 内 posix 路径） ─────────────────────────

pub fn posix_norm(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

pub fn dir_of(p: &str) -> &str {
    p.rfind('/').map(|i| &p[..i]).unwrap_or("")
}

pub fn resolve(base_dir: &str, rel: &str) -> String {
    if base_dir.is_empty() {
        posix_norm(rel)
    } else {
        posix_norm(&format!("{base_dir}/{rel}"))
    }
}

/// `target` 相对 `base_dir` 的路径（都是 zip 内绝对路径）。
pub fn relative_to(base_dir: &str, target: &str) -> String {
    let b: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    let t: Vec<&str> = target.split('/').filter(|s| !s.is_empty()).collect();
    let common = b.iter().zip(t.iter()).take_while(|(x, y)| x == y).count();
    let mut out: Vec<String> = vec!["..".into(); b.len() - common];
    out.extend(t[common..].iter().map(|s| s.to_string()));
    out.join("/")
}

pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let (Some(h), Some(l)) = (b.get(i + 1), b.get(i + 2)) {
                if let Ok(v) = u8::from_str_radix(&format!("{}{}", *h as char, *l as char), 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn find_opf(entries: &[Entry]) -> Option<usize> {
    // container.xml 指向优先，否则第一个 .opf
    if let Some(c) = entries.iter().find(|e| e.name == "META-INF/container.xml") {
        let t = String::from_utf8_lossy(&c.data);
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r#"full-path="([^"]+)""#).unwrap());
        if let Some(m) = re.captures(&t) {
            let p = posix_norm(&m[1]);
            if let Some(i) = entries.iter().position(|e| e.name == p) {
                return Some(i);
            }
        }
    }
    entries.iter().position(|e| e.name.to_ascii_lowercase().ends_with(".opf"))
}

// ───────────────────────── 1. 伪 DRM ─────────────────────────

/// encryption.xml 里的加密目标（zip 内路径）。
pub fn encrypted_targets(entries: &[Entry]) -> Option<Vec<String>> {
    let enc = entries.iter().find(|e| e.name == "META-INF/encryption.xml")?;
    let t = String::from_utf8_lossy(&enc.data);
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"CipherReference\s+URI="([^"]+)""#).unwrap());
    Some(re.captures_iter(&t).map(|c| posix_norm(&percent_decode(&c[1]))).filter(|u| !u.starts_with('#')).collect())
}

/// 真 DRM 判据：加密了非样式/字体/脚本的文件。返回违规项。
pub fn real_drm_items(targets: &[String]) -> Vec<String> {
    targets.iter().filter(|t| { let l = t.to_ascii_lowercase(); !PSEUDO_DRM_SAFE_EXTS.iter().any(|e| l.ends_with(e)) }).cloned().collect()
}

fn strip_pseudo_drm(entries: &mut Vec<Entry>, rep: &mut WashReport) -> Result<(), String> {
    let Some(targets) = encrypted_targets(entries) else { return Ok(()) };
    let bad = real_drm_items(&targets);
    if !bad.is_empty() {
        return Err(format!("加密 EPUB（真 DRM，加密了 {} 等 {} 项），xochitl/KOReader 都读不了", bad.iter().take(3).cloned().collect::<Vec<_>>().join("、"), bad.len()));
    }
    let drop: HashSet<String> = targets.iter().cloned().chain(std::iter::once("META-INF/encryption.xml".to_string())).collect();
    if let Some(oi) = find_opf(entries) {
        let opf_dir = dir_of(&entries[oi].name).to_string();
        let mut text = String::from_utf8_lossy(&entries[oi].data).into_owned();
        for t in &targets {
            let rel = relative_to(&opf_dir, t);
            let re = Regex::new(&format!(r#"<item\b[^>]*\bhref="{}"[^>]*/>\s*"#, regex::escape(&rel))).unwrap();
            text = re.replace_all(&text, "").into_owned();
        }
        entries[oi].data = text.into_bytes();
    }
    entries.retain(|e| !drop.contains(&e.name));
    rep.pseudo_drm_stripped = targets;
    Ok(())
}

// ───────────────────────── 2–4. CSS 声明处理 ─────────────────────────

fn decl_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // 属性名 : 值（值里的 HTML 实体 `&#39;` 含分号，按实体整体吃）
    RE.get_or_init(|| Regex::new(r#"(?i)([-a-zA-Z]+)\s*:\s*((?:&#?\w+;|[^;])*);?"#).unwrap())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Spacing {
    /// 不动 margin/padding。
    Keep,
    /// 上下归零、左右保留（p/div）。
    Vertical,
    /// 全部删除（body/html/@page）。
    All,
}

/// 对一段声明文本：剥 `filter` 里的属性；按 `spacing` 处理 margin/padding。（测试用薄封装）
#[cfg(test)]
fn filter_decls(decls: &str, filter: &[String], spacing: Spacing) -> String {
    filter_decls_with(decls, filter, spacing, None)
}

/// 值是否"非零缩进"（`0` / `0em` / `0.0pt` 之类算零；负值=悬挂缩进，保留不动）。
fn is_positive_indent(val: &str) -> bool {
    let v = val.trim().trim_end_matches("!important").trim();
    let num: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+').collect();
    num.parse::<f64>().map(|n| n > 0.0).unwrap_or(false)
}

/// 同 `filter_decls`，另把书里**非零** `text-indent` 统一改成 `indent`（Some 时）。
/// 为什么：书自带的类规则（calibre 转 AZW3 常见 `.calibre_ {text-indent:2em}`）xochitl 不认（只认裸 `p{}`），
/// KOReader 认且类规则特异性高于我们的 `p{}`——不统一就"xochitl 1.2em、KOReader 2em"，两器同字节不同观感
/// （2026-09-06 Phase E 英文书对照发现）。`text-indent:0`（诗歌/引文/列表明示不缩进）与负值保留。
fn filter_decls_with(decls: &str, filter: &[String], spacing: Spacing, indent: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    for c in decl_re().captures_iter(decls) {
        let prop = c[1].to_ascii_lowercase();
        let val = c[2].trim();
        if filter.iter().any(|f| *f == prop) {
            continue;
        }
        if prop == "text-indent" {
            if let Some(ind) = indent {
                if is_positive_indent(val) {
                    out.push(format!("text-indent:{ind}"));
                    continue;
                }
            }
        }
        let is_box = prop == "margin" || prop == "padding";
        let is_box_side = prop.starts_with("margin-") || prop.starts_with("padding-");
        match spacing {
            Spacing::Keep => {}
            Spacing::All if is_box || is_box_side => continue,
            Spacing::Vertical if prop.ends_with("-top") || prop.ends_with("-bottom") => {
                if is_box_side {
                    continue;
                }
            }
            Spacing::Vertical if is_box => {
                // 简写：保留左右
                let parts: Vec<&str> = val.split_whitespace().filter(|p| !p.starts_with('!')).collect();
                let important = if val.contains("!important") { " !important" } else { "" };
                let (r, l) = match parts.len() {
                    1 => (parts[0], parts[0]),
                    2 | 3 => (parts[1], parts[1]),
                    4 => (parts[1], parts[3]),
                    _ => continue,
                };
                out.push(if r == l { format!("{prop}:0 {r}{important}") } else { format!("{prop}:0 {r} 0 {l}{important}") });
                continue;
            }
            _ => {}
        }
        out.push(format!("{prop}:{val}"));
    }
    // 尾分号：xochitl 丢规则里最后一个无分号的声明（书 css `.calibre_ {…;margin:0}` 的 margin 曾被吞；若 text-indent 排最后就没缩进）
    let mut joined = out.join(";");
    if !joined.is_empty() {
        joined.push(';');
    }
    joined
}

fn selector_spacing(selector: &str) -> Spacing {
    static ELEM_P: OnceLock<Regex> = OnceLock::new();
    static ELEM_BODY: OnceLock<Regex> = OnceLock::new();
    let p = ELEM_P.get_or_init(|| Regex::new(r#"(?i)(^|[\s,>+~])(p|div)(?:[\s,.#:\[]|$)"#).unwrap());
    let b = ELEM_BODY.get_or_init(|| Regex::new(r#"(?i)(^|[\s,>+~])(body|html)(?:[\s,.#:\[]|$)|^@page\b"#).unwrap());
    let s = selector.trim();
    if b.is_match(s) {
        Spacing::All
    } else if p.is_match(s) {
        Spacing::Vertical
    } else {
        Spacing::Keep
    }
}

/// 整段 CSS（文件或 <style> 内容）：逐规则剥锁 + 边距处理。`@media{}` 嵌套靠"从内向外"匹配最内层规则。
pub fn filter_css(css: &str, opts: &WashOpts) -> String {
    static RULE: OnceLock<Regex> = OnceLock::new();
    let rule = RULE.get_or_init(|| Regex::new(r#"(?s)([^{}]+)\{([^{}]*)\}"#).unwrap());
    rule.replace_all(css, |c: &regex::Captures| {
        let sel = &c[1];
        let trimmed = sel.trim_start();
        if trimmed.starts_with("@font-face") || trimmed.starts_with("@import") {
            return c[0].to_string();
        }
        let spacing = match selector_spacing(sel) {
            Spacing::Vertical if opts.keep_para_spacing => Spacing::Keep,
            s => s,
        };
        format!("{}{{{}}}", sel, filter_decls_with(&c[2], &opts.filter_props, spacing, Some(indent_for(opts))))
    }).into_owned()
}

/// 本书的首行缩进值（拉丁 1.2em / 中文 2em；Auto 兜底中文）。`wash_css` 与书 css 统一改写共用。
fn indent_for(opts: &WashOpts) -> &'static str {
    if opts.lang == LangMode::Latin { "1.2em" } else { "2em" }
}

fn style_attr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?is)<([a-z][a-z0-9]*)\b([^>]*?)\sstyle="([^"]*)"([^>]*)>"#).unwrap())
}

/// (x)html：`style=""`（按标签名定边距策略）+ `<style>` 块剥锁；注入清洗样式块；折叠重复 id。
pub fn wash_html(html: &str, opts: &WashOpts) -> (String, usize) {
    let before_dup = count_dup_id_tags(html);
    let s = collapse_dup_id_attrs(html);
    let s = style_attr_re().replace_all(&s, |c: &regex::Captures| {
        let tag = c[1].to_ascii_lowercase();
        let spacing = match tag.as_str() {
            "body" | "html" => Spacing::All,
            "p" | "div" if !opts.keep_para_spacing => Spacing::Vertical,
            _ => Spacing::Keep,
        };
        let cleaned = filter_decls_with(&c[3], &opts.filter_props, spacing, Some(indent_for(opts)));
        if cleaned.is_empty() {
            format!("<{}{}{}>", &c[1], &c[2], &c[4])
        } else {
            format!("<{}{} style=\"{}\"{}>", &c[1], &c[2], cleaned, &c[4])
        }
    }).into_owned();
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    let block = BLOCK.get_or_init(|| Regex::new(r#"(?is)(<style\b[^>]*>)(.*?)(</style>)"#).unwrap());
    let s = block.replace_all(&s, |c: &regex::Captures| {
        if c[1].contains(WASH_MARK) {
            // 旧版（v9 及以前）注入的内联 <style class="cj-wash"> 块：xochitl 本就无视它，重洗时清掉（已改外链 css）。
            String::new()
        } else {
            format!("{}{}{}", &c[1], filter_css(&c[2], opts), &c[3])
        }
    }).into_owned();
    // 不再注入内联 <style>（xochitl 无视内联）；排版规则由 wash_entries 写成外链 css + 逐 html 加 <link>。
    let s = match opts.lang {
        LangMode::Latin => flush_first_para_after_heading(&s),
        _ => cjk_paragraphize(&s),
    };
    (s, before_dup)
}

/// 中文书两种"假段落"归一（2026-09-06 《人骨拼圖》2017 旧 EPUB 真机）：
/// ① 全书没有 `<p>`，每章一个 `<div>` 里 `<br/>` 分行、段首两个全角空格——`p{text-indent:2em}` 没有对象，xochitl 又把
///    U+3000 折叠掉 → 零缩进；KOReader 把 U+3000 按字体宽度画出来 → "换字体缩进跟着变"。→ 按 `<br>` 切成 `<p>`。
/// ② 段首烘死的全角空格 / nbsp（有 `<p>` 的书也常见）→ 剥掉，缩进统一走外链 css（字体无关的精确 2em）。
/// 只在文件里 `<br` 数 ≥ 4 且 `<p` 为 0 时做 ①；② 对所有 `<p>` 做。块级标签（h1–h6/div/section 的开闭、img、table）原样保留。
fn cjk_paragraphize(html: &str) -> String {
    static LEAD: OnceLock<Regex> = OnceLock::new();
    static BR: OnceLock<Regex> = OnceLock::new();
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    let lead = LEAD.get_or_init(|| Regex::new(r#"(?i)(<p\b[^>]*>)(?:\s|\u{3000}|&#12288;|&#x3000;|&nbsp;|&#160;|&#xa0;)+"#).unwrap());
    let out = lead.replace_all(html, "$1").into_owned();
    let n_br = out.matches("<br").count();
    let n_p = Regex::new(r#"(?i)<p\b"#).unwrap().find_iter(&out).count();
    if n_br < 4 || n_p > 0 {
        return out;
    }
    let Some(bstart) = out.find("<body") else { return out };
    let Some(bopen_end) = out[bstart..].find('>').map(|i| bstart + i + 1) else { return out };
    let Some(bend) = out.rfind("</body>") else { return out };
    let (head, body, tail) = (&out[..bopen_end], &out[bopen_end..bend], &out[bend..]);
    let br = BR.get_or_init(|| Regex::new(r#"(?is)(?:\s*<br\b[^>]*>\s*)+"#).unwrap());
    // 块级开闭标签 / 整块元素：不裹进 p
    let block = BLOCK.get_or_init(|| Regex::new(r#"(?is)^\s*(?:</?(?:div|section|article|body|blockquote|ul|ol|li|table|tr|td|th|figure|figcaption)\b[^>]*>|<h[1-6]\b[^>]*>.*?</h[1-6]>|<img\b[^>]*>|<hr\b[^>]*>|<a\b[^>]*id="[^"]*"[^>]*>\s*</a>)\s*"#).unwrap());
    let mut res = String::with_capacity(body.len() + 64);
    for piece in br.split(body) {
        let mut rest = piece;
        // 剥前导块级标签
        loop {
            match block.find(rest) {
                Some(m) if m.start() == 0 => {
                    res.push_str(&rest[..m.end()]);
                    rest = &rest[m.end()..];
                }
                _ => break,
            }
        }
        // 剥尾随块级闭合标签
        let mut trailing = String::new();
        loop {
            let t = rest.trim_end();
            if let Some(i) = t.rfind('<') {
                let tag = &t[i..];
                if Regex::new(r#"(?i)^</(?:div|section|article|blockquote|ul|ol|li|table|tr|td|th|figure)>$"#).unwrap().is_match(tag) {
                    trailing.insert_str(0, tag);
                    rest = &t[..i];
                    continue;
                }
            }
            break;
        }
        let text = rest.trim().trim_start_matches(|c: char| c == '\u{3000}' || c == '\u{a0}' || c.is_whitespace());
        if !text.is_empty() {
            res.push_str("<p>");
            res.push_str(text);
            res.push_str("</p>");
        }
        res.push_str(&trailing);
    }
    format!("{head}{res}{tail}")
}

/// 拉丁习惯：**标题后 / 章首 / 场景切换后的第一段不缩进**（英文排版惯例：只有紧接上一段的段落才缩进）。
/// xochitl 的 CSS 引擎（2026-09-06 八轮渲染缓存量化，书架白皮书 §03y）：不认内联 `style=""`；`text-indent:0` 当"没设"；
/// 类规则压过元素规则，但同为类规则时**先出现者胜**（书的表链接在前）；规则最后一个无分号的声明被丢。
/// 因此顶格段＝`<div class="cj-flush">`（**剥掉书的类与 style**，只留 cj-flush，id 等保留）+ 外链 `.cj-flush{text-indent:0;…;}`；
/// KOReader 走标准 CSS 同样顶格。判定"前面是标题/切换"的信号（2026-09-06 用《Tell Me Your Dreams》AZW3 定，它的章名不是 `<h>`
/// 而是加粗段落、场景切换是段末双 `<br/>`）：
///   ① 前一个块是 `</h1>`–`</h6>`；② 前一段是"标题样段落"：≤80 字且（全文加粗/strong/class 含 bold、或以
///   Chapter/Book/Part/Prologue/Epilogue 开头）且不以句末标点结尾；③ 前一段以 ≥2 个 `<br>` 结尾或本身是空段/`* * *`
///   之类的分隔（空段过多的书——用空段当段距——不按分隔算）；④ 文件里第一个有正文的段（章首）。幂等。
/// ⚠ xochitl 认不认内联 style 属性待真机核（书架白皮书 §05 Phase E ②）；不认也只是照常缩进 1.2em，无害。
fn flush_first_para_after_heading(html: &str) -> String {
    static P: OnceLock<Regex> = OnceLock::new();
    static CLASS: OnceLock<Regex> = OnceLock::new();
    static STYLE: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static BR2: OnceLock<Regex> = OnceLock::new();
    static HEAD_WORD: OnceLock<Regex> = OnceLock::new();
    static SEP: OnceLock<Regex> = OnceLock::new();
    // 块序列：h 结束标签 / p 元素（p 内不再嵌 p，非贪婪到最近 </p> 够用）/ 上次洗出的 cj-flush div（重洗幂等）
    let p = P.get_or_init(|| Regex::new(r#"(?is)</h[1-6]>|<p\b([^>]*)>(.*?)</p>|<div\b([^>]*\bcj-flush\b[^>]*)>(.*?)</div>"#).unwrap());
    let class_re = CLASS.get_or_init(|| Regex::new(r#"(?i)\bclass="([^"]*)""#).unwrap());
    let style_re = STYLE.get_or_init(|| Regex::new(r#"(?i)\bstyle="[^"]*""#).unwrap());
    let tag = TAG.get_or_init(|| Regex::new(r#"(?s)<[^>]+>"#).unwrap());
    let br2 = BR2.get_or_init(|| Regex::new(r#"(?is)(<br\b[^>]*>\s*){2,}(</span>|</a>|\s)*$"#).unwrap());
    let head_word = HEAD_WORD.get_or_init(|| Regex::new(r#"(?i)^\s*(chapter|book|part|prologue|epilogue|section|interlude)\b"#).unwrap());
    let sep = SEP.get_or_init(|| Regex::new(r#"^[\s\*#~—–\-·•]*$"#).unwrap());
    let text_of = |inner: &str| -> String {
        let t = tag.replace_all(inner, "");
        t.replace("&#160;", " ").replace("&nbsp;", " ").trim().to_string()
    };
    // 空段太多（>20%）的书是拿空段当段距，不当场景分隔
    let total_p = p.captures_iter(html).filter(|c| c.get(1).is_some() || c.get(3).is_some()).count();
    let empty_p = p.captures_iter(html).filter(|c| c.get(2).or(c.get(4)).map(|m| sep.is_match(&text_of(m.as_str()))).unwrap_or(false)).count();
    let empty_is_sep = total_p == 0 || empty_p * 5 <= total_p;
    let mut flush_next = true; // ④ 章首
    p.replace_all(html, |c: &regex::Captures| {
        let (attrs, inner, already_div) = match (c.get(1), c.get(3)) {
            (Some(a), _) => (a.as_str(), &c[2], false),
            (None, Some(a)) => (a.as_str(), &c[4], true),
            (None, None) => {
                flush_next = true; // ① </hN>
                return c[0].to_string();
            }
        };
        let text = text_of(inner);
        if text.is_empty() || sep.is_match(&text) {
            if empty_is_sep {
                flush_next = true; // ③ 空段 / * * * 分隔
            }
            return c[0].to_string();
        }
        let bold_wrapped = (inner.contains("<b>") || inner.contains("<strong") || inner.contains("bold")) && text.chars().count() <= 80;
        let heading_like = !_terminal_latin(&text) && text.chars().count() <= 80 && (bold_wrapped || head_word.is_match(&text));
        let out = if flush_next && !heading_like && !already_div {
            // 只留 cj-flush 一个类、去掉 style：书的类规则（如 `.calibre_ {text-indent:1.2em}`）在 xochitl 里同为类规则时
            // **先出现者胜**（诊断 13/14），带着书的类就压不住；元素/内联通道又都不通（诊断 7–10）。id 等其它属性保留。
            let mut kept = class_re.replace_all(attrs, "").to_string();
            kept = style_re.replace_all(&kept, "").to_string();
            let kept = kept.split_whitespace().collect::<Vec<_>>().join(" ");
            let sep = if kept.is_empty() { "" } else { " " };
            format!("<div class=\"cj-flush\"{sep}{kept}>{inner}</div>")
        } else {
            c[0].to_string()
        };
        // 下一段是否顶格：② 本段是标题样段落；③ 本段以双 <br> 结尾
        flush_next = heading_like || br2.is_match(inner);
        out
    }).into_owned()
}

/// 拉丁段落是否以句末标点结束（标题样段落判定用）。
fn _terminal_latin(t: &str) -> bool {
    t.trim_end().chars().last().map(|c| ".!?\"'”’)".contains(c)).unwrap_or(false)
}

/// 给 `<head>` 注入指向外链 wash css 的 `<link>`（`href`=该 html 相对 css 的路径）。幂等（已有则跳过）。
/// 无 `</head>` 时补一对 head；无 `<body` 也不动（异常文件）。
fn inject_css_link(html: &str, href: &str) -> String {
    let marker = format!("href=\"{href}\"");
    if html.contains(&marker) {
        return html.to_string();
    }
    let link = format!("<link rel=\"stylesheet\" type=\"text/css\" href=\"{href}\"/>");
    if let Some(i) = html.find("</head>") {
        format!("{}{}{}", &html[..i], link, &html[i..])
    } else if let Some(i) = html.find("<body") {
        format!("{}<head>{}</head>{}", &html[..i], link, &html[i..])
    } else {
        html.to_string()
    }
}

/// 外链 wash css 的内容。按 `opts.lang` 注中/英首行缩进习惯 + 段落上下边距归零（除非 keep_para_spacing）。
/// ⚠ 两条 xochitl css 解析器的脆弱性（真机坐实）：
/// ① **只用裸元素选择器 `p{}`**——一条类/相邻/at-rule 选择器就让整表失效（《缩进诊断5》带 `.big` 时连 `p{}` 都不生效）。
/// ② **不用 `!important`**——带 `!important` 的外链规则不生效（v10 真机「都没缩进」），去掉即生效（diag6/《飘》验证）。
/// 我们的 `<link>` 注在 `</head>` 前、晚于书自带 css，同特异性靠源序后者胜，无需 `!important` 也能盖过书里的 `p{text-indent:0}`。
/// `opts.lang` 应已被 `wash_entries` 从 `Auto` 解析为具体值（此处把 `Auto` 兜底当 `Cjk`）。
pub fn wash_css(opts: &WashOpts) -> String {
    // 拉丁 1.2em / 中文 2em（Auto 兜底中文）。
    let indent = indent_for(opts);
    let mut decl = format!("text-indent:{indent};");
    if !opts.keep_para_spacing {
        decl.push_str("margin-top:0;margin-bottom:0;padding-top:0;padding-bottom:0;");
    }
    // ⚠ 每条声明都以 `;` 收尾：xochitl 会丢掉规则里最后一个没分号的声明（2026-09-06 诊断 11/12：`p{text-indent:2em}`
    // 整条不生效、`p{text-indent:2em;}` 生效）——keep_para_spacing 档位此前因此在 xochitl 上没缩进。
    // `.cj-flush`：拉丁首段顶格段（wash_html 换成的 `<div class="cj-flush">`），KOReader 靠它归零缩进/段距；
    // xochitl 上 div 本就无 p 规则、且书的类规则够不到（类已剥），此条只是保险。
    // ⚠ 值用 0.01em 不用 0：xochitl 把 `text-indent:0` 当"没设"→ 落回从外层 `<div class="calibre1">` 之类**继承**来的缩进
    //   （诊断 14 V1/V7 vs Sheldon v5，2026-09-06）；0.01em ≈ 0.1pt 肉眼不可见，KOReader 同样视为顶格。
    let flush = if opts.keep_para_spacing { ".cj-flush{text-indent:0.01em;}" } else { ".cj-flush{text-indent:0.01em;margin-top:0;margin-bottom:0;}" };
    // figure/figcaption：以前这条规则只管了 `<p>` 的边距，`article.rs` 网文管线常把图片包成
    // `<figure><img/><figcaption>…</figcaption></figure>`（真机书里也不算罕见），这两个元素
    // 完全没被清零过——默认（未洗）上下边距在"图片夹在正文中间"的场景会造成明显留白，2026-09-10
    // 真机拿 aeon.co 一篇网文复现坐实（诊断EPUB→投原生→量 xochitl 渲染 PDF，不肉眼猜，见书架
    // 白皮书 §03aq）。**两条规则分开写，不写成 `figure,figcaption{}`**——xochitl 的 CSS 解析器
    // 脆，只认裸元素选择器，逗号/复合选择器直接整条规则失效（`lang_aware_indent` 测试断言过
    // 这条红线，别在这里破例）。keep_para_spacing 档位同样清零：那档的意图是"保留正文段落之间
    // 的呼吸感"，不是"保留图片周围的默认边距"，两件事语义不同，不该被同一个开关连带控制。
    format!("p{{{decl}}}\n{flush}\nfigure{{margin:0;padding:0;}}\nfigcaption{{margin:0;padding:0;}}\n")
}

pub fn count_dup_id_tags(html: &str) -> usize {
    static TAG: OnceLock<Regex> = OnceLock::new();
    static ID: OnceLock<Regex> = OnceLock::new();
    let tag = TAG.get_or_init(|| Regex::new(r#"(?s)<[a-zA-Z][^>]*>"#).unwrap());
    let id = ID.get_or_init(|| Regex::new(r#"\bid=""#).unwrap());
    tag.find_iter(html).filter(|m| id.find_iter(m.as_str()).count() > 1).count()
}

// ───────────────────────── OPF 视图 ─────────────────────────

pub(crate) struct Opf {
    pub(crate) index: usize,
    pub(crate) dir: String,
    /// manifest id → zip 路径
    #[allow(dead_code)]
    pub(crate) items: HashMap<String, String>,
    /// spine 顺序的 zip 路径
    pub(crate) spine: Vec<String>,
    pub(crate) nav_doc: Option<String>,
    #[allow(dead_code)]
    pub(crate) ncx: Option<String>,
}

pub(crate) fn parse_opf(entries: &[Entry]) -> Option<Opf> {
    let index = find_opf(entries)?;
    let dir = dir_of(&entries[index].name).to_string();
    let text = String::from_utf8_lossy(&entries[index].data);
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static ATTR: OnceLock<Regex> = OnceLock::new();
    static REF: OnceLock<Regex> = OnceLock::new();
    let item = ITEM.get_or_init(|| Regex::new(r#"(?s)<item\b([^>]*)/?>"#).unwrap());
    let attr = ATTR.get_or_init(|| Regex::new(r#"([a-zA-Z:-]+)\s*=\s*"([^"]*)""#).unwrap());
    let iref = REF.get_or_init(|| Regex::new(r#"<itemref\b[^>]*\bidref="([^"]+)""#).unwrap());
    let mut items = HashMap::new();
    let mut nav_doc = None;
    let mut ncx = None;
    for c in item.captures_iter(&text) {
        let attrs: HashMap<String, String> = attr.captures_iter(&c[1]).map(|a| (a[1].to_ascii_lowercase(), a[2].to_string())).collect();
        let (Some(id), Some(href)) = (attrs.get("id"), attrs.get("href")) else { continue };
        let path = resolve(&dir, &percent_decode(href));
        if attrs.get("properties").map(|p| p.split_whitespace().any(|x| x == "nav")).unwrap_or(false) {
            nav_doc = Some(path.clone());
        }
        if attrs.get("media-type").map(|m| m.contains("dtbncx")).unwrap_or(false) {
            ncx = Some(path.clone());
        }
        items.insert(id.clone(), path);
    }
    let spine: Vec<String> = iref.captures_iter(&text).filter_map(|c| items.get(&c[1]).cloned()).collect();
    Some(Opf { index, dir, items, spine, nav_doc, ncx })
}

// ───────────────────────── 5. 空页清理 ─────────────────────────

fn is_empty_page(html: &str) -> bool {
    static BODY: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    let body = BODY.get_or_init(|| Regex::new(r#"(?is)<body\b[^>]*>(.*?)</body>"#).unwrap());
    let tag = TAG.get_or_init(|| Regex::new(r#"(?s)<[^>]*>"#).unwrap());
    let inner = body.captures(html).map(|c| c[1].to_string()).unwrap_or_default();
    let low = inner.to_ascii_lowercase();
    if low.contains("<img") || low.contains("<svg") || low.contains("<image") || low.contains("<video") || low.contains("<audio") {
        return false;
    }
    let text = tag.replace_all(&inner, "");
    let text = text.replace("&nbsp;", " ").replace("&#160;", " ").replace('\u{a0}', " ");
    text.trim().is_empty()
}

fn remove_empty_pages(entries: &mut Vec<Entry>, rep: &mut WashReport) {
    let Some(opf) = parse_opf(entries) else { return };
    let mut removed: Vec<String> = Vec::new();
    for p in &opf.spine {
        if Some(p) == opf.nav_doc.as_ref() {
            continue;
        }
        if let Some(e) = entries.iter().find(|e| &e.name == p) {
            if is_html(&e.name) && is_empty_page(&String::from_utf8_lossy(&e.data)) {
                removed.push(p.clone());
            }
        }
    }
    if removed.is_empty() || removed.len() >= opf.spine.len() {
        return; // 全空不动（别把书删没）
    }
    let removed_set: HashSet<&String> = removed.iter().collect();
    // 替换目标：spine 里下一篇未删的，没有则上一篇
    let replacement = |p: &String| -> Option<String> {
        let i = opf.spine.iter().position(|x| x == p)?;
        opf.spine[i + 1..].iter().chain(opf.spine[..i].iter().rev()).find(|x| !removed_set.contains(x)).cloned()
    };
    let repl: HashMap<String, String> = removed.iter().filter_map(|p| replacement(p).map(|r| (p.clone(), r))).collect();
    // OPF：删 itemref + item
    let ids: Vec<String> = opf.items.iter().filter(|(_, v)| removed_set.contains(v)).map(|(k, _)| k.clone()).collect();
    let mut text = String::from_utf8_lossy(&entries[opf.index].data).into_owned();
    for id in &ids {
        let re = Regex::new(&format!(r#"<itemref\b[^>]*\bidref="{}"[^>]*/?>(?:\s*</itemref>)?\s*"#, regex::escape(id))).unwrap();
        text = re.replace_all(&text, "").into_owned();
        let re = Regex::new(&format!(r#"<item\b[^>]*\bid="{}"[^>]*/?>(?:\s*</item>)?\s*"#, regex::escape(id))).unwrap();
        text = re.replace_all(&text, "").into_owned();
    }
    entries[opf.index].data = text.into_bytes();
    // 目录（ncx/nav）里指向被删页的引用 → 改指替换页
    let toc_files: Vec<String> = entries.iter().filter(|e| is_toc_file(&e.name)).map(|e| e.name.clone()).collect();
    for tf in toc_files {
        let tdir = dir_of(&tf).to_string();
        let Some(e) = entries.iter_mut().find(|e| e.name == tf) else { continue };
        let text = String::from_utf8_lossy(&e.data).into_owned();
        let new = href_re().replace_all(&text, |c: &regex::Captures| {
            let target = resolve(&tdir, &percent_decode(&c[2]));
            match repl.get(&target) {
                Some(r) => format!("{}=\"{}\"", &c[1], relative_to(&tdir, r)),
                None => c[0].to_string(),
            }
        }).into_owned();
        e.data = new.into_bytes();
    }
    entries.retain(|e| !removed_set.contains(&e.name));
    rep.empty_pages_removed = removed;
}

pub fn is_toc_file(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    let base = l.rsplit('/').next().unwrap_or(&l);
    l.ends_with(".ncx") || (base.starts_with("nav") && (base.ends_with(".xhtml") || base.ends_with(".html")))
}

pub fn href_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r##"(src|href)="([^"#]+)(#[^"]*)?""##).unwrap())
}

// ───────────────────────── 6. 自动目录 ─────────────────────────

/// 目录条目数（ncx `src` + nav `href`，排除 toc 文件自指与非 html 目标）。
pub fn toc_entry_count(entries: &[Entry]) -> usize {
    let mut n = 0;
    for e in entries.iter().filter(|e| is_toc_file(&e.name)) {
        let t = String::from_utf8_lossy(&e.data);
        n += href_re().captures_iter(&t).filter(|c| { let l = c[2].to_ascii_lowercase(); !l.starts_with("http") && (l.ends_with(".xhtml") || l.ends_with(".html") || l.ends_with(".htm")) }).count();
    }
    n
}

fn heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // h1–h6 都收（书只用 h3 当章标题也不漏目录）；多级由 build_ncx/build_nav 按 dense-rank 嵌套。
    RE.get_or_init(|| Regex::new(r#"(?is)<h([1-6])\b([^>]*)>(.*?)</h[1-6]>"#).unwrap())
}

/// 把出现过的 h 级别稠密化为连续深度 1..N（如书用 {h1,h3} → 各条 rank 1/2），供嵌套用。
fn dense_ranks(items: &[(u8, String, String, String)]) -> Vec<u8> {
    let mut levels: Vec<u8> = items.iter().map(|i| i.0).collect();
    levels.sort_unstable();
    levels.dedup();
    items.iter().map(|i| (levels.iter().position(|&l| l == i.0).unwrap_or(0) as u8) + 1).collect()
}

pub(crate) fn plain_text(html: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    let tag = TAG.get_or_init(|| Regex::new(r#"(?s)<[^>]*>"#).unwrap());
    let t = tag.replace_all(html, "");
    t.replace("&nbsp;", " ").replace("&#160;", " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// 从 spine 各章 h1/h2 生成目录；标题无 id 则补 `id="cj-toc-N"`。返回条目 (level, title, zip路径, frag)。
fn collect_headings(entries: &mut [Entry], spine: &[String], nav_doc: Option<&String>) -> Vec<(u8, String, String, String)> {
    let mut out = Vec::new();
    let mut counter = 0usize;
    static ID: OnceLock<Regex> = OnceLock::new();
    let id_re = ID.get_or_init(|| Regex::new(r#"\bid="([^"]*)""#).unwrap());
    for p in spine {
        if Some(p) == nav_doc {
            continue;
        }
        let Some(e) = entries.iter_mut().find(|e| &e.name == p) else { continue };
        let html = String::from_utf8_lossy(&e.data).into_owned();
        let mut changed = false;
        let new = heading_re().replace_all(&html, |c: &regex::Captures| {
            let level: u8 = c[1].parse().unwrap_or(1);
            let title = plain_text(&c[3]);
            if title.is_empty() {
                return c[0].to_string();
            }
            let attrs = c[2].to_string();
            let (attrs, frag) = match id_re.captures(&attrs) {
                Some(m) => (attrs.clone(), m[1].to_string()),
                None => {
                    counter += 1;
                    changed = true;
                    let f = format!("cj-toc-{counter}");
                    (format!("{attrs} id=\"{f}\""), f)
                }
            };
            out.push((level, title, p.clone(), frag));
            format!("<h{}{}>{}</h{}>", &c[1], attrs, &c[3], &c[1])
        }).into_owned();
        if changed {
            e.data = new.into_bytes();
        }
    }
    out
}

fn build_ncx(items: &[(u8, String, String, String)], ncx_dir: &str, title: &str, uid: &str) -> String {
    let ranks = dense_ranks(items);
    let depth_max = ranks.iter().copied().max().unwrap_or(1);
    let mut s = format!(r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1"><head><meta name="dtb:uid" content="{}"/><meta name="dtb:depth" content="{depth_max}"/></head><docTitle><text>{}</text></docTitle><navMap>"#, xml_escape(uid), xml_escape(title));
    let mut depth = 0u8; // 当前打开的 navPoint 层数
    for (i, (_, t, path, frag)) in items.iter().enumerate() {
        let d = ranks[i].min(depth + 1); // 钳制：不跳跃深入 >1 层，保证良构
        if d <= depth {
            for _ in 0..(depth - d + 1) {
                s.push_str("</navPoint>");
            }
        }
        let href = toc_href(&relative_to(ncx_dir, path), frag);
        s.push_str(&format!(r#"<navPoint id="np{}" playOrder="{}"><navLabel><text>{}</text></navLabel><content src="{}"/>"#, i + 1, i + 1, xml_escape(t), xml_escape(&href)));
        depth = d;
    }
    for _ in 0..depth {
        s.push_str("</navPoint>");
    }
    s.push_str("</navMap></ncx>");
    s
}

fn build_nav(items: &[(u8, String, String, String)], nav_dir: &str) -> String {
    let ranks = dense_ranks(items);
    let mut s = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops"><head><title>目录</title></head><body><nav epub:type="toc" id="toc"><h1>目录</h1>"#);
    let mut depth = 0u8; // 当前打开的 <ol> 层数
    for (i, (_, t, path, frag)) in items.iter().enumerate() {
        let d = ranks[i].min(depth + 1);
        if d > depth {
            for _ in depth..d {
                s.push_str("<ol>");
            }
        } else {
            for _ in d..depth {
                s.push_str("</li></ol>");
            }
            s.push_str("</li>");
        }
        depth = d;
        let href = toc_href(&relative_to(nav_dir, path), frag);
        s.push_str(&format!(r#"<li><a href="{}">{}</a>"#, xml_escape(&href), xml_escape(t)));
    }
    for _ in 0..depth {
        s.push_str("</li></ol>");
    }
    s.push_str("</nav></body></html>");
    s
}

/// 目录条目的 href：`frag` 为空（正文没有锚点可指，退化条目直接指文件本身）时不带 `#`。
fn toc_href(rel_path: &str, frag: &str) -> String {
    if frag.is_empty() { rel_path.to_string() } else { format!("{rel_path}#{frag}") }
}

/// 标题文本"标题+编号"拆分启发式（EPUB 线原则①：原书标题跟小节/章节编号拼在一行，如「第一章 1」，
/// TOC 要显示成两级——父级标题 + 缩进子级编号）。只在编号看起来像"小节序号"而非"印刷页码残留"时拆：
/// 数字编号要求 ≤99（页码常见三位数以上，且小节编号在同一本书里通常不会突然跳到几十以上）；中文数字编号
/// （〇一二三四五六七八九十百千，常见于章节内小节"之一/之二"变体的「1」以中文数字呈现）不做位数限制，
/// 因为原书不会用中文数字写页码。标题与编号之间的分隔允许普通空格与全角空格（U+3000）。
fn split_numbered_title(title: &str) -> Option<(String, String)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"^(.+?)[ \u{3000}\t]+([0-9]+|[〇一二三四五六七八九十百千]+)$"#).unwrap());
    let c = re.captures(title.trim())?;
    let head = c[1].trim();
    let num = &c[2];
    if head.is_empty() {
        return None;
    }
    if let Ok(n) = num.parse::<u32>() {
        if n == 0 || n > 99 {
            return None; // 三位数以上大概率是印刷页码残留，不是小节编号，原样保留避免拆错
        }
    }
    Some((head.to_string(), num.to_string()))
}

/// 对已收集的标题条目做"标题+编号"拆分：命中的条目拆成父级(标题) + 子级(编号)两条，子级 level = 父级+1、
/// 指向同一锚点（不是两个可跳转目标，只是给 TOC 一个视觉分级，跟 `dense_ranks` 的嵌套机制天然兼容）。
fn split_numbered_titles(items: Vec<(u8, String, String, String)>) -> Vec<(u8, String, String, String)> {
    let mut out = Vec::with_capacity(items.len());
    for (level, title, path, frag) in items {
        match split_numbered_title(&title) {
            Some((head, num)) => {
                out.push((level, head, path.clone(), frag.clone()));
                out.push((level.saturating_add(1), num, path, frag));
            }
            None => out.push((level, title, path, frag)),
        }
    }
    out
}

/// 无任何 h1–h6 语义标题时的兜底 TOC：退化到按 spine 文件边界逐条生成，条目文本取该文件正文首个非空
/// 文本片段（截断），纯图片页/取不到文本则用"正文 N"占位——保证"没有目录的书优化后至少有可用目录"这个
/// 底线，而不是无声放弃。只在**多数** spine 文件确实有可提取文本时才生成，避免给纯图片书（漫画/画册）
/// 灌一堆没有信息量的"正文 N"占目录——那种书更适合交给漫画识别走专门路径，不该占用这条兜底。
fn fallback_spine_toc(entries: &[Entry], spine: &[String], nav_doc: Option<&String>) -> Vec<(u8, String, String, String)> {
    let pages: Vec<&String> = spine.iter().filter(|p| Some(*p) != nav_doc).collect();
    if pages.is_empty() {
        return Vec::new();
    }
    let texts: Vec<Option<String>> = pages
        .iter()
        .map(|p| {
            let e = entries.iter().find(|e| &&e.name == p)?;
            let html = std::str::from_utf8(&e.data).ok()?;
            static BODY: OnceLock<Regex> = OnceLock::new();
            let body_re = BODY.get_or_init(|| Regex::new(r#"(?is)<body\b[^>]*>(.*?)</body>"#).unwrap());
            let inner = body_re.captures(html).map(|c| c[1].to_string()).unwrap_or_default();
            let t = plain_text(&inner);
            if t.is_empty() { None } else { Some(t) }
        })
        .collect();
    let with_text = texts.iter().filter(|t| t.is_some()).count();
    if with_text * 2 < pages.len() {
        return Vec::new(); // 多数页面没有可提取文本(疑似漫画/画册)，不生成兜底目录
    }
    pages.iter().zip(texts.iter()).enumerate().map(|(i, (p, t))| {
        let title = match t {
            Some(s) => s.chars().take(24).collect::<String>(),
            None => format!("正文 {}", i + 1),
        };
        (1u8, title, (*p).clone(), String::new())
    }).collect()
}

/// OPF 的 `unique-identifier` 实际取值（`<package unique-identifier="X">` 指向的那个
/// `<dc:identifier id="X">` 元素的文本内容）。EPUB2 规范要求 `toc.ncx` 的 `dtb:uid` 跟这个值
/// 完全一致——真机《疯探》坐实：这本"番茄小说 EPUB Generator"产物的 `toc.ncx` navMap 结构完全
/// 正确（94 条 navPoint 全部可达），但 `dtb:uid` 是生成器随手写的另一个 uuid，跟 OPF 的
/// `dc:identifier` 对不上；reMarkable 原生目录面板遇到这种不匹配**直接不显示目录入口**（不是
/// 显示空列表），换一本 `dtb:uid` 匹配的书（《雪人》）目录入口就在。见 `fix_ncx_uid`。
fn opf_unique_identifier(entries: &[Entry]) -> Option<String> {
    let i = find_opf(entries)?;
    let text = String::from_utf8_lossy(&entries[i].data);
    static PKG: OnceLock<Regex> = OnceLock::new();
    let pkg_re = PKG.get_or_init(|| Regex::new(r#"<package\b[^>]*\bunique-identifier="([^"]+)""#).unwrap());
    let uid_attr = &pkg_re.captures(&text)?[1];
    static ID: OnceLock<Regex> = OnceLock::new();
    let id_re = ID.get_or_init(|| Regex::new(r#"(?s)<dc:identifier\b[^>]*\bid="([^"]+)"[^>]*>([^<]*)</dc:identifier>"#).unwrap());
    id_re.captures_iter(&text).find(|c| &c[1] == uid_attr).map(|c| c[2].trim().to_string())
}

/// 修 `toc.ncx` 的 `dtb:uid` 跟 OPF 实际标识符不一致的问题（见 `opf_unique_identifier` 注释）。
/// 幂等、只在真的不一致时改；OPF 没有可解析的标识符（极少见）时不动。
fn fix_ncx_uid(entries: &mut Vec<Entry>, rep: &mut WashReport) {
    let Some(uid) = opf_unique_identifier(entries) else { return };
    static META: OnceLock<Regex> = OnceLock::new();
    let re = META.get_or_init(|| Regex::new(r#"(<meta\s+name="dtb:uid"\s+content=")[^"]*("\s*/?>)"#).unwrap());
    for e in entries.iter_mut() {
        if e.name.to_ascii_lowercase().ends_with(".ncx") {
            if let Ok(text) = std::str::from_utf8(&e.data) {
                if let Some(c) = re.captures(text) {
                    if c.get(0).map(|m| m.as_str()) != Some(&format!("{}{}{}", &c[1], xml_escape(&uid), &c[2])) {
                        let new = re.replace(text, |c: &regex::Captures| format!("{}{}{}", &c[1], xml_escape(&uid), &c[2])).into_owned();
                        if new != text {
                            e.data = new.into_bytes();
                            rep.ncx_uid_fixed += 1;
                        }
                    }
                }
            }
        }
    }
}

/// xochitl 定位目录文件不是走 EPUB 规范的 `<spine toc="IDREF">`，而是在二进制里**硬编码死查**
/// manifest 里 `id="ncx"` 这个字符串字面量（2026-09-19 反编译 xochitl 二进制坐实：在
/// `GeneratePdfFromEpub` 调用链里直接挖到这个写死的 3 字符哈希查找 key；`<spine toc="...">`
/// 只是这条硬编码查找失败时的一个后备分支，实测这条后备分支没能救回《疯探》——原因未查清，
/// 可能是 OPF 解析阶段没把 `<spine>` 的 `toc` 属性值正确落到后备分支读的那个字段）。《疯探》的
/// `<item id="toc" href="toc.ncx" .../>` + `<spine toc="toc">` 完全符合规范，但因为 manifest
/// id 不叫 "ncx"，navMap 里的标题全部提取失败、原生目录入口整个不出现（书本身照常能翻页——
/// 页面渲染走另一条不依赖这个 id 的路径）。真机验证：拿真实 content.opf 原封不动，只把这一个
/// id 从 "toc" 改成 "ncx"（`<spine toc="...">` 同步改，否则 idref 悬空），94 条章节标题全部
/// 恢复（`.epubindex` 从 7188 字节涨到 15558 字节）。幂等；已经叫 "ncx"、或跟另一条目 id 冲突
/// （改了会撞车，极罕见）时不动。
fn fix_ncx_manifest_id(entries: &mut Vec<Entry>, rep: &mut WashReport) {
    let Some(opf) = parse_opf(entries) else { return };
    if opf.items.contains_key("ncx") {
        return; // 已经叫 ncx，或者已有另一条目占了这个 id——两种情况都不该动
    }
    let text = String::from_utf8_lossy(&entries[opf.index].data).into_owned();
    static ITEM: OnceLock<Regex> = OnceLock::new();
    let item_re = ITEM.get_or_init(|| Regex::new(r#"(?s)<item\b[^>]*\bmedia-type="application/x-dtbncx\+xml"[^>]*/?>"#).unwrap());
    let Some(m) = item_re.find(&text) else { return };
    static IDATTR: OnceLock<Regex> = OnceLock::new();
    let id_re = IDATTR.get_or_init(|| Regex::new(r#"\bid="([^"]+)""#).unwrap());
    let Some(idc) = id_re.captures(m.as_str()) else { return };
    let old_id = idc[1].to_string();
    if old_id == "ncx" {
        return;
    }
    let new_tag = m.as_str().replacen(&format!(r#"id="{old_id}""#), r#"id="ncx""#, 1);
    let mut new_text = text.clone();
    new_text.replace_range(m.range(), &new_tag);
    // <spine toc="OLD_ID"> 同步改，不然这个属性从此指向一个不存在的 id（没有这个属性的书——极少
    // 见——说明它压根没靠 spine 的 toc 属性定位目录，不用管）。
    static SPINE_TOC: OnceLock<Regex> = OnceLock::new();
    let spine_re = SPINE_TOC.get_or_init(|| Regex::new(r#"(<spine\b[^>]*\btoc=")([^"]+)(")"#).unwrap());
    if let Some(c) = spine_re.captures(&new_text) {
        if &c[2] == old_id {
            let whole = c.get(0).unwrap();
            let replaced = format!("{}ncx{}", &c[1], &c[3]);
            let range = whole.range();
            new_text.replace_range(range, &replaced);
        }
    }
    entries[opf.index].data = new_text.into_bytes();
    rep.ncx_manifest_id_fixed += 1;
}

/// 剥 `toc.ncx` 里指向外部 DTD 的 `<!DOCTYPE ncx PUBLIC "..." "http://www.daisy.org/...dtd">` 声明
/// （2026-09-19 真机对照《疯探》vs《雪人》坐实的第二个差异——dtb:uid 修一致后原生目录入口仍然不见，
/// 两本书剩下的结构性区别就是这条：《疯探》的 `toc.ncx` 带这个外部 DTD 引用，《雪人》没有，也没有
/// 任何其它 reader/工具要求 NCX 必须带 DOCTYPE 才能解析——它纯粹是历史遗留的验证声明。真机是
/// USB/WiFi 隧道环境，如果 xochitl 的 XML 解析器老实去联网取这个外部 DTD，离线或路由不通时很可能
/// 卡住/超时/直接判整份 NCX 不可用，原生目录入口因此消失，但书本身照常能读——不影响 spine 阅读，
/// 只影响"目录"这个附加功能，症状完全吻合。剥掉不改变 NCX 的任何实际语义，纯粹去掉这个外部依赖，
/// `build_ncx` 自己生成的 NCX 也从来不带 DOCTYPE，这里是让已有 NCX 向那个已经验证过没问题的形态看齐。
fn strip_ncx_doctype(entries: &mut Vec<Entry>, rep: &mut WashReport) {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"(?is)<!DOCTYPE\s+ncx\b[^>]*>\s*"#).unwrap());
    for e in entries.iter_mut() {
        if e.name.to_ascii_lowercase().ends_with(".ncx") {
            if let Ok(text) = std::str::from_utf8(&e.data) {
                if re.is_match(text) {
                    e.data = re.replace(text, "").into_owned().into_bytes();
                    rep.ncx_doctype_stripped += 1;
                }
            }
        }
    }
}

/// OPF `<dc:title>` 的纯文本内容，取不到时兜底"目录"。
fn opf_book_title(entries: &[Entry], opf_index: usize) -> String {
    let t = String::from_utf8_lossy(&entries[opf_index].data);
    static T: OnceLock<Regex> = OnceLock::new();
    T.get_or_init(|| Regex::new(r#"(?s)<dc:title[^>]*>(.*?)</dc:title>"#).unwrap()).captures(&t).map(|c| plain_text(&c[1])).unwrap_or_else(|| "目录".into())
}

/// 分部标题前缀："第X部/卷/篇/辑"（X 为阿拉伯数字或中文数字），后面可能紧跟同一条目剩下的文本
/// （如"第一部　01　雪人"里"01　雪人"是这条目自己的章节标识，不是下一条的）。
fn part_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"^(第[0-9〇一二三四五六七八九十百千]+[部卷篇辑])[ \u{3000}\t]*(.*)$"#).unwrap())
}

/// 从已有 `toc.ncx` 的 navMap 里按文档顺序拍平抽取 (标题, content src) ——不管当前层级，只服务
/// `restructure_existing_toc_parts` 这种"重新看一眼已有目录内容决定要不要升级结构"的场景。
fn ncx_navpoint_titles_and_targets(ncx_text: &str) -> Vec<(String, String)> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"(?s)<navLabel>\s*<text>(.*?)</text>\s*</navLabel>\s*<content\s+src="([^"]+)"\s*/?>"#).unwrap());
    re.captures_iter(ncx_text).map(|c| (plain_text(&c[1]), c[2].to_string())).collect()
}

/// 书自带的扁平目录如果符合"第X部　编号　章名"这种排版惯例（分部标题只在每部第一条出现、其余
/// 条目隐式归属该部——真机《雪人》坐实：reMarkable 原生目录面板显示的是完全扁平的列表，"01 雪人"
/// 没有嵌在"第一部"下面），重建成两级：分部标题单独成一条父级（沿用该条目自己的跳转目标——分部
/// 标题这条本身就是这部的开篇章节，能跳）；分部前缀后剩下的文本（如"01　雪人"）连同后续不带
/// 前缀的条目一起降一级当子级。**一条"第X部"前缀都没匹配到＝原样不动**——不是所有书都用这种
/// 排版惯例，没信号时贸然重建有误伤风险，见 §03az。
fn restructure_existing_toc_parts(entries: &mut Vec<Entry>, mode: AutoToc, rep: &mut WashReport) {
    if mode == AutoToc::Off {
        return;
    }
    let Some(opf) = parse_opf(entries) else { return };
    let Some(ncx_path) = opf.ncx.clone() else { return };
    let Some(e) = entries.iter().find(|e| e.name == ncx_path) else { return };
    let Ok(ncx_text) = std::str::from_utf8(&e.data) else { return };
    let flat = ncx_navpoint_titles_and_targets(ncx_text);
    let re = part_prefix_re();
    if flat.len() < 2 || !flat.iter().any(|(t, _)| re.is_match(t)) {
        return;
    }
    let ncx_dir = dir_of(&ncx_path).to_string();
    let mut items: Vec<(u8, String, String, String)> = Vec::with_capacity(flat.len());
    let mut in_part = false;
    for (title, src) in &flat {
        let (raw_path, frag) = match src.split_once('#') {
            Some((p, f)) => (p, f.to_string()),
            None => (src.as_str(), String::new()),
        };
        let path = resolve(&ncx_dir, &percent_decode(raw_path));
        if let Some(c) = re.captures(title) {
            items.push((1, c[1].to_string(), path.clone(), frag.clone()));
            let rest = c[2].trim();
            if !rest.is_empty() {
                items.push((2, rest.to_string(), path, frag));
            }
            in_part = true;
        } else {
            items.push((if in_part { 2 } else { 1 }, title.clone(), path, frag));
        }
    }
    let title = opf_book_title(entries, opf.index);
    let uid = opf_unique_identifier(entries).unwrap_or_else(|| WASH_MARK.to_string());
    let ncx = build_ncx(&items, &ncx_dir, &title, &uid).into_bytes();
    if let Some(idx) = entries.iter().position(|e| e.name == ncx_path) {
        entries[idx].data = ncx;
    }
    if let Some(nav_path) = opf.nav_doc.clone() {
        let nav = build_nav(&items, dir_of(&nav_path)).into_bytes();
        if let Some(idx) = entries.iter().position(|e| e.name == nav_path) {
            entries[idx].data = nav;
        }
    }
    rep.toc_parts_restructured = items.len();
}

fn auto_toc(entries: &mut Vec<Entry>, mode: AutoToc, rep: &mut WashReport) {
    if mode == AutoToc::Off || (mode == AutoToc::IfMissing && toc_entry_count(entries) > 0) {
        return;
    }
    let Some(opf) = parse_opf(entries) else { return };
    let headings = collect_headings(entries, &opf.spine, opf.nav_doc.as_ref());
    let headings = if headings.is_empty() { fallback_spine_toc(entries, &opf.spine, opf.nav_doc.as_ref()) } else { split_numbered_titles(headings) };
    if headings.is_empty() {
        return;
    }
    let title = opf_book_title(entries, opf.index);
    let ncx_path = opf.ncx.clone().unwrap_or_else(|| resolve(&opf.dir, "toc.ncx"));
    let nav_path = opf.nav_doc.clone().unwrap_or_else(|| resolve(&opf.dir, "nav.xhtml"));
    let uid = opf_unique_identifier(entries).unwrap_or_else(|| WASH_MARK.to_string());
    let ncx = build_ncx(&headings, dir_of(&ncx_path), &title, &uid).into_bytes();
    let nav = build_nav(&headings, dir_of(&nav_path)).into_bytes();
    let mut text = String::from_utf8_lossy(&entries[opf.index].data).into_owned();
    if opf.ncx.is_none() {
        // id 必须叫 "ncx"（不是随便起的标记）——xochitl 定位目录文件靠二进制里硬编码死查这个
        // 字符串字面量，见 `fix_ncx_manifest_id` 的注释。
        text = text.replacen("</manifest>", &format!(r#"<item id="ncx" href="{}" media-type="application/x-dtbncx+xml"/></manifest>"#, relative_to(&opf.dir, &ncx_path)), 1);
        static SPINE: OnceLock<Regex> = OnceLock::new();
        let sp = SPINE.get_or_init(|| Regex::new(r#"<spine\b([^>]*)>"#).unwrap());
        text = sp.replace(&text, |c: &regex::Captures| {
            let attrs = c[1].to_string();
            if attrs.contains("toc=") {
                format!("<spine{attrs}>")
            } else {
                format!("<spine{attrs} toc=\"ncx\">")
            }
        }).into_owned();
    }
    if opf.nav_doc.is_none() {
        text = text.replacen("</manifest>", &format!(r#"<item id="cj-nav" href="{}" media-type="application/xhtml+xml" properties="nav"/></manifest>"#, relative_to(&opf.dir, &nav_path)), 1);
    }
    entries[opf.index].data = text.into_bytes();
    for (path, data) in [(ncx_path, ncx), (nav_path, nav)] {
        match entries.iter_mut().find(|e| e.name == path) {
            Some(e) => e.data = data,
            None => entries.push(Entry { name: path, data }),
        }
    }
    rep.toc_generated = headings.len();
}

// ───────────────────────── 入口 ─────────────────────────

/// 全书 CJK vs 拉丁字符占比 → 主语言（Han 字数 ≥ 拉丁字母数 = Cjk）。扫全部 html 正文，早停够量即定。
fn detect_dominant_script(entries: &[Entry]) -> LangMode {
    let (mut han, mut latin) = (0u64, 0u64);
    for e in entries.iter().filter(|e| is_html(&e.name) && !is_toc_file(&e.name)) {
        let Ok(t) = std::str::from_utf8(&e.data) else { continue };
        for ch in plain_text(t).chars() {
            if matches!(ch, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}') {
                han += 1;
            } else if ch.is_ascii_alphabetic() {
                latin += 1;
            }
        }
        if han + latin > 20_000 {
            break; // 够量即判，不必扫全书
        }
    }
    if han >= latin {
        LangMode::Cjk
    } else {
        LangMode::Latin
    }
}

/// 对条目表就地清洗。真 DRM 返回 Err（调用方应整体失败、原样不动）。
pub fn wash_entries(entries: &mut Vec<Entry>, opts: &WashOpts) -> Result<WashReport, String> {
    let mut rep = WashReport::default();
    strip_pseudo_drm(entries, &mut rep)?;
    remove_empty_pages(entries, &mut rep);
    // Auto → 探测主语言，解析成具体 Cjk/Latin 再逐文件注排版（探测在剥空页之后、注样式之前）。
    let opts = if opts.lang == LangMode::Auto {
        let mut o = opts.clone();
        o.lang = detect_dominant_script(entries);
        o
    } else {
        opts.clone()
    };
    let opts = &opts;
    // 外链 wash css 的 zip 路径（放 OPF 同目录；无 OPF 兜底放根）。排版规则写这里、逐 html 加 <link>——
    // xochitl 只认外链 css（内联 <style> 无视），见 WASH_CSS_NAME 注。
    let css_path = match find_opf(entries) {
        Some(i) => {
            let d = dir_of(&entries[i].name);
            if d.is_empty() { WASH_CSS_NAME.to_string() } else { format!("{d}/{WASH_CSS_NAME}") }
        }
        None => WASH_CSS_NAME.to_string(),
    };
    for e in entries.iter_mut() {
        let l = e.name.to_ascii_lowercase();
        if l.ends_with(".css") {
            if let Ok(t) = std::str::from_utf8(&e.data) {
                e.data = filter_css(t, opts).into_bytes();
                rep.css_files += 1;
            }
        } else if is_html(&e.name) && !is_toc_file(&e.name) {
            if let Ok(t) = std::str::from_utf8(&e.data) {
                let (out, dups) = wash_html(t, opts);
                let href = relative_to(dir_of(&e.name), &css_path);
                let out = inject_css_link(&out, &href);
                rep.dup_id_tags_collapsed += dups;
                e.data = out.into_bytes();
                rep.html_files += 1;
            }
        }
    }
    add_wash_css_entry(entries, &css_path, &wash_css(opts));
    fix_ncx_manifest_id(entries, &mut rep);
    restructure_existing_toc_parts(entries, opts.auto_toc, &mut rep);
    auto_toc(entries, opts.auto_toc, &mut rep);
    fix_ncx_uid(entries, &mut rep);
    strip_ncx_doctype(entries, &mut rep);
    Ok(rep)
}

/// 新增（或重优化时更新）外链 wash css 文件，并往 OPF manifest 补一条 `<item>`（幂等）。
fn add_wash_css_entry(entries: &mut Vec<Entry>, css_path: &str, content: &str) {
    if let Some(e) = entries.iter_mut().find(|e| e.name == css_path) {
        e.data = content.as_bytes().to_vec();
    } else {
        entries.push(Entry { name: css_path.to_string(), data: content.as_bytes().to_vec() });
    }
    if let Some(oi) = find_opf(entries) {
        let opf_dir = dir_of(&entries[oi].name).to_string();
        let href = relative_to(&opf_dir, css_path);
        let mut text = String::from_utf8_lossy(&entries[oi].data).into_owned();
        if !text.contains(&format!("href=\"{href}\"")) {
            if let Some(p) = text.find("</manifest>") {
                text.insert_str(p, &format!("<item id=\"cangjie-wash-css\" href=\"{href}\" media-type=\"text/css\"/>"));
                entries[oi].data = text.into_bytes();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, data: &str) -> Entry {
        Entry { name: name.into(), data: data.as_bytes().to_vec() }
    }
    fn s(entries: &[Entry], name: &str) -> String {
        String::from_utf8(entries.iter().find(|e| e.name == name).unwrap().data.clone()).unwrap()
    }
    const OPF: &str = r#"<?xml version="1.0"?><package version="2.0"><metadata><dc:title>测试书</dc:title></metadata><manifest><item id="css" href="style.css" media-type="text/css"/><item id="dk" href="dkagent.css" media-type="text/css"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="pb" href="pb.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="pb"/><itemref idref="c2"/></spine></package>"#;

    #[test]
    fn paths() {
        assert_eq!(posix_norm("OEBPS/../a/./b"), "a/b");
        assert_eq!(resolve("OEBPS/text", "../style.css"), "OEBPS/style.css");
        assert_eq!(relative_to("OEBPS", "OEBPS/text/c1.xhtml"), "text/c1.xhtml");
        assert_eq!(relative_to("OEBPS/text", "OEBPS/style.css"), "../style.css");
        assert_eq!(relative_to("", "a.xhtml"), "a.xhtml");
        assert_eq!(percent_decode("%E5%AD%97.xhtml"), "字.xhtml");
    }

    #[test]
    fn decl_filter_and_spacing() {
        let f: Vec<String> = DEFAULT_FILTER_PROPS.iter().map(|s| s.to_string()).collect();
        assert_eq!(filter_decls("font-family:'A';color:#333;text-indent:1em;font:12px x", &f, Spacing::Keep), "color:#333;text-indent:1em;", "color 不再剥离（EPUB 线保留原书颜色）");
        assert_eq!(filter_decls("margin:1em 2em;padding-top:3px;padding-left:4px", &f, Spacing::Vertical), "margin:0 2em;padding-left:4px;");
        assert_eq!(filter_decls("margin:1em 2em 3em 4em", &f, Spacing::Vertical), "margin:0 2em 0 4em;");
        assert_eq!(filter_decls("margin:5pt", &f, Spacing::Vertical), "margin:0 5pt;");
        assert_eq!(filter_decls("margin:5pt;padding:2px;line-height:1.5", &f, Spacing::All), "line-height:1.5;");
        assert_eq!(filter_decls("font-family: &#39;A&#39;; text-indent:2em", &f, Spacing::Keep), "text-indent:2em;", "实体分号不截断");
        assert_eq!(selector_spacing("p.calibre1"), Spacing::Vertical);
        assert_eq!(selector_spacing("div > p"), Spacing::Vertical);
        assert_eq!(selector_spacing("body"), Spacing::All);
        assert_eq!(selector_spacing("@page"), Spacing::All);
        assert_eq!(selector_spacing(".calibre1"), Spacing::Keep);
        assert_eq!(selector_spacing("span, pre"), Spacing::Keep);
    }

    #[test]
    fn css_file_filter_keeps_font_face_and_media() {
        let o = WashOpts::default();
        let css = "@font-face{font-family:X;src:url(x.ttf)} body{margin:5pt;color:#333} p{margin:1em 0;text-align:justify;text-indent:0} @media print{ p{margin-top:2em} } .c{margin:1em}";
        let out = filter_css(css, &o);
        assert!(out.contains("@font-face{font-family:X;src:url(x.ttf)}"), "{out}");
        assert!(out.contains("body{color:#333;}"), "margin 因 Spacing::All 剥、color 保留(不再剥): {out}");
        assert!(out.contains(" p{margin:0 0;text-align:justify;text-indent:0;}"), "text-align 保留(不再剥): {out}");
        assert!(out.contains("p{}"), "media 内规则也处理(单独 margin-top 整条丢弃，与 color/text-align 剥离与否无关): {out}");
        assert!(out.contains(".c{margin:1em;}"), "类选择器不动: {out}");
        let k = WashOpts { keep_para_spacing: true, ..Default::default() };
        assert!(filter_css("p{margin:1em 0}", &k).contains("p{margin:1em 0;}"));
    }

    #[test]
    fn strips_background_image_keeps_font_src() {
        let o = WashOpts::default();
        // 分卷页背景图（xochitl 平铺盖正文）应剥；@font-face 的 src:url 保留。
        let css = "@font-face{font-family:F;src:url(f.ttf)} body.fen{background:url(bg.png) no-repeat bottom center;background-size:100% auto;margin:0} .x{background-image:url(y.png);color:#333;text-indent:2em}";
        let out = filter_css(css, &o);
        assert!(!out.contains("bg.png") && !out.contains("y.png"), "背景图应剥: {out}");
        assert!(out.contains("f.ttf"), "@font-face src 保留: {out}");
        assert!(out.contains("text-indent:2em"), "非背景声明保留: {out}");
    }

    #[test]
    fn html_wash_filters_styles_and_collapses_dup_ids() {
        let o = WashOpts::default();
        let html = r#"<html><head><link rel="stylesheet" href="s.css"/></head><body style="margin:5pt"><p id="a" id="b" style="font-size:12px;margin-top:1em;margin-left:2em">x</p><div style="color:gray">y</div><style>p{color:#333;margin:1em}</style></body></html>"#;
        let (out, dups) = wash_html(html, &o);
        assert_eq!(dups, 1);
        assert!(out.contains(r#"<p id="a" style="margin-left:2em;">x</p>"#), "{out}");
        assert!(out.contains(r#"<div style="color:gray;">y</div>"#), "color 不再剥离、style 非空保留: {out}");
        assert!(out.contains("<body>"), "{out}");
        assert!(out.contains("<style>p{color:#333;margin:0 1em;}</style>"), "color 保留 + 1em 四边→上下归零左右保留: {out}");
        // wash_html 不再注入内联 <style>（排版规则改外链 css，由 wash_entries 注）——见 external_css_injected_and_linked。
        assert!(!out.contains(&format!(r#"class="{WASH_MARK}""#)), "不该再注入内联 cj-wash: {out}");
        // 旧版内联 cj-wash 块重洗时清掉
        let (cleaned, _) = wash_html(r#"<html><head><style class="cj-wash">p{text-indent:2em!important}</style></head><body><p>z</p></body></html>"#, &o);
        assert!(!cleaned.contains("cj-wash") && !cleaned.contains("text-indent"), "旧内联块未清: {cleaned}");
    }

    #[test]
    fn pseudo_drm_stripped_and_real_drm_rejected() {
        let mut v = vec![
            e("mimetype", "application/epub+zip"),
            e("META-INF/encryption.xml", r#"<encryption><EncryptedData><CipherData><CipherReference URI="dkagent.css"/></CipherData></EncryptedData></encryption>"#),
            e("content.opf", OPF),
            e("dkagent.css", "secret"),
            e("style.css", "p{}"),
            e("c1.xhtml", "<html><body><p>a</p></body></html>"),
            e("pb.xhtml", "<html><body><p>b</p></body></html>"),
            e("c2.xhtml", "<html><body><p>c</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.pseudo_drm_stripped, vec!["dkagent.css"]);
        assert!(!v.iter().any(|x| x.name == "dkagent.css" || x.name == "META-INF/encryption.xml"));
        assert!(!s(&v, "content.opf").contains("dkagent.css"), "manifest 项删除");
        let mut real = vec![e("META-INF/encryption.xml", r#"<CipherReference URI="OEBPS/c1.xhtml"/><CipherReference URI="a.ttf"/>"#), e("OEBPS/c1.xhtml", "")];
        let err = wash_entries(&mut real, &WashOpts::default()).unwrap_err();
        assert!(err.contains("真 DRM") && err.contains("OEBPS/c1.xhtml"), "{err}");
    }

    #[test]
    fn empty_page_removed_and_toc_retargeted() {
        let mut v = vec![
            e("content.opf", OPF),
            e("style.css", ""),
            e("dkagent.css", ""),
            e("toc.ncx", r#"<ncx><navMap><navPoint><content src="c1.xhtml"/></navPoint><navPoint><content src="pb.xhtml#x"/></navPoint></navMap></ncx>"#),
            e("c1.xhtml", "<html><body><p>a</p></body></html>"),
            e("pb.xhtml", r#"<html><body><div class="mbppagebreak"></div>&nbsp;</body></html>"#),
            e("c2.xhtml", "<html><body><p>c</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts { auto_toc: AutoToc::Off, ..Default::default() }).unwrap();
        assert_eq!(rep.empty_pages_removed, vec!["pb.xhtml"]);
        assert!(!v.iter().any(|x| x.name == "pb.xhtml"));
        let opf = s(&v, "content.opf");
        assert!(!opf.contains(r#"idref="pb""#) && !opf.contains(r#"id="pb""#), "{opf}");
        assert!(s(&v, "toc.ncx").contains(r#"src="c2.xhtml""#), "指向空页的目录改指下一篇: {}", s(&v, "toc.ncx"));
        // 有图的页不算空
        assert!(!is_empty_page(r#"<html><body><img src="a.png"/></body></html>"#));
    }

    #[test]
    fn auto_toc_generated_only_when_missing() {
        let mk = || vec![
            e("OEBPS/content.opf", r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="c1" href="text/c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="text/c2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#),
            e("OEBPS/text/c1.xhtml", "<html><body><h1>第一章</h1><p>a</p><h2 id=\"s1\">一节</h2></body></html>"),
            e("OEBPS/text/c2.xhtml", "<html><body><h1>第<i>二</i>章</h1><p>b</p></body></html>"),
        ];
        let mut v = mk();
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.toc_generated, 3);
        let ncx = s(&v, "OEBPS/toc.ncx");
        assert!(ncx.contains(r#"src="text/c1.xhtml#cj-toc-1""#) && ncx.contains(r#"src="text/c1.xhtml#s1""#) && ncx.contains("第二章"), "{ncx}");
        let nav = s(&v, "OEBPS/nav.xhtml");
        assert!(nav.contains(r#"<li><a href="text/c1.xhtml#cj-toc-1">第一章</a><ol><li><a href="text/c1.xhtml#s1">一节</a></li></ol></li>"#), "{nav}");
        let opf = s(&v, "OEBPS/content.opf");
        assert!(opf.contains(r#"toc="ncx""#) && opf.contains(r#"properties="nav""#), "{opf}");
        assert!(s(&v, "OEBPS/text/c1.xhtml").contains(r#"<h1 id="cj-toc-1">"#));
        assert_eq!(toc_entry_count(&v), 6, "ncx 3 + nav 3");
        // 已有目录 → IfMissing 不动
        let mut w = mk();
        w.push(e("OEBPS/toc.ncx", r#"<ncx><navMap><navPoint><content src="text/c1.xhtml"/></navPoint></navMap></ncx>"#));
        let rep = wash_entries(&mut w, &WashOpts::default()).unwrap();
        assert_eq!(rep.toc_generated, 0);
        assert!(!w.iter().any(|x| x.name == "OEBPS/nav.xhtml"));
    }

    #[test]
    fn existing_ncx_dtb_uid_synced_to_opf_identifier() {
        // 真机回归（2026-09-19，《疯探》）：navMap 结构完全正确，但 dtb:uid 是第三方生成器随手写的
        // 另一个 uuid，跟 OPF 的 dc:identifier 对不上——reMarkable 原生目录面板遇到这种不匹配
        // 直接不显示目录入口（不是空列表），换一本 dtb:uid 匹配的书目录入口就在。
        let opf = r#"<package version="2.0" unique-identifier="bookid"><metadata><dc:title>书</dc:title><dc:identifier id="bookid">urn:uuid:real-book-id</dc:identifier></metadata><manifest><item id="toc" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="toc"><itemref idref="c1"/></spine></package>"#;
        let mut v = vec![
            e("content.opf", opf),
            e("toc.ncx", r#"<ncx><head><meta name="dtb:uid" content="urn:uuid:stale-generator-id"/></head><navMap><navPoint><navLabel><text>章一</text></navLabel><content src="c1.xhtml"/></navPoint></navMap></ncx>"#),
            e("c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.ncx_uid_fixed, 1);
        let ncx = s(&v, "toc.ncx");
        assert!(ncx.contains(r#"content="urn:uuid:real-book-id""#), "dtb:uid 该改成跟 OPF 一致: {ncx}");
        assert!(!ncx.contains("stale-generator-id"), "旧的错误 uid 不该残留: {ncx}");
        // 已经一致时不误报、不改动字节（幂等）
        let mut w = vec![
            e("content.opf", opf),
            e("toc.ncx", r#"<ncx><head><meta name="dtb:uid" content="urn:uuid:real-book-id"/></head><navMap><navPoint><navLabel><text>章一</text></navLabel><content src="c1.xhtml"/></navPoint></navMap></ncx>"#),
            e("c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep2 = wash_entries(&mut w, &WashOpts::default()).unwrap();
        assert_eq!(rep2.ncx_uid_fixed, 0, "已经一致不该误报修复过");
    }

    #[test]
    fn ncx_manifest_id_renamed_to_ncx_and_spine_toc_synced() {
        // 真机回归（2026-09-19，《疯探》，反编译 xochitl 二进制坐实）：dtb:uid、DOCTYPE 都修一致
        // 后原生目录入口依然不出现——根因是 xochitl 定位目录文件硬编码死查 manifest 里 id="ncx"，
        // 不是走 `<spine toc="IDREF">`；《疯探》完全合规的 `id="toc"` + `<spine toc="toc">`
        // 因此找不到。只改这一个 id 名字，真机验证 94 条章节标题全部恢复。
        let opf = r#"<package version="2.0"><metadata><dc:title>疯探</dc:title></metadata><manifest><item id="toc" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="text/c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="toc"><itemref idref="c1"/></spine></package>"#;
        let mut v = vec![
            e("content.opf", opf),
            e("toc.ncx", r#"<ncx><navMap><navPoint><navLabel><text>第一章</text></navLabel><content src="text/c1.xhtml"/></navPoint></navMap></ncx>"#),
            e("text/c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.ncx_manifest_id_fixed, 1);
        let opf_out = s(&v, "content.opf");
        assert!(opf_out.contains(r#"<item id="ncx" href="toc.ncx""#), "manifest 里 ncx 条目的 id 该改成 \"ncx\": {opf_out}");
        assert!(opf_out.contains(r#"<spine toc="ncx">"#), "spine 的 toc 属性该同步指向新 id: {opf_out}");
        assert!(!opf_out.contains(r#"id="toc""#), "旧 id 不该残留: {opf_out}");
        // 已经叫 "ncx" 时原样不动、不误报（幂等）
        let already_ok = r#"<package version="2.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="ncx"><itemref idref="c1"/></spine></package>"#;
        let mut w = vec![
            e("content.opf", already_ok),
            e("toc.ncx", r#"<ncx><navMap><navPoint><navLabel><text>第一章</text></navLabel><content src="c1.xhtml"/></navPoint></navMap></ncx>"#),
            e("c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep2 = wash_entries(&mut w, &WashOpts::default()).unwrap();
        assert_eq!(rep2.ncx_manifest_id_fixed, 0, "已经叫 ncx 不该误报修复过");
    }

    #[test]
    fn ncx_external_doctype_stripped() {
        // 真机回归（2026-09-19，dtb:uid 修一致后原生目录入口仍不出现）：《疯探》"番茄小说 EPUB
        // Generator" 产物的 toc.ncx 带外部 DTD 引用（daisy.org），《雪人》没有——这是两本书唯一
        // 剩下的结构性差异。剥掉不改变 NCX 语义，只去掉这个外部依赖。
        let ncx_with_doctype = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE ncx PUBLIC \"-//NISO//DTD ncx 2005-1//EN\" \"http://www.daisy.org/z3986/2005/ncx-2005-1.dtd\">\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\"><navMap><navPoint><navLabel><text>章一</text></navLabel><content src=\"c1.xhtml\"/></navPoint></navMap></ncx>";
        let mut v = vec![
            e("content.opf", r#"<package version="2.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="toc" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="toc"><itemref idref="c1"/></spine></package>"#),
            e("toc.ncx", ncx_with_doctype),
            e("c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.ncx_doctype_stripped, 1);
        let ncx = s(&v, "toc.ncx");
        assert!(!ncx.to_ascii_uppercase().contains("DOCTYPE"), "DOCTYPE 该被剥掉: {ncx}");
        assert!(ncx.contains("<navPoint>") || ncx.contains(r#"<content src="c1.xhtml"/>"#), "navMap 内容不该被动: {ncx}");
        // 没有 DOCTYPE 的书原样不动、不误报
        let mut w = vec![
            e("content.opf", r#"<package version="2.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="toc" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="toc"><itemref idref="c1"/></spine></package>"#),
            e("toc.ncx", r#"<ncx><navMap><navPoint><navLabel><text>章一</text></navLabel><content src="c1.xhtml"/></navPoint></navMap></ncx>"#),
            e("c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep2 = wash_entries(&mut w, &WashOpts::default()).unwrap();
        assert_eq!(rep2.ncx_doctype_stripped, 0, "没有 DOCTYPE 不该误报剥过");
    }

    #[test]
    fn existing_flat_toc_with_part_prefix_restructured_into_two_levels() {
        // 真机回归（2026-09-19，《雪人》）：书自带扁平 toc.ncx，条目形如"第一部　01　雪人"
        // （首条，部+编号+章名）/"　02　卵石眼"（后续，只有编号+章名，隐式归属同一部）——原生
        // 目录面板显示的是完全扁平的列表，"01 雪人"没有嵌在"第一部"下面。
        let opf = r#"<package version="2.0" unique-identifier="BookId"><metadata><dc:title>雪人</dc:title><dc:identifier id="BookId">www.haodoo.net</dc:identifier></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="2.xhtml" media-type="application/xhtml+xml"/><item id="c10" href="10.xhtml" media-type="application/xhtml+xml"/><item id="c11" href="11.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="ncx"><itemref idref="c1"/><itemref idref="c2"/><itemref idref="c10"/><itemref idref="c11"/></spine></package>"#;
        let ncx = r#"<ncx><head><meta name="dtb:uid" content="www.haodoo.net"/></head><navMap>
<navPoint><navLabel><text>第一部　01　雪人</text></navLabel><content src="1.xhtml"/></navPoint>
<navPoint><navLabel><text>　02　卵石眼</text></navLabel><content src="2.xhtml"/></navPoint>
<navPoint><navLabel><text>第二部　10　粉筆</text></navLabel><content src="10.xhtml"/></navPoint>
<navPoint><navLabel><text>　11　死亡面具</text></navLabel><content src="11.xhtml"/></navPoint>
</navMap></ncx>"#;
        let mut v = vec![
            e("content.opf", opf),
            e("toc.ncx", ncx),
            e("1.xhtml", "<html><body><p>正文</p></body></html>"),
            e("2.xhtml", "<html><body><p>正文</p></body></html>"),
            e("10.xhtml", "<html><body><p>正文</p></body></html>"),
            e("11.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.toc_parts_restructured, 6, "2 个分部标题 + 4 条章节＝6 条");
        let out = s(&v, "toc.ncx");
        // 分部标题单独成父级、指向自己这条的目标（第一部开篇就是 1.xhtml）
        assert!(out.contains(r#"<text>第一部</text></navLabel><content src="1.xhtml"/>"#), "{out}");
        // "01 雪人" 降一级挂在"第一部"下面，同样指向 1.xhtml（分部标题那条自己也是这一章）；
        // plain_text 会把全角空格归一成半角（split_whitespace 统一处理，跟标题里其它空白一视同仁）
        assert!(out.contains(r#"<text>01 雪人</text></navLabel><content src="1.xhtml"/>"#), "{out}");
        // "02 卵石眼"（原来没有分部前缀）也降一级，挂在当前活跃的"第一部"下
        assert!(out.contains(r#"<text>02 卵石眼</text></navLabel><content src="2.xhtml"/>"#), "{out}");
        assert!(out.contains(r#"<text>第二部</text></navLabel><content src="10.xhtml"/>"#), "{out}");
        assert!(out.contains(r#"<text>10 粉筆</text></navLabel><content src="10.xhtml"/>"#), "{out}");
        // dtb:depth 该反映真的有两层
        assert!(out.contains(r#"<meta name="dtb:depth" content="2""#), "{out}");
        // 没有分部信号的书原样不动（不该被误伤）
        let opf2 = r#"<package version="2.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="ncx"><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#;
        let ncx2 = r#"<ncx><navMap><navPoint><navLabel><text>楔子</text></navLabel><content src="c1.xhtml"/></navPoint><navPoint><navLabel><text>尾声</text></navLabel><content src="c2.xhtml"/></navPoint></navMap></ncx>"#;
        let mut w = vec![e("content.opf", opf2), e("toc.ncx", ncx2), e("c1.xhtml", "<html><body><p>a</p></body></html>"), e("c2.xhtml", "<html><body><p>b</p></body></html>")];
        let rep2 = wash_entries(&mut w, &WashOpts::default()).unwrap();
        assert_eq!(rep2.toc_parts_restructured, 0, "没有'第X部'前缀不该重建");
    }

    #[test]
    fn lang_aware_indent() {
        // 只用裸 p{} 元素选择器（xochitl 解析器脆）、不带 !important（xochitl 吃不下）；中文 2em / 拉丁 1.2em。
        let cjk = wash_css(&WashOpts { lang: LangMode::Cjk, ..Default::default() });
        assert!(cjk.contains("text-indent:2em;") && !cjk.contains("1.2em") && !cjk.contains("!important"));
        assert!(cjk.starts_with("p{") && !cjk.contains(',') && !cjk.contains('+') && !cjk.contains('@'), "禁用复杂/逗号选择器: {cjk}");
        assert!(cjk.contains("padding-bottom:0;}"), "每条规则以分号收尾（xochitl 丢最后一个无分号声明）: {cjk}");
        let lat = wash_css(&WashOpts { lang: LangMode::Latin, ..Default::default() });
        assert!(lat.contains("text-indent:1.2em") && !lat.contains("!important"));
        // keep_para_spacing 时不归零段距
        let keep = wash_css(&WashOpts { keep_para_spacing: true, ..Default::default() });
        assert!(!keep.contains("margin-top:0") && keep.contains("p{text-indent:2em;}"), "keep-spacing 也要尾分号: {keep}");
    }

    #[test]
    fn figure_and_figcaption_margin_zeroed_as_separate_bare_rules() {
        // 2026-09-10 真机 aeon.co 网文复现：figure/figcaption 默认边距没清零，图片夹在正文中间
        // 造成留白。修法＝跟 p 一样清零，但必须各自一条裸元素选择器规则——xochitl 解析器脆，
        // `figure,figcaption{}` 这种逗号选择器整条规则会失效（lang_aware_indent 测试断言过这条
        // 红线：不带逗号/复合选择器）。
        let css = wash_css(&WashOpts::default());
        assert!(css.contains("figure{margin:0;padding:0;}"), "{css}");
        assert!(css.contains("figcaption{margin:0;padding:0;}"), "{css}");
        assert!(!css.contains("figure,figcaption") && !css.contains("figcaption,figure"), "禁止逗号选择器: {css}");
        // keep_para_spacing 只管段落呼吸感，不该连带保留图片边距——不管这个档位开没开，figure/figcaption 都清零。
        let keep = wash_css(&WashOpts { keep_para_spacing: true, ..Default::default() });
        assert!(keep.contains("figure{margin:0;padding:0;}") && keep.contains("figcaption{margin:0;padding:0;}"), "{keep}");
    }

    #[test]
    fn external_css_injected_and_linked() {
        // 端到端：英文书 wash 后——排版规则进外链 cangjie-wash.css、每章 <link> 指向它、OPF manifest 补 item。
        let mut v = vec![
            e("OEBPS/content.opf", r#"<package version="3.0"><metadata><dc:title>B</dc:title></metadata><manifest><item id="c1" href="Text/c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#),
            e("OEBPS/Text/c1.xhtml", "<html><head></head><body><h2>Chapter One</h2><p>English prose flowing across the page with many words indeed here</p></body></html>"),
        ];
        wash_entries(&mut v, &WashOpts::default()).unwrap();
        // 外链 css 存在且带拉丁缩进（英文书探测为 Latin）
        let css = s(&v, "OEBPS/cangjie-wash.css");
        assert!(css.contains("text-indent:1.2em") && !css.contains("!important"), "外链 css 应含拉丁缩进、无 !important: {css}");
        // 章节 <link> 相对路径正确（Text/ 下 → ../cangjie-wash.css），且不再有内联 text-indent
        let c1 = s(&v, "OEBPS/Text/c1.xhtml");
        assert!(c1.contains(r#"href="../cangjie-wash.css""#), "章节 link 路径错: {c1}");
        assert!(!c1.contains("text-indent:1.2em"), "通用缩进规则不该内联进 html: {c1}");
        assert!(c1.contains(r#"Chapter One</h2><div class="cj-flush">"#), "拉丁：标题后首段换 div 顶格: {c1}");
        assert!(css.contains(".cj-flush{text-indent:0.01em;margin-top:0;margin-bottom:0;}"), "外链 css 带 cj-flush 规则（0.01em 压继承）且尾分号: {css}");
        // OPF manifest 补了 item（相对 opf 目录 = cangjie-wash.css）
        let opf = s(&v, "OEBPS/content.opf");
        assert!(opf.contains(r#"href="cangjie-wash.css""#) && opf.contains("text/css"), "manifest 未补 item: {opf}");
        // 幂等：重洗不重复加 link / item
        wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(s(&v, "OEBPS/Text/c1.xhtml").matches("cangjie-wash.css").count(), 1, "link 重复");
        assert_eq!(s(&v, "OEBPS/content.opf").matches("cangjie-wash.css").count(), 1, "manifest item 重复");
    }

    #[test]
    fn book_css_indent_harmonized_and_first_para_flush() {
        // 书自带类规则的非零 text-indent 改成本书缩进；0 与负值保留；拉丁标题后首段内联不缩进且幂等
        let lat = WashOpts { lang: LangMode::Latin, ..Default::default() };
        let css = filter_css(".calibre_ {display:block;text-indent:2em;margin:0} .quote{text-indent:0;margin-left:2em} .hang{text-indent:-1.5em} p{text-indent:3%}", &lat);
        assert!(css.contains(".calibre_ {display:block;text-indent:1.2em;margin:0;}"), "{css}");
        assert!(css.contains(".quote{text-indent:0;margin-left:2em;}") && css.contains(".hang{text-indent:-1.5em;}"), "{css}");
        assert!(css.contains("p{text-indent:1.2em;}"), "百分比也算非零: {css}");
        let cjk = WashOpts { lang: LangMode::Cjk, ..Default::default() };
        assert!(filter_css(".calibre_ {text-indent:1.2em}", &cjk).contains("text-indent:2em"));
        let (h, _) = wash_html(r#"<html><body><h1 id="a">T</h1>
<div class="x"><p class="c" style="color:red;text-indent:2em">first</p><p>second</p></div><h2>U</h2><p style="text-indent:0">already</p></body></html>"#, &lat);
        assert!(h.contains(r#"<div class="cj-flush">first</div>"#), "只留 cj-flush、剥 class/style: {h}");
        assert!(h.contains(r#"<p>second</p>"#), "第二段不动: {h}");
        assert_eq!(h.matches("cj-flush").count(), 2, "h1 后与 h2 后各一段: {h}");
        let (h2, _) = wash_html(&h, &lat);
        assert_eq!(h2.matches("cj-flush").count(), 2, "幂等: {h2}");
        let (c, _) = wash_html("<html><body><h1>T</h1><p>x</p></body></html>", &cjk);
        assert!(!c.contains("text-indent:0"), "中文不做首段不缩进: {c}");
    }

    #[test]
    fn cjk_br_book_paragraphized_and_fullwidth_indent_stripped() {
        let cjk = WashOpts { lang: LangMode::Cjk, ..Default::default() };
        // 《人骨拼圖》形态：一章一个 div，h3 + 双 br + 每段全角空格开头、单 br 分段，无 <p>
        let src = "<html><head></head><body class=\"calibre\">\n<div class=\"calibre1\">\n<h3 class=\"calibre3\">14</h3><br class=\"calibre2\"/><br class=\"calibre2\"/>　　這間辦公室高居在曼哈頓下城高處。<br class=\"calibre2\"/>　　「對不起？長官？」<br class=\"calibre2\"/>　　嚴格說來，她不能算是。<br class=\"calibre2\"/></div></body></html>";
        let (h, _) = wash_html(src, &cjk);
        assert!(h.contains("<div class=\"calibre1\">\n<h3 class=\"calibre3\">14</h3>"), "块级标签原样: {h}");
        assert!(h.contains("<p>這間辦公室高居在曼哈頓下城高處。</p><p>「對不起？長官？」</p><p>嚴格說來，她不能算是。</p></div>"), "按 br 段落化且剥全角空格: {h}");
        assert!(!h.contains("<br") && !h.contains('　'), "br 与全角空格都不剩: {h}");
        // 有 <p> 的书：不动 br，但剥段首全角空格/nbsp
        let (h2, _) = wash_html("<html><body><p>　　第一段。</p><p>&#160;&#160;第二段。<br/>换行</p></body></html>", &cjk);
        assert!(h2.contains("<p>第一段。</p><p>第二段。<br/>换行</p>"), "{h2}");
        // 拉丁书不做段落化
        let lat = WashOpts { lang: LangMode::Latin, ..Default::default() };
        let (h3, _) = wash_html("<html><body><div>line one<br/>line two<br/>line three<br/>line four<br/>five</div></body></html>", &lat);
        assert!(h3.contains("line one<br/>line two"), "{h3}");
    }

    #[test]
    fn latin_flush_after_bold_heading_scene_break_and_chapter_start() {
        // 《Tell Me Your Dreams》形态：章名=加粗段落（非 <h>），场景切换=段末双 <br/>，无空段
        let lat = WashOpts { lang: LangMode::Latin, ..Default::default() };
        let src = r#"<html><body><div><p class="calibre_"><a href="x.html#1"><span class="bold"><span class="underline">Chapter Three</span></span></a></p><p class="calibre_"><span class="bold">I</span>N another place, at another time, Alette Peters could have been a successful artist.</p><p class="calibre_">Her father’s voice was blue.</p><p class="calibre_">The sound of running water was gray.<br class="calibre3"/><br class="calibre3"/></p><p class="calibre_">Alette Peters was twenty years old.</p><p class="calibre_">She could be plain-looking.</p><p class="calibre_">* * *</p><p class="calibre_">After the break.</p><p class="calibre_">Still after.</p></div></body></html>"#;
        let (h, _) = wash_html(src, &lat);
        assert_eq!(h.matches("cj-flush").count(), 3, "章首正文 + 双br 后 + * * * 后各一段: {h}");
        assert!(h.contains(r#"<div class="cj-flush"><span class="bold">I</span>N another"#), "章首正文顶格＝换成只带 cj-flush 的 div（章名段本身不算）: {h}");
        assert!(h.contains(r#"<div class="cj-flush">Alette Peters was twenty"#), "双 br 后顶格: {h}");
        assert!(h.contains(r#"<div class="cj-flush">After the break.</div>"#), "* * * 后顶格: {h}");
        assert!(h.contains(r#"<p class="calibre_">Her father"#) && h.contains(r#"<p class="calibre_">Still after"#), "普通续段仍是 p: {h}");
        assert!(h.contains(r#"<p class="calibre_"><a href="x.html#1">"#), "章名段自己不动: {h}");
        // 拿空段当段距的书（空段 > 20%）：空段不算场景分隔
        let spaced = r#"<html><body><p>One.</p><p></p><p>Two.</p><p></p><p>Three.</p><p></p><p>Four.</p></body></html>"#;
        let (h3, _) = wash_html(spaced, &lat);
        assert_eq!(h3.matches("cj-flush").count(), 1, "只有章首一段顶格: {h3}");
        let (h4, _) = wash_html(&h, &lat);
        assert_eq!(h4.matches("cj-flush").count(), 3, "幂等（cj-flush div 当段落参与计数，后一段不被误顶格）: {h4}");
        assert!(h4.contains(r#"<p class="calibre_">Her father"#), "重洗后续段仍不顶格: {h4}");
    }

    #[test]
    fn detect_script() {
        let cjk = vec![e("c.xhtml", "<html><body><p>这是一本中文书籍需要两字缩进的测试内容足够多的汉字</p></body></html>")];
        assert_eq!(detect_dominant_script(&cjk), LangMode::Cjk);
        let en = vec![e("c.xhtml", "<html><body><p>This is an English book with plenty of latin letters here indeed</p></body></html>")];
        assert_eq!(detect_dominant_script(&en), LangMode::Latin);
    }

    #[test]
    fn auto_toc_from_h3_and_deep_nesting() {
        // 只用 h3 当章标题：旧 h1/h2 正则会漏，现在应生成目录
        let mut v = vec![
            e("content.opf", r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#),
            e("c1.xhtml", "<html><body><h3>章一</h3><p>a</p><h3>章二</h3></body></html>"),
        ];
        assert_eq!(wash_entries(&mut v, &WashOpts::default()).unwrap().toc_generated, 2, "h3 也进目录");
        // 多级嵌套 h1>h2>h3>h1
        let items = vec![(1u8, "A".into(), "c.xhtml".into(), "a".into()), (2, "B".into(), "c.xhtml".into(), "b".into()), (3, "C".into(), "c.xhtml".into(), "c".into()), (1, "D".into(), "c.xhtml".into(), "d".into())];
        let nav = build_nav(&items, "");
        assert!(nav.contains(r#"<li><a href="c.xhtml#a">A</a><ol><li><a href="c.xhtml#b">B</a><ol><li><a href="c.xhtml#c">C</a></li></ol></li></ol></li><li><a href="c.xhtml#d">D</a></li></ol>"#), "{nav}");
        let ncx = build_ncx(&items, "", "T", "cj-wash");
        assert!(ncx.contains(r#"<navPoint id="np1" playOrder="1"><navLabel><text>A</text></navLabel><content src="c.xhtml#a"/><navPoint id="np2""#), "{ncx}");
        assert!(ncx.contains(r#"</navPoint></navPoint></navPoint><navPoint id="np4""#), "C 收 3 层再开 D: {ncx}");
    }

    #[test]
    fn split_numbered_title_splits_section_number_not_page_number() {
        assert_eq!(split_numbered_title("第一章 1"), Some(("第一章".into(), "1".into())));
        assert_eq!(split_numbered_title("第一章　1"), Some(("第一章".into(), "1".into())), "全角空格分隔也要认");
        assert_eq!(split_numbered_title("第一章 三"), Some(("第一章".into(), "三".into())), "中文数字编号");
        assert_eq!(split_numbered_title("第一章 237"), None, "三位数以上大概率是印刷页码残留，不拆");
        assert_eq!(split_numbered_title("第一章"), None, "没有编号尾巴不拆");
        assert_eq!(split_numbered_title("1984"), None, "整体是数字不是「标题+编号」结构");
        assert_eq!(split_numbered_title("第一章 0"), None, "0 不是有效小节编号");
    }

    #[test]
    fn auto_toc_splits_numbered_titles_into_nested_entries() {
        let mut v = vec![
            e("content.opf", r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#),
            e("c1.xhtml", "<html><body><h1>第一章 1</h1><p>a</p><h1>后记</h1></body></html>"),
        ];
        wash_entries(&mut v, &WashOpts::default()).unwrap();
        let nav = s(&v, "nav.xhtml");
        assert!(
            nav.contains(r#"<li><a href="c1.xhtml#cj-toc-1">第一章</a><ol><li><a href="c1.xhtml#cj-toc-1">1</a></li></ol></li><li><a href="c1.xhtml#cj-toc-2">后记</a></li>"#),
            "「第一章 1」拆成父子两级、都指向同一锚点；「后记」没有编号尾巴不拆: {nav}"
        );
    }

    #[test]
    fn auto_toc_fallback_when_no_headings_at_all() {
        // 全书没有 h1–h6，退化到按 spine 文件生成目录（取正文首段文本当标题）
        let mut v = vec![
            e("content.opf", r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#),
            e("c1.xhtml", "<html><body><p>从前有座山，山里有座庙。</p></body></html>"),
            e("c2.xhtml", "<html><body><p>庙里有个老和尚在讲故事。</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.toc_generated, 2, "无标题也要生成兜底目录");
        let nav = s(&v, "nav.xhtml");
        assert!(nav.contains(r#"<a href="c1.xhtml">从前有座山，山里有座庙。</a>"#), "无锚点、直接指文件本身: {nav}");
        assert!(nav.contains(r#"<a href="c2.xhtml">庙里有个老和尚在讲故事。</a>"#), "{nav}");
        let ncx = s(&v, "toc.ncx");
        assert!(ncx.contains(r#"content src="c1.xhtml"/"#), "ncx 同样不带 # : {ncx}");
    }

    #[test]
    fn auto_toc_fallback_skipped_when_mostly_imageonly_pages() {
        // 多数页是纯图片(无可提取文本)——疑似漫画/画册，不该被兜底目录灌一堆"正文 N"
        let mut v = vec![
            e("content.opf", r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/><item id="c3" href="c3.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/><itemref idref="c3"/></spine></package>"#),
            e("c1.xhtml", r#"<html><body><img src="p1.jpg"/></body></html>"#),
            e("c2.xhtml", r#"<html><body><img src="p2.jpg"/></body></html>"#),
            e("c3.xhtml", "<html><body><p>唯一一页有字。</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.toc_generated, 0, "多数纯图片页不生成兜底目录");
        assert!(!v.iter().any(|x| x.name == "nav.xhtml"));
    }
}
