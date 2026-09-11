//! 基础 HTML 规整：正文提取（body_inner）、块切分、同章内链规整、id 折叠/去重、裸 `&` 转义。
use super::*;

pub(super) fn body_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // 非贪婪：章节可能是多个 <html> 文档拼接（标题文档+正文文档），贪婪会跨文档吞并
    R.get_or_init(|| Regex::new(r"(?si)<body[^>]*>(.*?)</body>").unwrap())
}

/// 第一个 `<body …>` 与其后第一个 `</body>` 之间的原文（不分大小写）；没有成对的 body → `None`。
/// 与正则 `(?is)<body\b[^>]*>(.*?)</body>` 的首个匹配逐字节等价，但只用无捕获组的 DFA 找开标签、
/// 闭标签用字节搜索——带捕获组的惰性 `(.*?)` 走的是慢速引擎，整章扫下来是清洗层最大的一块耗时
/// （2026-09-24 审计：空页判断、兜底目录、漫画分卷三处各编一份这条正则）。
pub(crate) fn first_body_inner(html: &str) -> Option<&str> {
    static OPEN: OnceLock<Regex> = OnceLock::new();
    let open = OPEN.get_or_init(|| Regex::new(r"(?i)<body\b[^>]*>").unwrap());
    let rest = &html[open.find(html)?.end()..];
    // 找到的是 ASCII 的 `<`，切在字符边界上。
    let end = rest.as_bytes().windows(7).position(|w| w.eq_ignore_ascii_case(b"</body>"))?;
    Some(&rest[..end])
}

pub(super) fn void_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)<(area|base|br|col|embed|hr|img|input|link|meta|param|source|track|wbr)((?:\s[^>]*?)?)\s*/?>").unwrap()
    })
}
pub(super) fn calibre_pb_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)<div\b[^>]*mbppagebreak[^>]*>(?:\s*</div>)?").unwrap())
}
pub(super) fn br_run_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)(?:<br\b[^>]*?/>\s*){3,}").unwrap())
}
pub(super) fn block_end_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)(?:</(?:p|div|h[1-6]|blockquote|section|article|ul|ol|table|pre)>|<hr\s*/?>)").unwrap()
    })
}
pub(super) fn a_href_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r##"(?si)(<a\b[^>]*?\bhref=")([^"#]*)(#[^"]*)?("[^>]*>)(.*?)(</a>)"##).unwrap()
    })
}
pub(super) fn id_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)\bid="([^"]+)""#).unwrap())
}

/// & 后不是 `#?\w+;` 实体的，转 &amp;（手写替 lookahead）。
pub(super) fn escape_bare_amp(s: &str) -> String {
    let cs: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < cs.len() {
        if cs[i] == '&' {
            let mut j = i + 1;
            if j < cs.len() && cs[j] == '#' {
                j += 1;
            }
            let start = j;
            while j < cs.len() && (cs[j].is_ascii_alphanumeric() || cs[j] == '_') {
                j += 1;
            }
            let is_entity = j > start && j < cs.len() && cs[j] == ';';
            if is_entity {
                out.push('&');
            } else {
                out.push_str("&amp;");
            }
        } else {
            out.push(cs[i]);
        }
        i += 1;
    }
    out
}

/// 把 codec 解码出的完整 XHTML 规整成 EPUB 章节能用的 <body> 内层片段。
pub fn body_inner(html: &str) -> String {
    // 提取所有 <body> 段并合并——章节可能是"标题文档 + 正文文档"多个 <html> 拼接，
    // 每个文档一个 body，都要保留（否则只留第一个=标题空壳，正文丢失 → 空白页）。
    let mut inner = String::new();
    for cap in body_re().captures_iter(html) {
        if let Some(m) = cap.get(1) {
            if !inner.is_empty() {
                inner.push('\n');
            }
            inner.push_str(m.as_str());
        }
    }
    if inner.is_empty() {
        inner = html.to_string();
    }
    let inner = void_re().replace_all(&inner, "<${1}${2}/>").into_owned();
    let inner = calibre_pb_re().replace_all(&inner, "").into_owned();
    let inner = br_run_re().replace_all(&inner, "<br/><br/>").into_owned();
    escape_bare_amp(&inner)
}

/// 长章按块边界切成小片（每片约 max_chars 字符）。对齐 download.split_blocks。
pub fn split_blocks(html: &str, max_chars: usize, break_before: Option<&str>) -> Vec<String> {
    // 每个 block = 上个块尾到本块结束标签（含标签）
    let mut blocks: Vec<&str> = Vec::new();
    let mut last = 0usize;
    for m in block_end_re().find_iter(html) {
        blocks.push(&html[last..m.end()]);
        last = m.end();
    }
    if last < html.len() {
        blocks.push(&html[last..]);
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut cur = String::new();
    for block in blocks {
        if let Some(bb) = break_before {
            if block.contains(bb) && !cur.trim().is_empty() {
                chunks.push(std::mem::take(&mut cur));
            }
        }
        cur.push_str(block);
        if cur.chars().count() >= max_chars {
            chunks.push(std::mem::take(&mut cur));
        }
    }
    if !cur.trim().is_empty() {
        chunks.push(cur);
    }
    if chunks.is_empty() {
        vec![html.to_string()]
    } else {
        chunks
    }
}


/// 规整脚注类内链：目标锚点就在本章内 → href 规整成裸 `#锚点`（xochitl 唯一会跳的类别）。
/// 跨文件/外链不动。对齐 epub._fix_internal_links。
pub fn fix_internal_links(html: &str) -> String {
    if !html.contains("href=") {
        return html.to_string();
    }
    let ids: std::collections::HashSet<String> =
        id_re().captures_iter(html).filter_map(|c| c.get(1).map(|m| m.as_str().to_string())).collect();
    a_href_re()
        .replace_all(html, |c: &regex::Captures| {
            let g0 = c.get(0).unwrap().as_str();
            let href = c.get(2).map(|m| m.as_str()).unwrap_or("");
            if href.starts_with("http://")
                || href.starts_with("https://")
                || href.starts_with("mailto:")
                || href.starts_with("tel:")
            {
                return g0.to_string();
            }
            match c.get(3) {
                Some(anchor) => {
                    let aid = &anchor.as_str()[1..];
                    if ids.contains(aid) {
                        format!(
                            "{}#{}{}{}{}",
                            c.get(1).unwrap().as_str(),
                            aid,
                            c.get(4).unwrap().as_str(),
                            c.get(5).unwrap().as_str(),
                            c.get(6).unwrap().as_str()
                        )
                    } else {
                        g0.to_string()
                    }
                }
                None => g0.to_string(),
            }
        })
        .into_owned()
}

// ===== 脚注内联 =====
// 微信读书两套脚注机制，统一内联成**朴素同章锚点**：marker=<a href="#frag">，注释聚章末
// 可见 <div class="footnotes"> 内每条 <p id="frag">。点 marker 跳章末注释、rM 原生「返回」跳回。
// ⚠ 不用 EPUB3 epub:type="noteref"/aside="footnote"——真机实测 xochitl 会把 footnote 语义的
// aside 隐藏、且不把 noteref 渲染成可点链接（点了无黑块、跳不动）。朴素同章锚点才是它唯一会跳的形态。
//  - 导入版(如《13 67》)：marker=<a href="partXXXX.html#frag" type="noteref">，注释是独立
//    <aside id="frag" type="footnote"> 汇总在某章末尾、跨文件 → 死链。先 collect 全书 aside 索引，
//    再把注释按 frag 内联到引用它的 marker 所在章。
//  - 数字版(如《赎罪》)：marker=<img class="qqreader-footnote" alt="注释全文">，内容就在 alt、
//    不跨文件 → 按章内顺序编号，img 换 noteref，alt 生成章末 aside。

/// 折叠单个开始标签上的**重复 `id=` 属性**：每标签只保留第一个 id、删除后续的。
/// 同元素两个 `id` 属性是非法 XHTML——reMarkable 用严格 XML 解析，遇重复属性**整章渲染失败**（只出前
/// 几页，真机《消失的爱人》只 7 页根因，2026-09-01）。转换器（kf8 aid→id / mobi 注入 fpN）把我们的锚点
/// id 放在首位，故"保留首个"= 保住锚点、丢弃冗余的既存 id（calibre `filepos`/`calibre_pb` 等）；也兜底
/// 第三方 EPUB 本就带的重复 id 属性。`\bid=` 不误伤 `aid=`。
pub fn collapse_dup_id_attrs(html: &str) -> String {
    static RE_TAG: OnceLock<Regex> = OnceLock::new();
    let re_tag = RE_TAG.get_or_init(|| Regex::new(r#"(?s)<[a-zA-Z][^>]*>"#).unwrap());
    // 绝大多数章节没有双 id 标签：先只读扫一遍，没有就直接返回，免得 replace_all 为每个标签各复制一份字符串重建全文。
    if !re_tag.find_iter(html).any(|m| id_re().find_iter(m.as_str()).nth(1).is_some()) {
        return html.to_string();
    }
    re_tag
        .replace_all(html, |c: &regex::Captures| {
            let tag = &c[0];
            let ids: Vec<_> = id_re().find_iter(tag).collect();
            if ids.len() <= 1 {
                return tag.to_string();
            }
            let mut out = String::with_capacity(tag.len());
            let mut last = 0usize;
            for m in ids.iter().skip(1) {
                // 连同紧邻的一个前导空白一起删，避免留下双空格
                let mut start = m.start();
                if start > last && tag.as_bytes()[start - 1] == b' ' {
                    start -= 1;
                }
                out.push_str(&tag[last..start]);
                last = m.end();
            }
            out.push_str(&tag[last..]);
            out
        })
        .into_owned()
}

/// 全书 id 去重：reMarkable 锚点是**全书命名空间**，多章重用同一 id（如每章都有 `id="fn1"`）会让
/// 所有 `#fn1` 都跳到全书第一处。按章调用、跨章累积 `seen`：本章某 id 若已在别章出现过，就把它
/// （及本章内指向它的**同文件** `href="#id"`）改成全书唯一名。跨文件 `href="f#id"` 不碰。
/// **先折叠单元素重复 id 属性**（非法 XHTML 兜底，见 `collapse_dup_id_attrs`），再做跨章值去重。
pub fn dedup_ids_in_chapter(html: &str, seen: &mut std::collections::HashSet<String>) -> String {
    let html = &collapse_dup_id_attrs(html);
    let mut rename: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let mut local: std::collections::HashSet<String> = std::collections::HashSet::new();
    for c in id_re().captures_iter(html) {
        let id = c.get(1).unwrap().as_str().to_string();
        if !local.insert(id.clone()) {
            continue; // 本章内同 id 只决策一次
        }
        if seen.contains(&id) {
            let mut n = 2usize;
            let mut cand = format!("{id}-x{n}");
            while seen.contains(&cand) || local.contains(&cand) {
                n += 1;
                cand = format!("{id}-x{n}");
            }
            seen.insert(cand.clone());
            local.insert(cand.clone());
            rename.insert(id, cand);
        } else {
            seen.insert(id);
        }
    }
    if rename.is_empty() {
        return html.to_string();
    }
    // **一遍**替换 `id="旧"` 与同文件 `href="#旧"`（此前每个重命名各编译两个正则、各扫全章两遍：每章 60 个重复 id 时
    // 章均 120 次编译+240 次全章扫描，300 章的书 host 上 3.1s、设备上按 8 倍估算 25s，见 bench；现在每章 1 次扫描）。
    // 属性名/值按原文精确匹配（旧实现的 `(?i)` 连值也忽略大小写，会把 `id="A"` 误当 `id="a"` 一起改名）。
    static RE_ID_OR_HREF: OnceLock<Regex> = OnceLock::new();
    let re = RE_ID_OR_HREF.get_or_init(|| Regex::new(r##"(?i)\bid="([^"]+)"|href="#([^"]+)""##).unwrap());
    re.replace_all(html, |c: &regex::Captures| {
        let (val, is_id) = match c.get(1) {
            Some(v) => (v.as_str(), true),
            None => (c.get(2).unwrap().as_str(), false),
        };
        match rename.get(val) {
            // 与旧实现一致：属性名统一成小写 `id="…"` / `href="#…"`。
            Some(new) if is_id => format!(r#"id="{new}""#),
            Some(new) => format!(r##"href="#{new}""##),
            None => c[0].to_string(),
        }
    })
    .into_owned()
}

#[cfg(test)]
mod body_inner_tests {
    use super::*;
    #[test]
    fn merges_title_doc_and_body_doc() {
        // 微信读书章节 = 标题文档 + 正文文档 两个 <html> 拼接（长夜第一章的真实形态）
        let doc = "<?xml version=\"1.0\"?>\n<html><head><title>第一章</title></head><body>\n  <h1 class=\"firstTitle2\">第一章</h1>\n</body></html>\n<?xml version=\"1.0\"?>\n<!DOCTYPE html>\n<html><body><p>正文第一段</p><p>正文第二段</p></body></html>";
        let out = body_inner(doc);
        assert!(out.contains("第一章"), "缺标题: {out}");
        assert!(out.contains("正文第一段"), "缺正文: {out}");
        assert!(out.contains("正文第二段"), "缺正文2: {out}");
        assert!(!out.contains("</body>"), "残留 </body>: {out}");
        assert!(!out.to_lowercase().contains("<html"), "残留 <html>: {out}");
        assert!(!out.contains("DOCTYPE"), "残留 DOCTYPE: {out}");
    }
    #[test]
    fn single_doc_body_unchanged() {
        let doc = "<html><head><title>x</title></head><body><p>只有一段</p></body></html>";
        assert_eq!(body_inner(doc).trim(), "<p>只有一段</p>");
    }
}
