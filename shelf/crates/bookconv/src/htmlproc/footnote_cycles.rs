//! 脚注双向互指环的拆解（marker↔注释互相回链会让 xochitl 原生"返回"条失效）。
use super::*;

pub(super) fn block_close_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)</(p|div|h[1-6]|li|blockquote|section|article|ul|ol|table|pre)>").unwrap())
}
pub(super) fn a_open_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?si)<a\b[^>]*>"#).unwrap())
}
pub(super) fn href_attr_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)\s*\bhref="[^"]*""#).unwrap())
}
pub(super) fn href_frag_re() -> &'static Regex {
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
    // `ids_sorted` 按位置升序：二分找"位置 < open_start 的最后一个"（此前每个锚点从头线性扫，单文件大书
    // 几千条脚注是"锚点数 × id 数"）。
    let nearest = |open_start: usize| -> Option<String> {
        let k = ids_sorted.partition_point(|(p, _)| *p < open_start);
        let (bp, id) = ids_sorted.get(k.checked_sub(1)?).map(|(p, id)| (*p, id.as_str()))?;
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
