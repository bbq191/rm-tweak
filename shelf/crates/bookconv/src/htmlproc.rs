//! HTML 规整：body_inner / split_blocks / fix_internal_links —— 移植自 download.py + epub.py。
//! regex crate 不支持 lookahead，`&(?!#?\w+;)` 的转义手写实现。

use regex::Regex;
use std::sync::OnceLock;

fn body_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // 非贪婪：章节可能是多个 <html> 文档拼接（标题文档+正文文档），贪婪会跨文档吞并
    R.get_or_init(|| Regex::new(r"(?si)<body[^>]*>(.*?)</body>").unwrap())
}
fn void_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)<(area|base|br|col|embed|hr|img|input|link|meta|param|source|track|wbr)((?:\s[^>]*?)?)\s*/?>").unwrap()
    })
}
fn calibre_pb_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)<div\b[^>]*mbppagebreak[^>]*>(?:\s*</div>)?").unwrap())
}
fn br_run_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)(?:<br\b[^>]*?/>\s*){3,}").unwrap())
}
fn block_end_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?i)(?:</(?:p|div|h[1-6]|blockquote|section|article|ul|ol|table|pre)>|<hr\s*/?>)").unwrap()
    })
}
fn a_href_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r##"(?si)(<a\b[^>]*?\bhref=")([^"#]*)(#[^"]*)?("[^>]*>)(.*?)(</a>)"##).unwrap()
    })
}
fn id_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)\bid="([^"]+)""#).unwrap())
}

/// & 后不是 `#?\w+;` 实体的，转 &amp;（手写替 lookahead）。
fn escape_bare_amp(s: &str) -> String {
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

fn block_close_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)</(p|div|h[1-6]|li|blockquote|section|article|ul|ol|table|pre)>").unwrap())
}
fn a_open_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?si)<a\b[^>]*>"#).unwrap())
}
fn href_attr_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)\s*\bhref="[^"]*""#).unwrap())
}
fn href_frag_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)\bhref="(#[^"]*)""#).unwrap())
}

/// reMarkable 导入 EPUB 时把章内锚点烘焙成静态索引；一旦某章存在"互指对"
/// （marker 链到注释、注释又链回 marker——几乎所有电子书脚注的标准双向结构），
/// 它的索引器会把这一对链接**整对丢弃**，导致脚注在设备上连可点热区都没有。
/// 破环：定位每个 2-环，把"源元素较晚出现"那条（即注释里的回链）**去链化**
/// ——`<a>`→`<span>`、删掉 href、保留 id——正向 `marker→注释` 即恢复可点，
/// 返回交给 reMarkable 原生"返回第 X 页"条。真机 chap_0010 8 条脚注实测全通。
pub fn break_footnote_cycles(html: &str) -> String {
    // 1) 收集 id → 首次出现的字节位置
    let mut id_pos: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut ids_sorted: Vec<(usize, String)> = Vec::new();
    for c in id_re().captures_iter(html) {
        let m = c.get(0).unwrap();
        let id = c.get(1).unwrap().as_str().to_string();
        id_pos.entry(id.clone()).or_insert(m.start());
        ids_sorted.push((m.start(), id));
    }
    ids_sorted.sort_by_key(|(p, _)| *p);
    if id_pos.is_empty() {
        return html.to_string();
    }

    // 最近前置 id（中间无块级闭合标签才算同元素范围内）
    let nearest = |open_start: usize| -> Option<String> {
        let mut best: Option<(usize, &str)> = None;
        for (p, id) in &ids_sorted {
            if *p < open_start {
                best = Some((*p, id.as_str()));
            } else {
                break;
            }
        }
        let (bp, id) = best?;
        if block_close_re().is_match(&html[bp..open_start]) {
            None
        } else {
            Some(id.to_string())
        }
    };

    // 2) 收集同文件锚点 <a href="#frag">：source_id（自带 id 优先，否则最近前置 id）+ target
    struct Anchor {
        open_start: usize,
        open_end: usize,
        source: String,
        target: String,
    }
    let mut anchors: Vec<Anchor> = Vec::new();
    let mut edges: std::collections::HashSet<(String, String)> = std::collections::HashSet::new();
    for m in a_open_re().find_iter(html) {
        let tag = m.as_str();
        // href 必须是同文件裸锚点 #frag
        let href = match href_frag_re().captures(tag) {
            Some(c) => c.get(1).unwrap().as_str().to_string(),
            None => continue,
        };
        let target = href[1..].to_string();
        if !id_pos.contains_key(&target) {
            continue;
        }
        let self_id = id_re().captures(tag).map(|c| c.get(1).unwrap().as_str().to_string());
        let source = match self_id.or_else(|| nearest(m.start())) {
            Some(s) => s,
            None => continue,
        };
        edges.insert((source.clone(), target.clone()));
        anchors.push(Anchor { open_start: m.start(), open_end: m.end(), source, target });
    }

    // 3) 找 2-环 (A->B 且 B->A)，标记"源元素较晚"那条 <a> 去链化
    let mut kill: Vec<(usize, usize)> = Vec::new(); // (open_start, open_end)
    for a in &anchors {
        if a.source == a.target {
            continue;
        }
        if edges.contains(&(a.target.clone(), a.source.clone())) {
            let ps = *id_pos.get(&a.source).unwrap();
            let pt = *id_pos.get(&a.target).unwrap();
            if ps > pt {
                // 本锚点的源元素较晚 → 它是注释里的回链 → 去链
                kill.push((a.open_start, a.open_end));
            }
        }
    }
    if kill.is_empty() {
        return html.to_string();
    }

    // 4) 从后往前改写：<a ...(去href)...> → <span ...>，对应 </a> → </span>
    kill.sort_by_key(|(s, _)| *s);
    let mut out = html.to_string();
    for (open_start, open_end) in kill.into_iter().rev() {
        // 定位配对的 </a>
        let close_rel = match out[open_end..].find("</a>") {
            Some(i) => open_end + i,
            None => continue,
        };
        // 先改 close，再改 open（open 在前，先动后面的不影响 open 位置）
        out.replace_range(close_rel..close_rel + 4, "</span>");
        let open_tag = &out[open_start..open_end];
        let no_href = href_attr_re().replace(open_tag, "").into_owned();
        let new_tag = format!("<span{}", &no_href[2..]); // 去掉 "<a"
        out.replace_range(open_start..open_end, &new_tag);
    }
    out
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

#[cfg(test)]
mod footnote_tests {
    use super::*;
    /// 真实微信读书脚注（marker/注释同章、href 带旧文件名前缀）应被规整成裸 `#锚点`
    /// 且该章判定为含章内锚点（不切分）——这是 xochitl 唯一会原生跳转的形态。
    #[test]
    fn real_footnote_normalizes() {
        // chap_0005 真实脚注：marker(id=zw1)→text00004.html#zhu1；注释(id=zhu1)→text00004.html#zw1，两者同章。
        let html = r#"<p><a href="text00004.html#zhu1" id="zw1">[1]</a>正文</p><p><a href="text00004.html#zw1" id="zhu1">[1]</a>注释文字</p>"#;
        let out = fix_internal_links(html);
        eprintln!("IN : {html}");
        eprintln!("OUT: {out}");
        assert!(out.contains(r##"href="#zhu1""##), "marker 未规整成裸锚点");
        assert!(out.contains(r##"href="#zw1""##), "注释回链未规整成裸锚点");
    }

    #[test]
    fn kindle_backlink_broken_forward_kept() {
        // Kindle 形态：marker 的 id 在 <small> 上、注释段的回链在独立 <a> 上。
        let html = r##"<p>正文小鼠波波<sup class="c4"><small id="filepos16001"><a href="#filepos21550"><span>[1]</span></a></small></sup>后续</p>
<p id="filepos21550" class="c12"><a href="#filepos16001"><span>[1]</span></a><span>注释文字</span></p>"##;
        let out = break_footnote_cycles(html);
        // 正向 marker→注释 保留
        assert!(out.contains(r##"<a href="#filepos21550"><span>[1]</span></a>"##), "正向脚注链接被误删");
        // 注释里的回链去链化：<a href="#filepos16001"> 不再是链接
        assert!(!out.contains(r##"href="#filepos16001""##), "回链未去链");
        // 注释段 id 保留（正向链接的落点）
        assert!(out.contains(r##"<p id="filepos21550""##), "注释段 id 丢失");
    }

    #[test]
    fn weread_backlink_broken_id_preserved() {
        // weread 形态：id 与回链在同一个 <a> 上，去链后必须保留 id。
        let html = r##"<p>正文<a href="#zhu1" id="zw1">[1]</a>后续</p>
<p><a href="#zw1" id="zhu1">[1]</a>注释文字</p>"##;
        let out = break_footnote_cycles(html);
        assert!(out.contains(r##"<a href="#zhu1" id="zw1">[1]</a>"##), "正向 marker 被误删");
        // 注释锚点去链但保留 id=zhu1（marker 的落点）
        assert!(out.contains(r##"id="zhu1""##), "注释 id 丢失（正向落点会断）");
        assert!(!out.contains(r##"href="#zw1""##), "注释回链未去链");
        assert!(out.contains("<span"), "回链应变成 span");
    }

    #[test]
    fn no_cycle_left_untouched() {
        // 单向 TOC 链接（无回指）不应被动
        let html = r##"<p><a href="#c1">章一</a></p><h2 id="c1">章一</h2>"##;
        assert_eq!(break_footnote_cycles(html), html);
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

fn aside_footnote_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?si)<aside\b[^>]*\btype="footnote"[^>]*>(.*?)</aside>"#).unwrap())
}
fn noteref_a_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?si)<a\b([^>]*\btype="noteref"[^>]*)>(.*?)</a>"#).unwrap())
}
fn sup_noteref_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // <sup> 整体包裹的 noteref（图标脚标常见形态）。group1=a 属性、group2=a 内容(图标)。
    R.get_or_init(|| {
        Regex::new(r#"(?si)<sup[^>]*>\s*<a\b([^>]*\btype="noteref"[^>]*)>(.*?)</a>\s*</sup>"#).unwrap()
    })
}
fn qqfootnote_img_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?si)<img\b([^>]*\bclass="qqreader-footnote"[^>]*?)/?>"#).unwrap())
}
/// 微读 **duokan 图片脚注**标记：`<sup..><a href="#frag">&lt;img ... class="duokan-footnote.." ../&gt;</a></sup>`。
/// 与 qqreader 版不同：注释块**已在同文件** `<p id="frag">`（前向锚有效），且标记里的 `<img>` 被
/// **实体转义**成字面文本、又是微读 CDN 远程图 → 设备离线渲染成一坨死文本、点不动。故只需把标记
/// 内容换成干净可点上标数字、保留 `href="#frag"`（注释块原地不动即可跳）。group1=`<a>` 属性、
/// group2=转义 img 内层（含 class/alt）。
fn duokan_footnote_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#"(?si)<sup\b[^>]*>\s*<a\b([^>]*)>\s*&lt;img\b(.*?)/?&gt;\s*</a>\s*</sup>"#).unwrap()
    })
}
/// Calibre 洗过的 duokan 形态（`ebook-convert` AZW3/EPUB→EPUB 后）：标记里的 `<img>` 是**真标签**（非实体
/// 转义）、src 已是本地图（图片内容仅是"注释N"小图，点不动、设备上一坨小块），`<a>` 上带回链落点
/// `id="c_X_Y"`。group1=`<a>` 属性、group2=img 属性。与转义版共用同一替换逻辑。
fn duokan_footnote_img_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#"(?si)<sup\b[^>]*>\s*<a\b([^>]*)>\s*<img\b([^>]*?)/?>\s*</a>\s*</sup>"#).unwrap()
    })
}
/// 把**指向本文件**的带文件名 href（`href="part0004.html#frag"` 写在 part0004.html 里，Calibre
/// `ebook-convert` 的固定写法）归一成裸 `#frag`。reMarkable 只跟同文件裸锚；且优化器各 pass
/// （`break_footnote_cycles` 只认裸锚、`referenced_note_frags` 把带路径的一律当跨文件尾注搬走）
/// 都以"裸锚=同文件"为前提，不归一会把同章脚注误当跨文件处理（标记丢 id、注释块被搬成无效嵌套）。
/// 按路径末段（basename）比对；非本文件的跨文件 href、无 fragment 的 href 不动。
pub fn normalize_self_hrefs(html: &str, own_basename: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    if own_basename.is_empty() {
        return html.to_string();
    }
    R.get_or_init(|| Regex::new(r##"(?i)href="([^"#]+)#([^"]*)""##).unwrap())
        .replace_all(html, |c: &regex::Captures| {
            let path = c.get(1).unwrap().as_str();
            let base = path.rsplit('/').next().unwrap_or(path);
            if base == own_basename {
                format!("href=\"#{}\"", c.get(2).unwrap().as_str())
            } else {
                c.get(0).unwrap().as_str().to_string()
            }
        })
        .into_owned()
}
/// 从（转义的）img 内层抽注释序号：`alt="注释12"` → `12`。取不到返回 None（调用方用章内计数兜底）。
fn duokan_note_num(img_inner: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)alt="[^"]*?(\d+)"#).unwrap())
        .captures(img_inner)
        .map(|c| c.get(1).unwrap().as_str().to_string())
}
/// duokan 注释块里的**回链** `<a ... href="#c_X_Y">注释文字</a>`（跳回标记的 `c_X_Y` 锚，
/// 但该 id 在书里根本不存在=悬空）。**必须去链成纯文本**：reMarkable 链接索引器遇到"注释块内含
/// 出链"会把整个脚注对判为互指对而**整对丢弃 → 正向 marker 也点不动**（同 break_footnote_cycles
/// 的病理，但那里只拆真 2-环、够不到这悬空回链）。去链后注释跟 qqreader 版一样纯文本、正向恢复可跳。
fn duokan_backlink_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r##"(?si)<a\b[^>]*\bhref="#c_\d+_\d+"[^>]*>(.*?)</a>"##).unwrap())
}
/// duokan 注释块是**无效嵌套 `<p>`**：`<p id="a_X_Y"><p class="pfootnotetext">注释文字</p></p>`。
/// 多条注释成簇相邻时，reMarkable 渲染无效嵌套 `<p>` 会自动闭合外层 + 杂散 `</p>` → **吞/合并其中一条**
/// （"缺一条"根因）。拍平成合法单段 `<p id="a_X_Y">注释文字</p>`，锚点保留、每条独立可跳。
/// group1=id（a_X_Y）、group2=内层 `<p>` 属性、group3=注释文字。
fn duokan_note_block_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r##"(?si)<p\b[^>]*\bid="(a_\d+_\d+)"[^>]*>\s*<p\b([^>]*)>(.*?)</p>\s*</p>"##).unwrap()
    })
}
fn href_fragment(attrs: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)href="[^"]*#([^"]+)""#).unwrap())
        .captures(attrs)
        .map(|c| c.get(1).unwrap().as_str().to_string())
}
fn alt_text(attrs: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)alt="([^"]*)""#).unwrap())
        .captures(attrs)
        .map(|c| c.get(1).unwrap().as_str().to_string())
}
fn esc_text(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}
/// aside 注释内层的回链 `href="partXXXX.html#frag"` 去跨文件前缀成同章 `href="#frag"`
/// （内联后 marker 与注释同章，回链落点也在该章 → 同章可返回）。无 fragment 的 href 不动。
fn deprefix_footnote_hrefs(s: &str) -> String {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)href="[^"]*(#[^"]*)""#).unwrap())
        .replace_all(s, r#"href="$1""#)
        .into_owned()
}

/// 提取一章里所有 `<aside ... id="X" type="footnote">…</aside>`，从正文移除，
/// 返回 (清理后正文, [(id, 注释内层html)])。内层含回链 `<a>` + 注释文本，原样保留。
pub fn collect_footnote_asides(html: &str) -> (String, Vec<(String, String)>) {
    let mut index: Vec<(String, String)> = Vec::new();
    let cleaned = aside_footnote_re()
        .replace_all(html, |c: &regex::Captures| {
            let whole = c.get(0).unwrap().as_str();
            let inner = c.get(1).unwrap().as_str();
            let open_tag = &whole[..whole.find('>').unwrap_or(whole.len())];
            if let Some(idc) = id_re().captures(open_tag) {
                index.push((idc.get(1).unwrap().as_str().to_string(), inner.to_string()));
            }
            String::new()
        })
        .into_owned();
    (cleaned, index)
}

// ===== 优化器：跨文件普通尾注收集 + 全书 id 去重 =====
// （break_footnote_cycles 管同文件裸锚点环、preserve_relink_footnotes 管 noteref+跨文件普通尾注，
//  两者产物再过 dedup_ids_in_chapter 保证 id 全书唯一——reMarkable 锚点是全书命名空间。）

/// 元素开标签是否带"注释"语义：epub:type/type/class 含 footnote|endnote|rearnote|note。
/// 只认语义确证的块 → 目录页/普通交叉引用的跨文件链接绝不会被误当尾注搬走。
fn note_semantic(open_tag: &str) -> bool {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r#"(?i)\b(?:epub:type|type|class)="[^"]*(?:footnote|endnote|rearnote|note)[^"]*""#).unwrap()
    })
    .is_match(open_tag)
}
/// href 里的**跨文件** fragment：`href="非空路径#frag"` → Some(frag)；同文件 `href="#frag"` → None。
fn href_crossfile_fragment(attrs: &str) -> Option<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r##"(?i)href="([^"#]+)#([^"]+)""##).unwrap())
        .captures(attrs)
        .map(|c| c.get(2).unwrap().as_str().to_string())
}
fn aside_any_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?si)<aside\b[^>]*>(.*?)</aside>"#).unwrap())
}
fn p_any_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?si)<p\b[^>]*>(.*?)</p>"#).unwrap())
}
fn li_any_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?si)<li\b[^>]*>(.*?)</li>"#).unwrap())
}
fn div_any_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    // 非贪婪到最近 </div>；嵌套 div 会欠匹配 → 收集处对含嵌套的跳过（见 collect_footnote_notes 守卫）。
    R.get_or_init(|| Regex::new(r#"(?si)<div\b[^>]*>(.*?)</div>"#).unwrap())
}
fn a_generic_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?si)<a\b([^>]*)>(.*?)</a>"#).unwrap())
}

/// 扫一章里所有会被优化器"搬运"的尾注引用 frag：noteref marker + 跨文件普通 `<a>`。
/// 优化器据此只搬**被引用**的注释块（[`collect_footnote_notes`] 的过滤集），未被引用的原地不动。
pub fn referenced_note_frags(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    for c in noteref_a_re().captures_iter(html) {
        if let Some(f) = href_fragment(c.get(1).unwrap().as_str()) {
            out.push(f);
        }
    }
    for m in a_open_re().find_iter(html) {
        if let Some(f) = href_crossfile_fragment(m.as_str()) {
            out.push(f);
        }
    }
    out
}

/// 优化器版注释收集：从各章抽出**被引用(referenced)**的注释块——`<aside>`/`<p>`/`<li>` 且带注释
/// 语义([`note_semantic`])——按 id 建索引、从原位移除，交给 [`preserve_relink_footnotes`] 搬进引用
/// 它的那一章。**只搬 referenced 里的**：未被任何 marker 引用的块原样留在原处（杜绝"移走又没人接=
/// 内容丢失"）。`<div>` 因嵌套用正则不可靠切分暂不支持（真实尾注绝大多数是 p/li/aside）。
/// `require_semantic`：优化器传 true——对任意导入书要求块自带 footnote/note 语义（`note_semantic`），
/// 防把普通跨文件交叉引用误当尾注搬走；下载管线传 **false**——微读注释块 class 是混淆名（如
/// `class_s1r`）无语义，但 marker（noteref/纯跨文件`<a>`）就是脚注引用，故"**id 被 marker 引用**"
/// 本身即注释的充分证据（referenced 已只含脚注 marker 的 frag），不再要 class 语义（13·67 的 `<p>` 注释即此）。
pub fn collect_footnote_notes(
    html: &str,
    referenced: &std::collections::HashSet<String>,
    require_semantic: bool,
) -> (String, Vec<(String, String)>) {
    let mut index: Vec<(String, String)> = Vec::new();
    let mut cleaned = html.to_string();
    // p 不嵌 p、li 不嵌 li（扁平尾注表）、aside 不嵌 aside → 非贪婪匹配到最近闭合安全。
    // div 也收（v7：不少书注释块是 <div id=fn>），但**嵌套 div 正则不可靠** → 内含 <div 的跳过不搬（零丢失）。
    for re in [aside_any_re(), p_any_re(), li_any_re(), div_any_re()] {
        cleaned = re
            .replace_all(&cleaned, |c: &regex::Captures| {
                let whole = c.get(0).unwrap().as_str();
                let open = &whole[..whole.find('>').map(|i| i + 1).unwrap_or(whole.len())];
                let inner = c.get(1).unwrap().as_str();
                let is_div = whole.as_bytes().get(..4).map(|b| b.eq_ignore_ascii_case(b"<div")).unwrap_or(false);
                if is_div && inner.contains("<div") {
                    return whole.to_string(); // 嵌套 div：欠匹配风险，原样保留不搬
                }
                let id = match id_re().captures(open).map(|m| m.get(1).unwrap().as_str().to_string()) {
                    Some(i) => i,
                    None => return whole.to_string(),
                };
                if referenced.contains(&id) && (!require_semantic || note_semantic(open)) {
                    index.push((id, inner.to_string()));
                    String::new()
                } else {
                    whole.to_string()
                }
            })
            .into_owned();
    }
    (cleaned, index)
}

/// 折叠单个开始标签上的**重复 `id=` 属性**：每标签只保留第一个 id、删除后续的。
/// 同元素两个 `id` 属性是非法 XHTML——reMarkable 用严格 XML 解析，遇重复属性**整章渲染失败**（只出前
/// 几页，真机《消失的爱人》只 7 页根因，2026-09-01）。转换器（kf8 aid→id / mobi 注入 fpN）把我们的锚点
/// id 放在首位，故"保留首个"= 保住锚点、丢弃冗余的既存 id（calibre `filepos`/`calibre_pb` 等）；也兜底
/// 第三方 EPUB 本就带的重复 id 属性。`\bid=` 不误伤 `aid=`。
pub fn collapse_dup_id_attrs(html: &str) -> String {
    static RE_TAG: OnceLock<Regex> = OnceLock::new();
    let re_tag = RE_TAG.get_or_init(|| Regex::new(r#"(?s)<[a-zA-Z][^>]*>"#).unwrap());
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
    let mut out = html.to_string();
    for (old, new) in &rename {
        let e = regex::escape(old);
        let id_pat = Regex::new(&format!(r#"(?i)\bid="{e}""#)).unwrap();
        out = id_pat.replace_all(&out, regex::NoExpand(&format!(r#"id="{new}""#))).into_owned();
        let href_pat = Regex::new(&format!(r##"(?i)href="#{e}""##)).unwrap();
        out = href_pat.replace_all(&out, regex::NoExpand(&format!(r##"href="#{new}""##))).into_owned();
    }
    out
}

/// 内联本章脚注。`index`=全书 footnote-id→注释内层html（导入版跨章注释）。
/// 处理本章 marker（导入版 noteref <a> + 数字版 qqreader-footnote <img>），收集对应注释到
/// 章末**可见** `<div class="footnotes">` 区，每条 `<p id="frag">`；marker 换成朴素同章
/// 锚点 `<a href="#frag">`。**不用 EPUB3 epub:type/aside**——实测 xochitl 会把 footnote 语义
/// 的 aside 隐藏且不渲染 noteref 为可点链接（点不了、无黑块）。朴素同章锚点是真机验证过唯一会跳的形态。
/// `qfn_counter` 跨章累积，保证数字版自造 id **全书唯一**（reMarkable 锚点是全书空间，
/// 每章从 1 重编 id 会撞车跳错章）。pipeline 逐章调用时传同一个计数器。
pub fn inline_footnotes(
    html: &str,
    index: &std::collections::HashMap<String, String>,
    qfn_counter: &mut usize,
) -> String {
    let mut notes: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

    // 1) 导入版：<a type="noteref" href="...#frag">N</a> → <a href="#frag">N</a>（保留原角标文本）
    let s1 = noteref_a_re()
        .replace_all(html, |c: &regex::Captures| {
            let attrs = c.get(1).unwrap().as_str();
            let marker = c.get(2).unwrap().as_str().trim();
            match href_fragment(attrs).and_then(|f| index.get(&f).map(|t| (f, t))) {
                Some((frag, text)) => {
                    if seen.insert(frag.clone()) {
                        let inner = deprefix_footnote_hrefs(text);
                        notes.push(format!("<p id=\"{frag}\">{inner}</p>"));
                    }
                    format!("<a href=\"#{frag}\">{}</a>", esc_text(marker))
                }
                None => c.get(0).unwrap().as_str().to_string(), // 注释没抓到 → 保留原样
            }
        })
        .into_owned();

    // 1b) 纯跨文件 marker：<a href="其他文件#frag">N</a>（无 noteref，微读部分脚注是这形态）。
    // noteref pass 已把 noteref 改成同文件 #frag（不再匹配 href_crossfile_fragment）；此处只认仍是
    // 跨文件、且 frag 命中 index（注释块已被 collect_footnote_asides 收走）的纯 <a> → 改同章
    // <a href="#frag"> + 注释内联本章。不修则该注释既断链、又因被收走而**丢失**（13·67「大帮3」即此形态）。
    let s1b = a_generic_re()
        .replace_all(&s1, |c: &regex::Captures| {
            let attrs = c.get(1).unwrap().as_str();
            let content = c.get(2).unwrap().as_str().trim();
            match href_crossfile_fragment(attrs).and_then(|f| index.get(&f).map(|t| (f, t))) {
                Some((frag, text)) => {
                    if seen.insert(frag.clone()) {
                        notes.push(format!("<p id=\"{frag}\">{}</p>", deprefix_footnote_hrefs(text)));
                    }
                    format!("<a href=\"#{frag}\">{}</a>", esc_text(content))
                }
                None => c.get(0).unwrap().as_str().to_string(),
            }
        })
        .into_owned();

    // 2) 数字版：<img class="qqreader-footnote" alt="注释全文"> → <a href="#qfnG"><sup>L</sup></a>
    // id 用全局递增 G（全书唯一，防跨章撞车），<sup> 显示章内序号 L（用户看着自然从 1 起）。
    let mut local = 0usize;
    let s2 = qqfootnote_img_re()
        .replace_all(&s1b, |c: &regex::Captures| {
            let alt = alt_text(c.get(1).unwrap().as_str()).unwrap_or_default();
            local += 1;
            *qfn_counter += 1;
            let id = format!("qfn{}", *qfn_counter);
            notes.push(format!("<p id=\"{id}\">{local}. {}</p>", esc_text(&alt)));
            format!("<a href=\"#{id}\"><sup>{local}</sup></a>")
        })
        .into_owned();

    // 3) duokan 版：注释块已同文件（前向锚有效），只把死图标记换成可点上标（共用于导入优化器路径）。
    let s3 = fix_duokan_markers(&s2);

    if notes.is_empty() {
        return s3;
    }
    format!("{s3}\n<hr/>\n<div class=\"footnotes\">\n{}\n</div>", notes.join("\n"))
}

/// 修微读 **duokan 图片脚注标记**（下载路径 `inline_footnotes` 与导入优化器 `optimize_epub` **共用**）：
/// `<sup><a href="#frag">&lt;img class="duokan-footnote.." alt="注释N"/&gt;</a></sup>`
/// → `<a href="#frag"><sup>N</sup></a>`。duokan 的注释块**已在同文件** `<p id="frag">`（前向锚有效），
/// 故只需把"实体转义 + 远程 CDN 图 = 离线不可点"的死图标记换成干净可点上标数字、保留 href；注释块不动。
/// 非 duokan 脚注 img（`duokan-footnote` 类不在内层）或跨文件锚点原样放行。
/// **Calibre 洗后形态**（2026-09-02，`wash_epub.sh` 产物）：img 是真标签+本地图、`<a>` 带 `id="c_X_Y"`、
/// 注释块是合法 `<li id="a_X_Y"><p>…<a href="#c_X_Y">`（真 2-环）——真 img 也换上标、**id 保留**，
/// 环交给前置的 `break_footnote_cycles` 拆（回链去链、id 留作落点）；合法嵌套的 li 不动。
pub fn fix_duokan_markers(html: &str) -> String {
    let mut local = 0usize;
    // 标记替换（转义 img 版 / Calibre 真 img 版共用）：保留 href 与 `<a>` 自带 id（Calibre 形态的
    // 回链落点，丢了则注释里的回链悬空 → reMarkable 判互指整对丢弃）。
    let mut rewrite = |a_attrs: &str, img_inner: &str, whole: &str| -> String {
        match (img_inner.contains("duokan-footnote"), href_fragment(a_attrs)) {
            (true, Some(frag)) => {
                local += 1;
                let num = duokan_note_num(img_inner).unwrap_or_else(|| local.to_string());
                let id_attr = id_re()
                    .captures(a_attrs)
                    .map(|c| format!(" id=\"{}\"", c.get(1).unwrap().as_str()))
                    .unwrap_or_default();
                format!("<a href=\"#{frag}\"{id_attr}><sup>{}</sup></a>", esc_text(&num))
            }
            _ => whole.to_string(),
        }
    };
    let markers = duokan_footnote_re()
        .replace_all(html, |c: &regex::Captures| {
            rewrite(c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str(), c.get(0).unwrap().as_str())
        })
        .into_owned();
    let markers = duokan_footnote_img_re()
        .replace_all(&markers, |c: &regex::Captures| {
            rewrite(c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str(), c.get(0).unwrap().as_str())
        })
        .into_owned();
    // 去链注释块里的悬空回链（#c_X_Y），否则 reMarkable 判互指对整对丢弃、正向也点不动。
    let unlinked = duokan_backlink_re()
        .replace_all(&markers, |c: &regex::Captures| c.get(1).unwrap().as_str().to_string())
        .into_owned();
    // 拍平注释块的无效嵌套 <p>（成簇相邻时会被 reMarkable 吞掉一条），锚点保留。
    duokan_note_block_re()
        .replace_all(&unlinked, |c: &regex::Captures| {
            format!("<p id=\"{}\"{}>{}</p>", c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str(), c.get(3).unwrap().as_str())
        })
        .into_owned()
}

// ===== 字体解锁 =====
fn style_attr_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)\s*style="([^"]*)""#).unwrap())
}
fn font_decl_re() -> &'static Regex {
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
fn parse_css_color(value: &str) -> Option<(u8, u8, u8)> {
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
fn achromatic_dark(r: u8, g: u8, b: u8) -> bool {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    mx.saturating_sub(mn) <= 24 && (1..=239).contains(&mx)
}

/// font-weight 值是否"细到该提"（<400 或 lighter）——e-ink 上细笔画消失，提到 400 常规体。
fn is_thin_weight(value: &str) -> bool {
    let v = value.trim().to_ascii_lowercase();
    if v == "lighter" {
        return true;
    }
    v.parse::<u32>().map(|n| n < 400).unwrap_or(false)
}

/// 对一段 CSS 声明文本：灰色 `color` → `#000000`、细 `font-weight` → `400`。只认属性名本身
/// （`color` 前必须是 起始/空白/`;`/`{`/引号，从而排除 background-color/border-color 等 `-color`）。
fn darken_css_decls(css: &str) -> String {
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

/// 注释正文去标签成纯内联文本（供 `FootnoteMode::Inline` 塞进 `<span>`，杜绝块级标签造成非法嵌套
/// →xochitl 严格 XML 整章白屏）。折叠空白、还原常见空格实体。
fn inline_note_text(html: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    let tag = TAG.get_or_init(|| Regex::new(r"(?s)<[^>]*>").unwrap());
    let t = tag.replace_all(html, "");
    t.replace("&nbsp;", " ").replace("&#160;", " ").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 优化器专用脚注处理：**完整保留 marker 原始内容/样式**(sup/上标/图标一律不动)，只把 `<a>`
/// 上 xochitl 不认的 `epub:type` 去掉、href 规整成同章 `#frag`(xochitl 唯一会跳的形态)；
/// 注释块(index 提供，跨文件也行)收集、移到本章末尾可见 `<div class="footnotes">`。
/// 与 inline_footnotes 的区别：那个为微信读书**重造** marker，这个为第三方书**保留脚标原样**。
pub fn preserve_relink_footnotes(html: &str, index: &std::collections::HashMap<String, String>, mode: crate::optimize::FootnoteMode) -> String {
    use crate::optimize::FootnoteMode;
    let mut appended: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut counter = 0usize;

    // 两种呈现：Inline=注释文字就地内联〔…〕始终可见、不跳转（xochitl 无弹窗）；
    //          Anchor=注释移章末 + marker 改同章锚点（图标留原位、独立可点 [N]）。
    let mut make = |attrs: &str, content: &str, sup_wrapped: bool| -> Option<String> {
        let frag = href_fragment(attrs)?;
        let text = index.get(&frag)?;
        if mode == FootnoteMode::Inline {
            // 内联：**丢弃原 marker**（很多书 marker 是图标 <img>，xochitl 按固有尺寸渲染=巨大且每条重复），
            // 就地只留内联注释 `〔…〕`（注释**去标签成纯文本**，杜绝块级标签塞进 <p> 致 xochitl 严格 XML 白屏）。
            let _ = (content, sup_wrapped);
            return Some(format!("<span class=\"cj-fnote\">〔{}〕</span>", inline_note_text(text)));
        }
        if seen.insert(frag.clone()) {
            // 注释放章末。不加可点回链——真机实测 reMarkable 会丢弃"marker↔注释"互指里较晚那条
            // (注释回链)，加了也点不了、反成死链迷惑人。返回靠 xochitl 原生。
            appended.push(format!("<p id=\"{frag}\">{}</p>", deprefix_footnote_hrefs(text)));
        }
        counter += 1;
        Some(if sup_wrapped {
            // 图标留在原 <sup> 内(纯视觉、去链)，[N] 独立跟在 sup 后(正常大小、可点)
            format!("<sup>{content}</sup><a href=\"#{frag}\">[{counter}]</a>")
        } else {
            format!("<a href=\"#{frag}\">{content}</a> <a href=\"#{frag}\">[{counter}]</a>")
        })
    };

    // 1) <sup> 整体包裹的图标 noteref
    let s1 = sup_noteref_re()
        .replace_all(html, |c: &regex::Captures| {
            make(c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str(), true)
                .unwrap_or_else(|| c.get(0).unwrap().as_str().to_string())
        })
        .into_owned();
    // 2) 剩余裸 noteref
    let out = noteref_a_re()
        .replace_all(&s1, |c: &regex::Captures| {
            make(c.get(1).unwrap().as_str(), c.get(2).unwrap().as_str(), false)
                .unwrap_or_else(|| c.get(0).unwrap().as_str().to_string())
        })
        .into_owned();
    drop(make); // 释放对 appended/seen 的可变借用，pass 3 要用

    // 3) 跨文件普通 <a href="其他文件#frag">：非 noteref，但 frag 已被 collect_footnote_notes 收进
    //    index（确证是注释）→ Inline 内联〔…〕/ Anchor 改同章锚点 + 注释搬章末。目标不在 index 的（目录/交叉引用）不动。
    let out = a_generic_re()
        .replace_all(&out, |c: &regex::Captures| {
            let attrs = c.get(1).unwrap().as_str();
            let content = c.get(2).unwrap().as_str();
            match href_crossfile_fragment(attrs).filter(|f| index.contains_key(f)) {
                Some(frag) => {
                    if mode == FootnoteMode::Inline {
                        // 跨文件普通 <a> marker（多为"12"数字文本）——内联模式丢弃 marker，只留内联注释。
                        return format!("<span class=\"cj-fnote\">〔{}〕</span>", inline_note_text(&index[&frag]));
                    }
                    if seen.insert(frag.clone()) {
                        appended.push(format!("<p id=\"{frag}\">{}</p>", deprefix_footnote_hrefs(&index[&frag])));
                    }
                    format!("<a href=\"#{frag}\">{content}</a>")
                }
                None => c.get(0).unwrap().as_str().to_string(),
            }
        })
        .into_owned();

    if appended.is_empty() {
        return out;
    }
    // ⚠ 注释区必须插到 </body> **之内**。optimize 处理的是完整 xhtml，若加到文件末尾就落在
    // </body></html> 外面=无效 HTML，xochitl 不为其中的 id 建锚点 → marker 死链、点不动。
    let block = format!("\n<hr/>\n<div class=\"footnotes\">\n{}\n</div>\n", appended.join("\n"));
    match out.rfind("</body>") {
        Some(pos) => format!("{}{}{}", &out[..pos], block, &out[pos..]),
        None => format!("{out}{block}"),
    }
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
mod footnote_inline_tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn imported_crossfile_footnote_inlined() {
        // 《13 67》真实形态：注释章的 aside（回链跨文件）+ 正文章的 noteref marker。
        let note_chapter = r#"<p>正文正文</p><aside id="a4ZX" type="footnote" class="class_svk"><a href="part0005.html#a507">1</a>. 警司注释文本</aside>"#;
        let (cleaned, idx) = collect_footnote_asides(note_chapter);
        assert!(!cleaned.contains("<aside"), "aside 未从注释章正文移除: {cleaned}");
        assert_eq!(idx.len(), 1);
        assert_eq!(idx[0].0, "a4ZX");

        let mut map: HashMap<String, String> = HashMap::new();
        for (k, v) in idx {
            map.insert(k, v);
        }
        let text_chapter = r#"<p>那位<a href="part0011.html#a4ZX" type="noteref" class="class_s4yp"> 1 </a>警司</p>"#;
        let out = inline_footnotes(text_chapter, &map, &mut 0);
        assert!(out.contains(r##"<a href="#a4ZX">1</a>"##), "marker 未变朴素同章锚点: {out}");
        assert!(!out.contains("epub:type"), "不应残留 epub:type: {out}");
        assert!(out.contains(r##"<p id="a4ZX">"##), "章末缺可见注释 <p id>: {out}");
        assert!(out.contains(r##"<div class="footnotes">"##), "缺章末注释区: {out}");
        assert!(out.contains("警司注释文本"), "注释文本丢失: {out}");
        assert!(out.contains(r##"href="#a507""##), "回链未去跨文件前缀: {out}");
        assert!(!out.contains("part0005.html"), "回链仍带 part 前缀: {out}");
    }

    #[test]
    fn digital_alt_footnote_inlined() {
        // 《赎罪》真实形态：脚注就是带 alt 的图标 img。
        let ch = r#"<p>海神喷泉<img class="qqreader-footnote" alt="贝尼尼的杰作，位于罗马。" src="https://res.weread.qq.com/wrepub/epub_22781957_2" data-w="50px" />后续</p>"#;
        let out = inline_footnotes(ch, &HashMap::new(), &mut 0);
        assert!(!out.contains("<img"), "脚注图标 img 未被替换: {out}");
        assert!(out.contains(r##"<a href="#qfn1"><sup>1</sup></a>"##), "img 未变同章锚点: {out}");
        assert!(out.contains(r##"<p id="qfn1">1. "##), "缺章末注释 <p id>: {out}");
        assert!(out.contains("贝尼尼的杰作"), "alt 注释文本丢失: {out}");
    }

    #[test]
    fn no_footnote_untouched() {
        let ch = "<p>普通正文，无脚注</p>";
        assert_eq!(inline_footnotes(ch, &HashMap::new(), &mut 0), ch);
    }

    #[test]
    fn normalize_self_hrefs_only_own_file() {
        let html = r##"<a href="part0004.html#a_1">本文件</a><a href="text/part0004.html#b">带目录本文件</a><a href="part0005.html#c">别的文件</a><a href="part0004.html">无锚</a><a href="#d">已裸</a>"##;
        let out = normalize_self_hrefs(html, "part0004.html");
        assert!(out.contains(r##"href="#a_1""##), "本文件应归一: {out}");
        assert!(out.contains(r##"href="#b""##), "带目录的本文件应归一: {out}");
        assert!(out.contains(r##"href="part0005.html#c""##), "跨文件不动: {out}");
        assert!(out.contains(r##"href="part0004.html">"##), "无 fragment 不动: {out}");
        assert_eq!(normalize_self_hrefs(html, ""), html, "空 basename 原样");
    }

    #[test]
    fn fix_duokan_markers_calibre_real_img_keeps_id() {
        // Calibre 洗后：真 <img>（本地图）+ <a> 自带回链落点 id。换上标、id 必须保留。
        let html = r##"<sup class="calibre4"><a class="duokan-footnote" href="#a_2_1" id="c_2_1"><img alt="注释7" class="duokan-footnote1" src="../images/00003.png"/></a></sup>"##;
        let out = fix_duokan_markers(html);
        assert_eq!(out, r##"<a href="#a_2_1" id="c_2_1"><sup>7</sup></a>"##);
        // 非 duokan 的真 img 链接不碰
        let other = r##"<sup><a href="#x"><img alt="图" class="icon" src="i.png"/></a></sup>"##;
        assert_eq!(fix_duokan_markers(other), other);
    }

    #[test]
    fn fix_duokan_markers_shared_fn() {
        // 共用函数（下载 inline_footnotes + 导入优化器都调）：多标记按 alt 号、非 duokan 的转义 img 不碰。
        let html = r##"<sup><a href="#a_1_1">&lt;img alt="注释1" class="duokan-footnote1" src="https://res.weread.qq.com/a.png"/&gt;</a></sup>正文<sup><a href="#a_1_2">&lt;img alt="注释2" class="duokan-footnote1" src="https://res.weread.qq.com/b.png"/&gt;</a></sup>
<sup><a href="#x">&lt;img alt="别的" class="other-icon"/&gt;</a></sup>"##;
        let out = fix_duokan_markers(html);
        assert!(out.contains(r##"<a href="#a_1_1"><sup>1</sup></a>"##), "标记1: {out}");
        assert!(out.contains(r##"<a href="#a_1_2"><sup>2</sup></a>"##), "标记2按 alt 号: {out}");
        assert!(!out.contains("duokan-footnote"), "duokan img 全清: {out}");
        assert!(out.contains(r##"class="other-icon""##), "非 duokan 的转义 img 不该被碰: {out}");
    }

    #[test]
    fn duokan_img_footnote_marker_becomes_tappable_sup() {
        // 《人骨拼图》真实 duokan 形态：标记的 <img> 被**实体转义**成字面死文本、又是微读 CDN
        // 远程图（离线渲染成一坨、点不动）；注释块**已同文件** <p id="a_8_1">（前向锚 #a_8_1 有效）。
        // 修复=标记换成干净可点上标数字、保留 href、注释块原地不动。
        // 真实结构：注释块是无效嵌套 <p>、内含悬空回链 <a href="#c_8_1">。
        let ch = r##"<p>喝了太多冰镇台克利<sup class="calibre4"><a href="#a_8_1">&lt;img alt="注释1" class="duokan-footnote1" src="https://res.weread.qq.com/x_00003.png" data-w="48px" data-ratio="1.000"/&gt;</a></sup>。</p>
<p id="a_8_1"><p class="pfootnotetext"><a class="calibre6" href="#c_8_1">一种由朗姆酒、柠檬汁和糖混合的加冰鸡尾酒。</a></p></p>"##;
        let out = inline_footnotes(ch, &HashMap::new(), &mut 0);
        assert!(out.contains(r##"<a href="#a_8_1"><sup>1</sup></a>"##), "标记未换成可点上标: {out}");
        assert!(!out.contains("&lt;img"), "转义死图标记未清除: {out}");
        assert!(!out.contains("duokan-footnote"), "duokan img 未替换: {out}");
        assert!(!out.contains("res.weread.qq.com"), "远程 CDN 图未去掉: {out}");
        assert!(out.contains("加冰鸡尾酒"), "注释文本不该丢: {out}");
        // 悬空回链去链（否则 reMarkable 判互指对整对丢弃、正向点不动）。
        assert!(!out.contains(r##"href="#c_8_1""##), "注释回链未去链: {out}");
        assert!(!out.contains("<a class=\"calibre6\""), "回链 <a> 未去掉: {out}");
        // 嵌套 <p> 拍平成合法单段（成簇相邻时无效嵌套会被 reMarkable 吞一条=缺一条）。
        assert!(out.contains(r##"<p id="a_8_1" class="pfootnotetext">"##), "嵌套 p 未拍平: {out}");
        assert!(!out.contains("<p id=\"a_8_1\">\n"), "外层空 p 仍在(未拍平): {out}");
    }

    #[test]
    fn plain_crossfile_marker_inlined() {
        // 《13·67》第441页「大帮3」真实形态：marker 是**纯 <a>（无 noteref）**、跨文件、指向被
        // collect_footnote_asides 收走的 aside。旧 inline_footnotes 只认 noteref → 漏掉 → 注释既断链
        // 又丢失。pass 1b 应据 index 命中把它内联。
        let note_chapter = r#"<aside id="a51T" type="footnote"><a href="part0038.html#a541">3</a>. 高级督察俗称大帮</aside>"#;
        let (cleaned, idx) = collect_footnote_asides(note_chapter);
        assert!(!cleaned.contains("<aside"));
        let mut map: HashMap<String, String> = HashMap::new();
        for (k, v) in idx { map.insert(k, v); }
        let text_chapter = r#"<p>被叫做"大帮"<span id="a541"/><a href="part0040.html#a51T" class="class_s4yp"> 3 </a>，在分区任职</p>"#;
        let out = inline_footnotes(text_chapter, &map, &mut 0);
        assert!(out.contains(r##"<a href="#a51T">3</a>"##), "纯跨文件 marker 未改同章锚点: {out}");
        assert!(!out.contains("part0040.html"), "跨文件 href 前缀未去掉: {out}");
        assert!(out.contains(r##"<p id="a51T">"##), "注释未内联本章章末: {out}");
        assert!(out.contains("高级督察俗称大帮"), "注释文本丢失: {out}");
        assert!(!out.contains("part0038.html"), "注释内回链未去跨文件前缀: {out}");
    }

    #[test]
    fn weread_p_note_collected_without_semantic() {
        // 13·67 真实形态：注释章里 <aside> 与 <p class="class_s1r">（**无 note 语义**）交替。
        // 下载管线 collect_footnote_notes(require_semantic=false) 须靠 id∈referenced 收 <p> 注释，
        // 再由 inline_footnotes pass 1b 内联跨文件纯 marker。
        use std::collections::HashSet;
        let text = r#"<p>被叫做"大帮"<span id="a541"/><a href="part0040.html#a51T" class="class_s4yp"> 3 </a>后续</p>"#;
        let notes = r#"<aside id="a51U" type="footnote" class="class_svk"><a href="x#y">4</a>. CID</aside><p id="a51T" class="class_s1r"><a href="part0037.html#a541">3</a>. 帮办、大帮：一词八十年代已式微</p>"#;
        // 全书 referenced
        let mut referenced: HashSet<String> = HashSet::new();
        for f in referenced_note_frags(text) { referenced.insert(f); }
        // 收注释（不要求语义）→ class_s1r 的 <p> 也应被收
        let (notes_cleaned, collected) = collect_footnote_notes(notes, &referenced, false);
        let mut map: HashMap<String, String> = HashMap::new();
        for (k, v) in collected { map.insert(k, v); }
        assert!(map.contains_key("a51T"), "class_s1r 的 <p> 注释未被收: keys={:?}", map.keys().collect::<Vec<_>>());
        assert!(!notes_cleaned.contains(r##"id="a51T""##), "a51T 注释未从原位移除");
        // 内联
        let out = inline_footnotes(text, &map, &mut 0);
        assert!(out.contains(r##"<a href="#a51T">3</a>"##), "纯跨文件 marker 未内联: {out}");
        assert!(out.contains("帮办、大帮"), "注释文本未内联: {out}");
    }

    #[test]
    fn plain_crossfile_missing_note_kept() {
        // 纯跨文件 marker 但 frag 不在 index（注释真缺）→ 原样保留，不乱改。
        let text = r#"<p>见<a href="chap03.xhtml#sec2">第三章</a></p>"#;
        assert_eq!(inline_footnotes(text, &HashMap::new(), &mut 0), text);
    }
}

#[cfg(test)]
mod optimizer_footnote_tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    // ---- collect_footnote_notes：只搬被引用 + 语义确证的块 ----
    #[test]
    fn collects_referenced_p_and_li_notes() {
        // 书末尾注文件：一个 <p> 尾注 + 一个 <li> 尾注 + 一个非注释 <p>（普通段）。
        let notes = r#"<p class="footnote" id="n1">注释一 <a href="text01.xhtml#r1">↩</a></p><ol><li class="endnote" id="n2">注释二</li></ol><p id="plain">普通段落无语义</p>"#;
        let mut referenced = HashSet::new();
        referenced.insert("n1".to_string());
        referenced.insert("n2".to_string());
        referenced.insert("plain".to_string()); // 即便被引用，无注释语义也不搬
        let (cleaned, idx) = collect_footnote_notes(notes, &referenced, true);
        let ids: Vec<&str> = idx.iter().map(|(i, _)| i.as_str()).collect();
        assert!(ids.contains(&"n1") && ids.contains(&"n2"), "referenced 的 p/li 注释应被收: {ids:?}");
        assert!(!ids.contains(&"plain"), "无注释语义的 <p> 不该被搬: {ids:?}");
        assert!(!cleaned.contains(r##"id="n1""##) && !cleaned.contains(r##"id="n2""##), "已收注释应从原位移除: {cleaned}");
        assert!(cleaned.contains(r##"id="plain""##), "普通段落应原样保留: {cleaned}");
    }

    #[test]
    fn skips_unreferenced_notes_no_loss() {
        // 未被任何 marker 引用的注释块必须原地保留（杜绝内容丢失）。
        let notes = r#"<p class="footnote" id="orphan">没人引用的注释文本</p>"#;
        let (cleaned, idx) = collect_footnote_notes(notes, &HashSet::new(), true);
        assert!(idx.is_empty(), "无引用不该收");
        assert_eq!(cleaned, notes, "无引用的注释应原样不动: {cleaned}");
    }

    #[test]
    fn collects_flat_div_notes_but_skips_nested() {
        let mut referenced = HashSet::new();
        referenced.insert("d1".to_string());
        referenced.insert("d2".to_string());
        // d1 扁平 div 注释应收；d2 含嵌套 div → 跳过不搬（零丢失）
        let notes = r#"<div class="footnote" id="d1">扁平注释文本</div><div class="footnote" id="d2">外<div>内嵌</div>层</div>"#;
        let (cleaned, idx) = collect_footnote_notes(notes, &referenced, true);
        let ids: Vec<&str> = idx.iter().map(|(i, _)| i.as_str()).collect();
        assert!(ids.contains(&"d1") && !ids.contains(&"d2"), "扁平 div 收、嵌套 div 跳过: {ids:?}");
        assert!(!cleaned.contains(r##"id="d1""##) && cleaned.contains(r##"id="d2""##), "{cleaned}");
    }

    // ---- referenced_note_frags：noteref + 跨文件普通 <a> ----
    #[test]
    fn scans_noteref_and_crossfile_markers() {
        let ch = r##"<p>甲<a epub:type="noteref" href="notes.xhtml#a1">1</a>乙<a href="notes.xhtml#a2">2</a>丙<a href="#local">本地</a></p>"##;
        let mut fr = referenced_note_frags(ch);
        fr.sort();
        assert!(fr.contains(&"a1".to_string()), "noteref frag 应收: {fr:?}");
        assert!(fr.contains(&"a2".to_string()), "跨文件普通 <a> frag 应收: {fr:?}");
        assert!(!fr.contains(&"local".to_string()), "同文件裸锚点不归优化器搬运（break_cycles 管）: {fr:?}");
    }

    // ---- preserve_relink_footnotes：跨文件普通尾注（"中间章节点了不跳"的主因）----
    #[test]
    fn crossfile_plain_endnote_relinked() {
        let mut index: HashMap<String, String> = HashMap::new();
        index.insert("n12".to_string(), "第十二条注释文本".to_string());
        let chapter = r#"<html><body><p>正文波波<a href="../notes/notes.xhtml#n12">12</a>后续</p></body></html>"#;
        let out = preserve_relink_footnotes(chapter, &index, crate::optimize::FootnoteMode::Anchor);
        assert!(out.contains(r##"<a href="#n12">12</a>"##), "跨文件 marker 未改成同章锚点: {out}");
        assert!(!out.contains("notes.xhtml"), "跨文件 href 前缀未去掉: {out}");
        assert!(out.contains(r##"<p id="n12">第十二条注释文本</p>"##), "注释未搬进本章章末: {out}");
        // 注释区必须落在 </body> 之内
        let body_end = out.find("</body>").unwrap();
        assert!(out[..body_end].contains(r##"<div class="footnotes">"##), "注释区落到 </body> 外: {out}");
        // Inline 模式：注释就地内联〔…〕、不跳转、无章末 div
        let inl = preserve_relink_footnotes(chapter, &index, crate::optimize::FootnoteMode::Inline);
        assert!(inl.contains("〔第十二条注释文本〕") && !inl.contains(r##"<div class="footnotes">"##) && !inl.contains(r##"href="#n12""##), "Inline 应内联常显不跳转: {inl}");
    }

    #[test]
    fn inline_drops_image_marker() {
        // 《飘》形态：noteref <a> 包着图标 <img>。内联模式必须丢弃图标（否则 xochitl 按固有尺寸渲染=巨大且每条重复）。
        let mut index: HashMap<String, String> = HashMap::new();
        index.insert("fn1".to_string(), "注释文字".to_string());
        let chapter = r##"<p>正文<a epub:type="noteref" href="#fn1"><span class="koboSpan"><img alt="note" src="../Images/i.png"/></span></a>后续</p>"##;
        let out = preserve_relink_footnotes(chapter, &index, crate::optimize::FootnoteMode::Inline);
        assert!(!out.contains("<img"), "内联模式应丢弃图标 marker: {out}");
        assert!(out.contains("〔注释文字〕"), "应内联注释: {out}");
    }

    #[test]
    fn crossfile_nonnote_link_untouched() {
        // 目标不在 index（不是注释）→ 跨文件链接原样不动，绝不误搬目录/交叉引用。
        let index: HashMap<String, String> = HashMap::new();
        let chapter = r#"<p>见<a href="chap03.xhtml#sec2">第三章</a></p>"#;
        assert_eq!(preserve_relink_footnotes(chapter, &index, crate::optimize::FootnoteMode::Anchor), chapter);
    }

    // ---- dedup_ids_in_chapter：跨章 id 撞车（"跳到错章"的次因）----
    #[test]
    fn dedup_renames_colliding_ids_across_chapters() {
        let mut seen = HashSet::new();
        let ch1 = r##"<p>甲<a href="#fn1">1</a></p><p id="fn1">注释甲</p>"##;
        let out1 = dedup_ids_in_chapter(ch1, &mut seen);
        assert_eq!(out1, ch1, "首章 id 不冲突，原样");
        // 第二章又用了 id="fn1" → 必须改名，且本章内 href="#fn1" 同步改
        let ch2 = r##"<p>乙<a href="#fn1">1</a></p><p id="fn1">注释乙</p>"##;
        let out2 = dedup_ids_in_chapter(ch2, &mut seen);
        assert!(!out2.contains(r##"id="fn1""##), "撞车 id 未改名: {out2}");
        // 改名后 marker 与目标仍配对（同一新 id）
        let new_id = Regex::new(r##"id="(fn1-x\d+)""##).unwrap().captures(&out2).map(|c| c.get(1).unwrap().as_str().to_string());
        let new_id = new_id.expect(&format!("未生成唯一新 id: {out2}"));
        assert!(out2.contains(&format!(r##"href="#{new_id}""##)), "本章 href 未随之改名: {out2}");
        assert!(out2.contains("注释乙"), "注释文本不应丢");
    }

    #[test]
    fn dedup_leaves_crossfile_href_alone() {
        // 跨文件 href="f#fn1" 不因本章 id 改名而被动（它指向别的文件）。
        let mut seen = HashSet::new();
        seen.insert("fn1".to_string()); // 假装别章已用过 fn1
        let ch = r#"<p id="fn1">本章注释</p><p>另见<a href="other.xhtml#fn1">跨文件</a></p>"#;
        let out = dedup_ids_in_chapter(ch, &mut seen);
        assert!(!out.contains(r##"<p id="fn1">"##), "本章 id 应改名");
        assert!(out.contains(r##"href="other.xhtml#fn1""##), "跨文件 href 不该被改: {out}");
    }

    // ---- collapse_dup_id_attrs：同元素双 id 属性（非法 XHTML → reMarkable 整章渲染失败）----
    #[test]
    fn collapse_keeps_first_id_and_drops_rest() {
        // 首位是注入锚点(aid/fp)，既存 id 在后 → 保住首位、删后续
        let h = r#"<p id="aid5N3C1" class="calibre6" id="filepos18251">正文</p>"#;
        let out = collapse_dup_id_attrs(h);
        assert_eq!(out, r#"<p id="aid5N3C1" class="calibre6">正文</p>"#, "应保首位 id、删后续: {out}");
        // 无重复 → 原样；不误伤 aid=（不是 id 属性）
        let ok = r#"<p aid="X" id="y">t</p>"#;
        assert_eq!(collapse_dup_id_attrs(ok), ok, "单 id 应原样、aid 不误删: {}", collapse_dup_id_attrs(ok));
        // 三个 id 也只留首个
        let three = r#"<div id="a" id="b" id="c"></div>"#;
        assert_eq!(collapse_dup_id_attrs(three), r#"<div id="a"></div>"#);
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
