//! 目录：目录文件判定、自动生成目录（ncx+nav）、已有扁平目录按"第X部"重建为两级。
use super::*;

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

pub(super) fn heading_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    // h1–h6 都收（书只用 h3 当章标题也不漏目录）；多级由 build_ncx/build_nav 按 dense-rank 嵌套。
    RE.get_or_init(|| Regex::new(r#"(?is)<h([1-6])\b([^>]*)>(.*?)</h[1-6]>"#).unwrap())
}

/// 把出现过的 h 级别稠密化为连续深度 1..N（如书用 {h1,h3} → 各条 rank 1/2），供嵌套用。
pub(super) fn dense_ranks(items: &[(u8, String, String, String)]) -> Vec<u8> {
    let mut levels: Vec<u8> = items.iter().map(|i| i.0).collect();
    levels.sort_unstable();
    levels.dedup();
    items.iter().map(|i| (levels.iter().position(|&l| l == i.0).unwrap_or(0) as u8) + 1).collect()
}

pub(crate) fn plain_text(html: &str) -> String {
    static TAG: OnceLock<Regex> = OnceLock::new();
    let tag = TAG.get_or_init(|| Regex::new(r#"(?s)<[^>]*>"#).unwrap());
    let t = tag.replace_all(html, "");
    // 字符引用还原成字符（2026-09-25 审计）：结果是"纯文本"，调用方写进 NCX/nav/OPF 时会再 `xml_escape` 一次——此前不还原，
    // 标题里的 `&amp;`/`&#12288;` 被转义成 `&amp;amp;`/`&amp;#12288;`，自动目录、重建目录、占位书名显示出字面的 `&amp;`。
    let t = t.replace("&nbsp;", " ");
    crate::util::xml_unescape(&t).split_whitespace().collect::<Vec<_>>().join(" ")
}

/// 从 spine 各章 h1/h2 生成目录；标题无 id 则补 `id="cj-toc-N"`。返回条目 (level, title, zip路径, frag)。
pub(super) fn collect_headings(entries: &mut [Entry], spine: &[String], nav_doc: Option<&String>) -> Vec<(u8, String, String, String)> {
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

pub(super) fn build_ncx(items: &[(u8, String, String, String)], ncx_dir: &str, title: &str, uid: &str) -> String {
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

pub(super) fn build_nav(items: &[(u8, String, String, String)], nav_dir: &str) -> String {
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
pub(super) fn toc_href(rel_path: &str, frag: &str) -> String {
    if frag.is_empty() { rel_path.to_string() } else { format!("{rel_path}#{frag}") }
}

/// 标题文本"标题+编号"拆分启发式（EPUB 线原则①：原书标题跟小节/章节编号拼在一行，如「第一章 1」，
/// TOC 要显示成两级——父级标题 + 缩进子级编号）。只在编号看起来像"小节序号"而非"印刷页码残留"时拆：
/// 数字编号要求 ≤99（页码常见三位数以上，且小节编号在同一本书里通常不会突然跳到几十以上）；中文数字编号
/// （〇一二三四五六七八九十百千，常见于章节内小节"之一/之二"变体的「1」以中文数字呈现）不做位数限制，
/// 因为原书不会用中文数字写页码。标题与编号之间的分隔允许普通空格与全角空格（U+3000）。
pub(super) fn split_numbered_title(title: &str) -> Option<(String, String)> {
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
pub(super) fn split_numbered_titles(items: Vec<(u8, String, String, String)>) -> Vec<(u8, String, String, String)> {
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
pub(super) fn fallback_spine_toc(entries: &[Entry], spine: &[String], nav_doc: Option<&String>) -> Vec<(u8, String, String, String)> {
    let pages: Vec<&String> = spine.iter().filter(|p| Some(*p) != nav_doc).collect();
    if pages.is_empty() {
        return Vec::new();
    }
    let texts: Vec<Option<String>> = pages
        .iter()
        .map(|p| {
            let e = entries.iter().find(|e| &&e.name == p)?;
            let html = std::str::from_utf8(&e.data).ok()?;
            let t = plain_text(crate::htmlproc::first_body_inner(html).unwrap_or(""));
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

/// 按 spine 页分段的兜底目录：每 `FALLBACK_TOC_PAGES` 页一条，标题"第 N–M 页"，指向该段第一页。
pub(super) fn page_chunk_toc(spine: &[String], nav_doc: Option<&String>) -> Vec<(u8, String, String, String)> {
    let pages: Vec<&String> = spine.iter().filter(|p| Some(*p) != nav_doc).collect();
    crate::comic_pdf::page_chunk_titles(pages.len())
        .into_iter()
        .map(|(start, title)| (1u8, title, pages[start].clone(), String::new()))
        .collect()
}

/// 分部标题前缀："第X部/卷/篇/辑"（X 为阿拉伯数字或中文数字），后面可能紧跟同一条目剩下的文本
/// （如"第一部　01　雪人"里"01　雪人"是这条目自己的章节标识，不是下一条的）。
pub(super) fn part_prefix_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"^(第[0-9〇一二三四五六七八九十百千]+[部卷篇辑])[ \u{3000}\t]*(.*)$"#).unwrap())
}

/// 从已有 `toc.ncx` 的 navMap 里按文档顺序拍平抽取 (标题, content src) ——不管当前层级，只服务
/// `restructure_existing_toc_parts` 这种"重新看一眼已有目录内容决定要不要升级结构"的场景。
pub(super) fn ncx_navpoint_titles_and_targets(ncx_text: &str) -> Vec<(String, String)> {
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
pub(super) fn restructure_existing_toc_parts(entries: &mut [Entry], mode: AutoToc, rep: &mut WashReport) {
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

pub(super) fn auto_toc(entries: &mut Vec<Entry>, mode: AutoToc, rep: &mut WashReport) {
    if mode == AutoToc::Off || (mode == AutoToc::IfMissing && toc_entry_count(entries) > 0) {
        return;
    }
    let Some(opf) = parse_opf(entries) else { return };
    let headings = collect_headings(entries, &opf.spine, opf.nav_doc.as_ref());
    let headings = if headings.is_empty() { fallback_spine_toc(entries, &opf.spine, opf.nav_doc.as_ref()) } else { split_numbered_titles(headings) };
    // 纯图片书（漫画/画册）：没有标题也没有可提取文字，`fallback_spine_toc` 故意不生成"正文 N"。但用户要求
    // **所有书都要有目录**（2026-09-20，乱马源书 NCX 是空的，转出来没目录），所以按页分段生成"第 N–M 页"
    // ——如实标注不是章节，只为能按段跳转（同 `comic_pdf::page_chunk_titles`，PDF 路径也是这套）。
    let headings = if headings.is_empty() { page_chunk_toc(&opf.spine, opf.nav_doc.as_ref()) } else { headings };
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
