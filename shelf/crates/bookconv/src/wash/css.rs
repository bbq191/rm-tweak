//! CSS 声明处理：按属性黑名单剥声明、`text-indent` 归一（`filter_css`）、html 内联 style/`<style>` 清洗（`wash_html`）。
use super::*;

// ───────────────────────── 2–4. CSS 声明处理 ─────────────────────────

pub(super) fn decl_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // 属性名 : 值（值里的 HTML 实体 `&#39;` 含分号，按实体整体吃）
    RE.get_or_init(|| Regex::new(r#"(?i)([-a-zA-Z]+)\s*:\s*((?:&#?\w+;|[^;])*);?"#).unwrap())
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Spacing {
    /// 不动 margin/padding。
    Keep,
    /// 上下归零、左右保留（p/div）。
    Vertical,
    /// 全部删除（body/html/@page）。
    All,
}

/// 对一段声明文本：剥 `filter` 里的属性；按 `spacing` 处理 margin/padding。（测试用薄封装）
#[cfg(test)]
pub(super) fn filter_decls(decls: &str, filter: &[String], spacing: Spacing) -> String {
    filter_decls_with(decls, filter, spacing, None)
}

/// 值是否"非零缩进"（`0` / `0em` / `0.0pt` 之类算零；负值=悬挂缩进，保留不动）。
pub(super) fn is_positive_indent(val: &str) -> bool {
    let v = val.trim().trim_end_matches("!important").trim();
    let num: String = v.chars().take_while(|c| c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+').collect();
    num.parse::<f64>().map(|n| n > 0.0).unwrap_or(false)
}

/// 同 `filter_decls`，另把书里**非零** `text-indent` 统一改成 `indent`（Some 时）。
/// 为什么：书自带的类规则（calibre 转 AZW3 常见 `.calibre_ {text-indent:2em}`）xochitl 不认（只认裸 `p{}`），
/// KOReader 认且类规则特异性高于我们的 `p{}`——不统一就"xochitl 1.2em、KOReader 2em"，两器同字节不同观感
/// （2026-09-06 Phase E 英文书对照发现）。`text-indent:0`（诗歌/引文/列表明示不缩进）与负值保留。
pub(super) fn filter_decls_with(decls: &str, filter: &[String], spacing: Spacing, indent: Option<&str>) -> String {
    let mut out: Vec<String> = Vec::new();
    for c in decl_re().captures_iter(decls) {
        let prop = c[1].to_ascii_lowercase();
        let val = c[2].trim();
        if filter.contains(&prop) {
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

pub(super) fn selector_spacing(selector: &str) -> Spacing {
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

/// 选择器是不是"注释容器类"：书自带的 `duokan-footnote-item`/`duokan-footnote-content` 这类，以及我们
/// 自己生成的 `.footnotes`（章末块）/`.cj-fnote`（Inline 内联注释）——判据是选择器文本含
/// "footnote"/"fnote"（大小写不敏感），不追求穷举每本书的命名，覆盖到目前真机见过的形态。
pub(super) fn is_footnote_container_selector(sel: &str) -> bool {
    let l = sel.to_ascii_lowercase();
    l.contains("footnote") || l.contains("fnote")
}

/// 注释容器专用的字号：比正文小一档，相对单位（随用户当前字号缩放，不是又一个"锁死"）。
pub(super) const FOOTNOTE_FONT_SIZE: &str = "0.9em";

/// 整段 CSS（文件或 <style> 内容）：逐规则剥锁 + 边距处理。`@media{}` 嵌套靠"从内向外"匹配最内层规则。
/// 注释容器类是唯一的例外分支：§03av EPUB 线原则②"解锁字号但保留原书颜色/加粗"保护的是**正文语义
/// 加粗**，注释容器类的 `font-weight:bold` 是原书模板写死的装饰样式，不是语义强调——2026-09-23 真机
/// 《甲午：摇摆的战争》坐实（`duokan-footnote-item{font-weight:bold}` 导致全书注释永远加粗，用户反馈
/// "跳转注释后字体不对"，追下去发现其实是加粗不是字体），用户拍板"只剥注释容器类的字重，不碰正文；
/// 注释字号固定比正文小一档"。
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
        if is_footnote_container_selector(sel) {
            let mut filter = opts.filter_props.clone();
            if !filter.iter().any(|p| p == "font-weight") {
                filter.push("font-weight".to_string());
            }
            let mut decls = filter_decls_with(&c[2], &filter, spacing, Some(indent_for(opts)));
            decls.push_str(&format!("font-size:{FOOTNOTE_FONT_SIZE};"));
            return format!("{sel}{{{decls}}}");
        }
        format!("{}{{{}}}", sel, filter_decls_with(&c[2], &opts.filter_props, spacing, Some(indent_for(opts))))
    }).into_owned()
}

/// 本书的首行缩进值（拉丁 1.2em / 中文 2em；Auto 兜底中文）。`wash_css` 与书 css 统一改写共用。
pub(super) fn indent_for(opts: &WashOpts) -> &'static str {
    if opts.lang == LangMode::Latin { "1.2em" } else { "2em" }
}

pub(super) fn style_attr_re() -> &'static Regex {
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
    let s = style_block_re().replace_all(&s, |c: &regex::Captures| {
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
