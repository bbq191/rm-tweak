//! 空页清理：删掉纯空白页并把目录/spine 引用改指向邻页。
use super::*;

// ───────────────────────── 5. 空页清理 ─────────────────────────

/// 页面 body 里既无媒体标签、去掉标签后也只剩空白（含 `&nbsp;`/`&#160;`/U+00A0）。
/// 2026-09-24 审计：此前正文复制 → 小写复制 → 正则去标签 → 三次 replace 各出一份整章副本，是清洗层最大的一块耗时；
/// 现在只在去标签时拼一份，其余按字节扫描，判定结果不变（`is_empty_page_matches_old_regex_impl` 对拍）。
pub(super) fn is_empty_page(html: &str) -> bool {
    let inner = crate::htmlproc::first_body_inner(html).unwrap_or("");
    let has = |pat: &[u8]| inner.as_bytes().windows(pat.len()).any(|w| w.eq_ignore_ascii_case(pat));
    if has(b"<img") || has(b"<svg") || has(b"<image") || has(b"<video") || has(b"<audio") {
        return false;
    }
    // 去标签（同正则 `<[^>]*>`：`<` 到其后第一个 `>`；没有 `>` 的 `<` 留作普通字符）。
    let mut text = String::with_capacity(inner.len());
    let mut rest = inner;
    while let Some(lt) = rest.find('<') {
        let Some(gt) = rest[lt..].find('>') else { break };
        text.push_str(&rest[..lt]);
        rest = &rest[lt + gt + 1..];
    }
    text.push_str(rest);
    // 其余只能是空白或不换行空格实体（去标签后才认实体：`&nb<i>sp;` 拼出来的也算，与原先先去标签再替换一致）。
    let mut s = text.as_str();
    while let Some(c) = s.chars().next() {
        if let Some(r) = s.strip_prefix("&nbsp;").or_else(|| s.strip_prefix("&#160;")) {
            s = r;
        } else if c.is_whitespace() {
            s = &s[c.len_utf8()..];
        } else {
            return false;
        }
    }
    true
}

/// 一遍扫描删掉 OPF 里 `id`/`idref` 属于 `ids` 的 `<item>`/`<itemref>`（连同紧随的空白与空闭合标签）。
/// 此前对每个被删页各编译两个正则、各整份扫描一遍 OPF：Calibre MOBI 转出的书常有上千个 `mbppagebreak` 独占页，
/// 即上千次编译 + 上千次整份 OPF 扫描（`bench_tmp emptypages`：1500 页/750 空页，host 380ms → 见提交说明）。
pub(super) fn drop_opf_refs(opf: &str, ids: &HashSet<&str>) -> String {
    static ITEMREF: OnceLock<Regex> = OnceLock::new();
    static ITEM: OnceLock<Regex> = OnceLock::new();
    static IDREF_ATTR: OnceLock<Regex> = OnceLock::new();
    static ID_ATTR: OnceLock<Regex> = OnceLock::new();
    let itemref = ITEMREF.get_or_init(|| Regex::new(r#"<itemref\b[^>]*>(?:\s*</itemref>)?\s*"#).unwrap());
    let item = ITEM.get_or_init(|| Regex::new(r#"<item\b[^>]*>(?:\s*</item>)?\s*"#).unwrap());
    let idref_attr = IDREF_ATTR.get_or_init(|| Regex::new(r#"\bidref="([^"]*)""#).unwrap());
    let id_attr = ID_ATTR.get_or_init(|| Regex::new(r#"\bid="([^"]*)""#).unwrap());
    let drop_if = |c: &regex::Captures, attr: &Regex| -> String {
        let whole = c.get(0).unwrap().as_str();
        let tag_end = whole.find('>').map_or(whole.len(), |i| i + 1);
        if attr.captures_iter(&whole[..tag_end]).any(|a| ids.contains(&a[1])) {
            String::new()
        } else {
            whole.to_string()
        }
    };
    let out = itemref.replace_all(opf, |c: &regex::Captures| drop_if(c, idref_attr));
    item.replace_all(&out, |c: &regex::Captures| drop_if(c, id_attr)).into_owned()
}

pub(super) fn remove_empty_pages(entries: &mut Vec<Entry>, rep: &mut WashReport) {
    let Some(opf) = parse_opf(entries) else { return };
    let mut removed: Vec<String> = Vec::new();
    // 条目名索引建一次（此前每个 spine 页线性找一遍全书条目，几千页漫画是"页数 × 条目数"次比较；同名取第一条）。
    let mut by_name: HashMap<&str, &Entry> = HashMap::with_capacity(entries.len());
    for e in entries.iter() {
        by_name.entry(e.name.as_str()).or_insert(e);
    }
    for p in &opf.spine {
        if Some(p) == opf.nav_doc.as_ref() {
            continue;
        }
        if let Some(e) = by_name.get(p.as_str()) {
            if is_html_entry(&e.name, &e.data) && is_empty_page(&String::from_utf8_lossy(&e.data)) {
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
    let ids: HashSet<&str> = opf.items.iter().filter(|(_, v)| removed_set.contains(v)).map(|(k, _)| k.as_str()).collect();
    let text = drop_opf_refs(&String::from_utf8_lossy(&entries[opf.index].data), &ids);
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
