//! AZW3 (KF8) → EPUB，纯 Rust clean-room（依 KF8/MOBI 格式规范 + MobileRead KF8 wiki，
//! **不抄 GPL 的 KindleUnpack 代码**）。
//!
//! 关键简化（真样本验证）：KF8 的 rawML（PalmDOC 解压全部文本记录后拼接）本身**已是重组好的
//! XHTML 文档序列**（skeleton 与其 fragment 交错、按存储序≈阅读序）。故**跳过最复杂的
//! INDX/CNCX skeleton+fragment 重组**——按 `<html>` 边界切章 + 清洗结构标签 + 重写 kindle:embed
//! 图片 + 去 kindle 内链即可，得到可读 EPUB。HUFF/CDIC 压缩与 DRM 明确拒绝。

use super::{common, palm};
use crate::epub::{Book, BookMeta, Chapter, Resource};
use regex::Regex;
use std::collections::{HashMap, HashSet};

/// AZW3 字节 → (优化后的 EPUB 字节, 书名)。
pub fn azw3_to_epub(data: &[u8]) -> Result<(Vec<u8>, String), String> {
    let records = palm::parse_palmdb(data)?;
    if records.len() < 2 {
        return Err("AZW3 记录太少".into());
    }
    let h = palm::parse_header(records[0])?;
    let exth = palm::parse_exth(h.mobi, h.mobi_hlen);

    // 解压文本记录 1..=trecs（剥 trailing bytes）→ rawML
    let raw = palm::decompress_text(&records, &h);
    let rawml = String::from_utf8_lossy(&raw).into_owned();

    // 图片资源：按记录序收集所有图片（JPEG/PNG/GIF）——kindle:embed:NNNN 是 1-based 索引。
    let images = palm::collect_images(&records);
    // 扫 rawML 用到的 embed 号，为其建资源 + 号→路径映射。
    let re_embed_scan = Regex::new(r#"(?i)kindle:embed:0*(\d+)"#).unwrap();
    let mut used: HashSet<usize> = HashSet::new();
    for cap in re_embed_scan.captures_iter(&rawml) {
        if let Ok(n) = cap[1].parse::<usize>() {
            used.insert(n);
        }
    }
    let mut resources: Vec<Resource> = Vec::new();
    let mut embed_path: HashMap<usize, String> = HashMap::new();
    for &n in &used {
        if n == 0 || n > images.len() {
            continue;
        }
        let img = &images[n - 1];
        let path = format!("images/embed{n}.{}", img.ext);
        embed_path.insert(n, path.clone());
        resources.push(Resource { path, media_type: img.mime.into(), bytes: img.bytes.clone() });
    }

    // 内链重映射上下文：fragment 起始表 + aid 位置表，把 kindle:pos 链接转真锚点（脚注/目录跳转可用）。
    let frag_starts = palm::parse_fragment_starts(&records, &h);
    let link_ctx = if frag_starts.is_empty() { None } else { Some(LinkCtx::build(&rawml, frag_starts)) };
    // 切章：有 NCX 真目录则**按 NCX 位置切**（一章一 spine + 一 nav，含层级）；否则按 <html> 块切。
    let ncx = palm::parse_ncx(&records, &h);
    let chapters = build_chapters(&rawml, &embed_path, &ncx, &exth.title, link_ctx.as_ref());
    if chapters.is_empty() || chapters.iter().all(|c| c.html_body.trim().is_empty()) {
        return Err("AZW3 无可读正文（可能是 KF8 变体/加密）".into());
    }

    let title = if exth.title.trim().is_empty() {
        first_title(&rawml)
            .filter(|s| !s.is_empty())
            .or_else(|| Some(palm::palmdb_name(data)))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "未命名".into())
    } else {
        exth.title.clone()
    };
    // 封面：EXTH 201/202 → images 下标；缺省不硬塞（不误挑正文插图当封面）。
    let (cover, cover_ext, cover_media_type) = match palm::pick_cover_index(&images, &exth) {
        Some(i) => (
            Some(images[i].bytes.clone()),
            images[i].ext.to_string(),
            images[i].mime.to_string(),
        ),
        None => (None, "jpg".into(), "image/jpeg".into()),
    };
    let mut book = Book {
        meta: BookMeta {
            book_id: format!("azw3:{}", common::sanitize_id(&title)),
            title: title.clone(),
            author: exth.author,
            language: palm::lang_or_default(&exth.language),
            publisher: exth.publisher,
            cover,
            cover_ext,
            cover_media_type,
        },
        chapters,
        resources,
        nav: Vec::new(),
    };
    let optimized = common::assemble_optimized(&mut book)?;
    Ok((optimized, title))
}

fn first_title(rawml: &str) -> Option<String> {
    let re = Regex::new(r#"(?is)<title[^>]*>(.*?)</title>"#).unwrap();
    re.captures(rawml).map(|c| c[1].trim().to_string()).filter(|s| !s.is_empty())
}

/// KF8 清洗正则集（去结构壳 / kindle:embed 图 / kindle:flow / aid→id 建锚），编译一次复用。
/// **不再去链** `<a>`——内链保留给两遍重映射（`LinkCtx`）转成真 epub 锚点（脚注/目录跳转可用）。
struct Cleaner {
    re_title: Regex,
    re_tag: Regex,
    re_xml: Regex,
    re_head: Regex,
    re_htmlopen: Regex,
    re_shell: Regex,
    re_img: Regex,
    re_flow: Regex,
    re_aid: Regex,
    re_tag_with_aid: Regex,
    re_id_attr: Regex,
}
impl Cleaner {
    fn new() -> Self {
        Cleaner {
            re_title: Regex::new(r#"(?is)<title[^>]*>(.*?)</title>"#).unwrap(),
            re_tag: Regex::new(r#"(?s)<[^>]+>"#).unwrap(),
            re_xml: Regex::new(r#"(?is)<\?xml[^>]*\?>"#).unwrap(),
            re_head: Regex::new(r#"(?is)<head\b.*?</head>"#).unwrap(),
            re_htmlopen: Regex::new(r#"(?is)<html\b[^>]*>"#).unwrap(),
            re_shell: Regex::new(r#"(?is)</?body\b[^>]*>|</html>"#).unwrap(),
            re_img: Regex::new(r#"(?is)<img\b[^>]*\bsrc="kindle:embed:0*(\d+)[^"]*"[^>]*>"#).unwrap(),
            re_flow: Regex::new(r#"(?is)<link\b[^>]*kindle:flow[^>]*>"#).unwrap(),
            // KF8 元素普遍带 aid（唯一）→ 转成 id="aid<X>" 当锚点（内链目标 + 脚注回跳目标）。
            re_aid: Regex::new(r#"(?i)\baid="([^"]+)""#).unwrap(),
            // calibre 做的 AZW3 元素常**同时**带 aid 与既存 id="filepos.../calibre_pb_..."。aid→id 前须先
            // 删该标签既存 id，否则转换后同标签出现两个 id= 属性 = 非法 XHTML → reMarkable 严格 XML 解析
            // 遇重复属性整章失败 → 只渲染前几页（真机《消失的爱人》只 7 页根因，2026-09-01）。KF8 内链走
            // kindle:pos→#aid<X>，既存 filepos id 无链接引用，删之安全。
            re_tag_with_aid: Regex::new(r#"(?is)<[a-z][a-z0-9]*\b[^>]*\baid="[^"]*"[^>]*>"#).unwrap(),
            re_id_attr: Regex::new(r#"(?i)\s+id="[^"]*""#).unwrap(),
        }
    }
    /// 段内 `<title>` 纯文本（KF8 常=书名，仅无 NCX 时作退化标题）。
    fn seg_title(&self, seg: &str) -> String {
        self.re_title
            .captures(seg)
            .map(|c| self.re_tag.replace_all(&c[1], "").trim().chars().take(80).collect())
            .unwrap_or_default()
    }
    /// 去结构壳（可含多个 skeleton 壳）+ 映射 kindle:embed 图 + 去 flow + aid→id 建锚 → 正文 HTML。
    /// **保留 `<a>` 内链原样**（kindle:pos），由 `remap_links` 两遍解析成真锚点。
    fn clean(&self, seg: &str, embed_path: &HashMap<usize, String>) -> String {
        let s = self.re_xml.replace_all(seg, "");
        let s = self.re_head.replace_all(&s, "");
        let s = self.re_htmlopen.replace_all(&s, "");
        let s = self.re_shell.replace_all(&s, "");
        let s = self.re_img.replace_all(&s, |cap: &regex::Captures| {
            let n: usize = cap[1].parse().unwrap_or(0);
            match embed_path.get(&n) {
                Some(p) => format!("<img src=\"{p}\"/>"),
                None => String::new(),
            }
        });
        let s = self.re_flow.replace_all(&s, "");
        // 先在带 aid 的标签上删既存 id（防 aid→id 后重复 id 属性），再做 aid→id。
        let s = self
            .re_tag_with_aid
            .replace_all(&s, |c: &regex::Captures| self.re_id_attr.replace_all(&c[0], "").into_owned());
        let s = self.re_aid.replace_all(&s, r#"id="aid$1""#);
        s.trim().to_string()
    }
}

/// 内链重映射上下文：fragment 起始偏移表 + rawML 中 aid 位置表（排序），把 `kindle:pos:fid:off` 解析成
/// 目标章文件 + `#aid<X>` 锚点。fid=十六进制、off=十进制（多为 0=fragment 首，非零少见；错基顶多落同
/// fragment 内相邻 aid，章级由 fid 精确定位不受影响）。
struct LinkCtx {
    frag_starts: Vec<usize>,
    aid_pos: Vec<(usize, String)>, // (rawML 字节位置, aid)，按位置升序
}
impl LinkCtx {
    fn build(rawml: &str, frag_starts: Vec<usize>) -> Self {
        let re = Regex::new(r#"(?i)\baid="([^"]+)""#).unwrap();
        let mut aid_pos: Vec<(usize, String)> =
            re.captures_iter(rawml).map(|c| (c.get(0).unwrap().start(), c[1].to_string())).collect();
        aid_pos.sort_by_key(|x| x.0);
        LinkCtx { frag_starts, aid_pos }
    }
    /// (fid, off) → (目标 rawML 偏移, 目标 aid)。找 ≤ 目标偏移的最近 aid（= 含该位置的元素）。
    fn resolve(&self, fid: usize, off: usize) -> Option<(usize, String)> {
        let base = *self.frag_starts.get(fid)?;
        let t = base + off;
        let idx = self.aid_pos.partition_point(|(p, _)| *p <= t);
        if idx == 0 {
            return None;
        }
        Some((t, self.aid_pos[idx - 1].1.clone()))
    }
}

/// 两遍第二遍：把各章 HTML 里保留的 `href="kindle:pos:fid:F:off:O"` 重写成 `chap_N.xhtml#aid<X>`。
/// 只换 href 属性值（保留 `<a>` 其余属性——尤其自身 id，是脚注回跳的目标）。解析不出 → `href="#"`（惰性）。
/// `ch_ranges`=各最终章 rawML [start,end) 定位目标章；`live_ids`=清洗后实际存活的 id 集合——目标 aid
/// 在 shell 元素上（body/html，已被剥）时锚点不存在，退回只跳章文件（对指向 fragment 首的 TOC 链正确）。
fn remap_links(
    html: &str,
    ctx: &LinkCtx,
    ch_ranges: &[(usize, usize)],
    live_ids: &HashSet<String>,
) -> String {
    // 每章调用一次：正则只编译一次（此前每章各编译两个，几百章的书白白多几百次编译）。
    static RE_POS: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    static RE_OTHER: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = RE_POS.get_or_init(|| Regex::new(r#"(?i)href="kindle:pos:fid:([0-9A-Fa-f]+):off:([0-9]+)""#).unwrap());
    let re_other = RE_OTHER.get_or_init(|| Regex::new(r#"(?i)href="kindle:[^"]*""#).unwrap());
    let s = re.replace_all(html, |cap: &regex::Captures| {
        let fid = usize::from_str_radix(&cap[1], 16).unwrap_or(usize::MAX);
        let off = cap[2].parse::<usize>().unwrap_or(0);
        match ctx.resolve(fid, off) {
            Some((t, aid)) => {
                let ci = ch_ranges.partition_point(|(a, _)| *a <= t).saturating_sub(1);
                let file = crate::epub::chapter_filename(ci);
                let id = format!("aid{aid}");
                if live_ids.contains(&id) {
                    format!(r#"href="{file}#{id}""#)
                } else {
                    format!(r#"href="{file}""#) // 锚在被剥的壳上 → 跳章首
                }
            }
            None => r##"href="#""##.to_string(),
        }
    });
    // 其余 kindle: 内链（非 pos）→ 惰性 #，避免残留 kindle: 死链
    re_other.replace_all(&s, r##"href="#""##).into_owned()
}

/// 把 rawML 切成章：
/// - **有 NCX 真目录**：按 NCX 位置切（吸附到 pos 前最近的 `<` 避免切进标签中间），一章=一 spine+一 nav，
///   标题=真章名、层级=NCX 层级+1；首条 NCX 前的前置内容（封面/版权页）作无标题段（进 spine 不进 nav）。
///   **能把塞进同一 skeleton 的多章拆开**（如《恶意》24 章仅 11 个 skeleton 块）。
/// - **无 NCX**：退化按 `<html>` 块切；标题取块内 `<title>`，等于书名的清空（避免满屏书名重复）。
fn build_chapters(
    rawml: &str,
    embed_path: &HashMap<usize, String>,
    ncx: &[palm::NcxEntry],
    book_title: &str,
    link_ctx: Option<&LinkCtx>,
) -> Vec<Chapter> {
    let cl = Cleaner::new();
    // 第一遍：切段 + 清洗（含 aid→id、保留 kindle:pos 链），记录每章的 rawML [start,end)。
    // 元组 = (Chapter, rawML 段起点)；段终点由下一章起点/rawml 末尾给出。
    let mut built: Vec<(Chapter, usize)> = Vec::new();

    if !ncx.is_empty() {
        // 按 NCX **精确位置**切（各章位置互异，不吸附以免邻近章被去重合并丢章）。切点可能落在标签中间，
        // 由 trim_partial_head 裁掉段首残缺标签片段。
        let mut cuts: Vec<(usize, String, i64)> = ncx
            .iter()
            .filter(|e| e.pos <= rawml.len())
            .map(|e| {
                // NCX 偏移是字节位置，可能落在多字节字符中间 → 下取到字符边界（最多回退 3 字节）
                (char_floor(rawml, e.pos), e.label.chars().take(80).collect::<String>(), (e.level as i64 + 1).clamp(1, 6))
            })
            .collect();
        cuts.sort_by_key(|c| c.0);
        cuts.dedup_by_key(|c| c.0); // 仅去完全相同位置

        // 首条 NCX 前的前置内容（封面/版权页）→ 无标题段（进 spine 不进 nav），rawML 起点 0
        let first_cut = cuts[0].0;
        if first_cut > 0 {
            let html = cl.clean(&rawml[..first_cut], embed_path);
            if !html.is_empty() {
                built.push((Chapter { title: String::new(), html_body: html, level: 1 }, 0));
            }
        }
        for i in 0..cuts.len() {
            let start = cuts[i].0;
            let end = if i + 1 < cuts.len() { cuts[i + 1].0 } else { rawml.len() };
            let seg = trim_partial_head(&rawml[start..end]);
            let html = cl.clean(seg, embed_path);
            if html.is_empty() {
                continue;
            }
            built.push((Chapter { title: cuts[i].1.clone(), html_body: html, level: cuts[i].2 }, start));
        }
    } else {
        // 无 NCX：按 <html> 块切
        let lower = rawml.to_ascii_lowercase();
        let mut starts: Vec<usize> = Vec::new();
        let mut from = 0;
        while let Some(rel) = lower[from..].find("<html") {
            let pos = from + rel;
            starts.push(pos);
            from = pos + 5;
        }
        if starts.is_empty() {
            starts.push(0);
        }
        for i in 0..starts.len() {
            let end = if i + 1 < starts.len() { starts[i + 1] } else { rawml.len() };
            let seg = &rawml[starts[i]..end];
            let mut title = cl.seg_title(seg);
            if title == book_title {
                title.clear(); // 等于书名 → 清空，避免满屏重复
            }
            let html = cl.clean(seg, embed_path);
            if html.is_empty() {
                continue;
            }
            built.push((Chapter { title, html_body: html, level: 1 }, starts[i]));
        }
    }

    // 第二遍：内链重映射（有 fragment/aid 上下文时）。ch_ranges = 各最终章 rawML [start, end)。
    if let Some(ctx) = link_ctx {
        let n = built.len();
        let ch_ranges: Vec<(usize, usize)> = (0..n)
            .map(|i| (built[i].1, if i + 1 < n { built[i + 1].1 } else { rawml.len() }))
            .collect();
        // 收集清洗后实际存活的 id="aid..."（shell 上的 aid 已被剥，不在此集）→ 决定锚点 vs 跳章首。
        let re_id = Regex::new(r#"(?i)\bid="(aid[^"]+)""#).unwrap();
        let mut live_ids: HashSet<String> = HashSet::new();
        for (ch, _) in &built {
            for c in re_id.captures_iter(&ch.html_body) {
                live_ids.insert(c[1].to_string());
            }
        }
        for (ch, _) in built.iter_mut() {
            ch.html_body = remap_links(&ch.html_body, ctx, &ch_ranges, &live_ids);
        }
    }
    built.into_iter().map(|(c, _)| c).collect()
}

/// 把字节位置下取到 `<` 最近的字符边界（`str::floor_char_boundary` 未稳定，手写）。
fn char_floor(s: &str, mut pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

/// 若段首落在标签中间（首字符非 `<`，且第一个 `>` 出现在第一个 `<` 之前），裁掉这段残缺标签片段
/// （到首个 `>` 之后），避免像 `2">正文` 的属性残片当正文渲染。段首本就是 `<` 则原样返回。
fn trim_partial_head(seg: &str) -> &str {
    if seg.as_bytes().first() == Some(&b'<') {
        return seg;
    }
    let lt = seg.find('<');
    let gt = seg.find('>');
    match (lt, gt) {
        (Some(l), Some(g)) if g < l => &seg[g + 1..],
        (None, Some(g)) => &seg[g + 1..],
        _ => seg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleaner_strips_shell_and_maps_embed() {
        let mut ep = HashMap::new();
        ep.insert(2usize, "images/embed2.jpg".to_string());
        let seg = r#"<?xml version="1.0"?><html xmlns="x"><head><title>第一章</title><link href="kindle:flow:0001"/></head><body aid="0"><p>正文</p></body></html><img src="kindle:embed:0002?mime=image/jpeg"/>"#;
        let cl = Cleaner::new();
        assert_eq!(cl.seg_title(seg), "第一章");
        let b = cl.clean(seg, &ep);
        assert!(b.contains("<p>正文</p>"), "{b}");
        assert!(b.contains("<img src=\"images/embed2.jpg\"/>"), "{b}");
        assert!(!b.contains("kindle:") && !b.contains("<head") && !b.contains("<body") && !b.contains("<html"), "{b}");
    }

    #[test]
    fn build_chapters_ncx_splits_within_one_skeleton() {
        // 单个 <html> 块里塞两章，NCX 两条 → 按位置切成两章、各拿真名+层级（验《恶意》式多章一块）
        let ep = HashMap::new();
        let rawml = r#"<html><body><p>甲章正文</p><p>乙章正文</p></body></html>"#;
        let p_yi = rawml.find("<p>乙章正文").unwrap();
        let ncx = vec![
            palm::NcxEntry { pos: rawml.find("<p>甲章正文").unwrap(), label: "甲章".into(), level: 0 },
            palm::NcxEntry { pos: p_yi, label: "乙章".into(), level: 1 },
        ];
        let chs = build_chapters(rawml, &ep, &ncx, "书名", None);
        let titled: Vec<_> = chs.iter().filter(|c| !c.title.is_empty()).collect();
        assert_eq!(titled.len(), 2, "两章都在: {:?}", chs.iter().map(|c| &c.title).collect::<Vec<_>>());
        assert_eq!(titled[0].title, "甲章");
        assert_eq!(titled[0].level, 1);
        assert_eq!(titled[1].title, "乙章");
        assert_eq!(titled[1].level, 2);
        assert!(titled[0].html_body.contains("甲章正文") && !titled[0].html_body.contains("乙章正文"));
        assert!(titled[1].html_body.contains("乙章正文"));
    }

    #[test]
    fn build_chapters_no_ncx_dedups_booktitle() {
        // 无 NCX → 按 <html> 块切，块 <title>=书名的清空、非书名保留
        let ep = HashMap::new();
        let rawml = r#"<html><head><title>书名</title></head><body><p>a</p></body></html><html><head><title>序言</title></head><body><p>b</p></body></html>"#;
        let chs = build_chapters(rawml, &ep, &[], "书名", None);
        assert_eq!(chs.len(), 2);
        assert_eq!(chs[0].title, "");
        assert_eq!(chs[1].title, "序言");
    }

    #[test]
    fn trim_partial_head_drops_attr_remnant() {
        assert_eq!(trim_partial_head(r#"22">正文<p>x</p>"#), "正文<p>x</p>"); // 切进标签中间
        assert_eq!(trim_partial_head("<p>正文</p>"), "<p>正文</p>"); // 段首本是标签
        assert_eq!(trim_partial_head("纯文本无标签"), "纯文本无标签"); // 无 '>' 原样
    }

    #[test]
    fn cleaner_converts_aid_to_id_and_keeps_links() {
        let ep = HashMap::new();
        let cl = Cleaner::new();
        let seg = r#"<html><body><p aid="X1">注<a href="kindle:pos:fid:0002:off:0000000005" aid="X2">[1]</a></p></body></html>"#;
        let b = cl.clean(seg, &ep);
        assert!(b.contains(r#"id="aidX1""#), "aid→id: {b}");
        assert!(b.contains(r#"id="aidX2""#), "链接自身 aid→id(回跳锚): {b}");
        assert!(b.contains("kindle:pos:fid:0002"), "clean 阶段保留内链待重映射: {b}");
    }

    #[test]
    fn cleaner_no_dup_id_when_element_has_existing_id() {
        // calibre 做的 AZW3：元素同时带 aid 与既存 id="filepos..."（真机《消失的爱人》崩因）
        let ep = HashMap::new();
        let cl = Cleaner::new();
        let seg = r#"<html><body><p aid="5N3C1" class="calibre6" id="filepos18251">正文</p><div id="calibre_pb_6" class="mbppagebreak" aid="5N3C4"></div></body></html>"#;
        let b = cl.clean(seg, &ep);
        // 每个标签只能有一个 id 属性（否则非法 XHTML → reMarkable 整章渲染失败）
        for tag in b.split('>') {
            let cnt = tag.matches(" id=").count() + if tag.starts_with("id=") { 1 } else { 0 };
            assert!(cnt <= 1, "标签出现重复 id 属性: <{tag}> in {b}");
        }
        // 锚点保留为 aid 派生的 id（内链目标），既存 filepos/calibre_pb id 被删
        assert!(b.contains(r#"id="aid5N3C1""#), "aid 锚点应保留: {b}");
        assert!(b.contains(r#"id="aid5N3C4""#), "aid 锚点应保留: {b}");
        assert!(!b.contains("filepos18251"), "既存 filepos id 应删除: {b}");
    }

    #[test]
    fn remap_links_resolves_kindle_pos_to_anchor() {
        // fragment 2 起于 rawML 100；aid "A2" 在位置 100 → kindle:pos:fid:2:off:0 → chap#? #aidA2
        let ctx = LinkCtx {
            frag_starts: vec![0, 50, 100],
            aid_pos: vec![(0, "A0".into()), (50, "A1".into()), (100, "A2".into())],
        };
        // 两章：[0,80) 与 [80,200)。目标 rawoff=100 落第二章(idx1)。
        let ranges = vec![(0usize, 80usize), (80usize, 200usize)];
        let live: HashSet<String> = ["aidA2".to_string()].into_iter().collect();
        let html = r#"<a href="kindle:pos:fid:0002:off:0000000000">跳</a>"#;
        let out = remap_links(html, &ctx, &ranges, &live);
        assert!(out.contains(r#"href="chap_0002.xhtml#aidA2""#), "{out}");
        // 目标 aid 不在存活集（shell 上被剥）→ 退回只跳章文件
        let no_anchor = remap_links(html, &ctx, &ranges, &HashSet::new());
        assert!(no_anchor.contains(r#"href="chap_0002.xhtml""#) && !no_anchor.contains("#aid"), "{no_anchor}");
        // 解析不出（fid 越界）→ 惰性 #
        let bad = remap_links(r#"<a href="kindle:pos:fid:00FF:off:0000000000">x</a>"#, &ctx, &ranges, &live);
        assert!(bad.contains(r##"href="#""##), "{bad}");
    }
}
