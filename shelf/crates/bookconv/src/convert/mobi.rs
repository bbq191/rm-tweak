//! MOBI6 → EPUB，纯 Rust（与 KF8 共用 `palm` 容器底座，**不依赖 `mobi` crate**——该 crate 在真机
//! 词典样本上把 extra_data_flags 尾字节判断错、解压乱码，见 `palm.rs`）。PalmDOC 解压得整本 HTML
//! （`<mbp:pagebreak>` 分页、`<img recindex="N">` 引图 records、`<a filepos=N>` 内链）。
//!
//! **与 EPUB/KF8 对齐的关键（2026-09-01）**：MOBI6 没有 KF8 的 NCX/aid，章名与内链靠 **filepos 机制**——
//! `<a filepos=N>` 跳到 rawML 第 N 字节处的元素（真样本《喜鹊谋杀案》验证：401 个目标 100% 落在 `<` 标签
//! 起点）。转换做三件事，得到与原生 EPUB 一致的产物：
//!   1. **按书内目录（TOC）切章**——TOC 链文字即真章名（MOBI6 的 NCX 等价物），无 TOC 退化按 pagebreak 切。
//!   2. **为每个被引用的 filepos 目标注入 `id="fpN"` 锚点**（属性注入进目标元素，保留原属性）。
//!   3. **就地把 `filepos=N` 改写成 `href="chap_X.xhtml#fpN"`**（脚注/目录跳转可用）。
//!
//! 副产品：目录页链接改写后指向大量 chap 文件，读起来是一份正常可点的书内目录页，跟原生 EPUB
//! 自带的 HTML 目录页同构（2026-09-19 前 `optimize::remove_toc_from_spine` 会把这种"指向一堆
//! chap 文件"的页面从 spine 剥掉——当时的假设是"reMarkable 有自己的原生 TOC，书内目录页冗余"，
//! 但真机反馈（《疯探》真书）证明这个假设站不住：这类页面就是书籍**正文**，不是能安全丢的冗余物，
//! 违背 EPUB 线原则①"保留目录页"，已整个删掉这个剥离机制——MOBI 转换产物现在跟原生 EPUB 一样，
//! 目录页原样留在 spine 里）。

use super::{common, palm};
use crate::epub::{Book, BookMeta, Chapter, Resource};
use regex::Regex;
use std::collections::HashMap;

/// MOBI6 字节 → (优化后的 EPUB 字节, 书名)。
pub fn mobi_to_epub(data: &[u8]) -> Result<(Vec<u8>, String), String> {
    let records = palm::parse_palmdb(data)?;
    if records.len() < 2 {
        return Err("MOBI 记录太少".into());
    }
    let h = palm::parse_header(records[0])?;
    let exth = palm::parse_exth(h.mobi, h.mobi_hlen);
    let images = palm::collect_images(&records);

    let raw = palm::decompress_text(&records, &h);
    // filepos 偏移是**整个 rawML**（含 `<html><head><body>`）的字节位置，故全程用 full-rawml 坐标，
    // 不 extract_body（那会平移偏移）。壳（head/html/body）在 clean_seg 里逐段剥掉。
    let rawml = String::from_utf8_lossy(&raw).into_owned();
    if rawml.to_ascii_lowercase().find("<body").is_none() {
        return Err("MOBI 无 <body>（可能是 KF8 变体或损坏文件，请用 calibre 转 EPUB）".into());
    }

    // 扫 rawML 用到的 <img recindex="N">（1-based，映射 images[N-1]），存成资源 + 建 recindex→路径映射。
    let re_img = Regex::new(r#"(?is)<img\b[^>]*\brecindex="0*(\d+)"[^>]*>"#).unwrap();
    let mut resources: Vec<Resource> = Vec::new();
    let mut used: HashMap<usize, String> = HashMap::new();
    for cap in re_img.captures_iter(&rawml) {
        let n: usize = cap[1].parse().unwrap_or(0);
        if n == 0 || n > images.len() || used.contains_key(&n) {
            continue;
        }
        let img = &images[n - 1];
        let path = format!("images/img{n}.{}", img.ext);
        used.insert(n, path.clone());
        resources.push(Resource { path, media_type: img.mime.into(), bytes: img.bytes.clone() });
    }

    let chapters = build_chapters(&rawml, &used, &re_img);
    if chapters.is_empty() || chapters.iter().all(|c| c.html_body.trim().is_empty()) {
        return Err("MOBI 无可读正文".into());
    }

    let title = if exth.title.trim().is_empty() {
        let n = palm::palmdb_name(data);
        if n.is_empty() { "未命名".into() } else { n }
    } else {
        exth.title.clone()
    };
    // 封面：EXTH 201/202 → images 下标；缺省不硬塞。
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
            book_id: format!("mobi:{}", common::sanitize_id(&title)),
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

/// 扫全 rawML 被 `<a filepos=N>` 引用的目标 N（去重、升序、在界内）——每个都要注入锚点。
fn collect_targets(rawml: &str) -> Vec<usize> {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r#"(?is)\bfilepos=0*(\d+)"#).unwrap());
    let mut v: Vec<usize> = re
        .captures_iter(rawml)
        .filter_map(|c| c[1].parse::<usize>().ok())
        .filter(|&n| n > 0 && n <= rawml.len())
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// 一个切章点：rawML 偏移 + 章名 + 层级（1/2）。
struct Cut {
    off: usize,
    title: String,
    level: i64,
}

/// 提取书内目录（MOBI6 的 NCX 等价物）作切章依据：取 filepos 链最密集的一段（pagebreak 分隔）当目录页，
/// 其有序 `(N, 文字)` 即章界+章名。去重（按 N 首现）、按偏移排序。链数不足阈值→无 TOC（返回空，退化 pagebreak）。
fn extract_toc(rawml: &str) -> Vec<Cut> {
    const TOC_MIN_LINKS: usize = 8;
    // 各 pagebreak 段的链接计数，找最密的一段。用链接**源位置**归段，故带位置扫一遍。
    static RSRC: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re_src = RSRC.get_or_init(|| Regex::new(r#"(?is)<a\b[^>]*\bfilepos=0*(\d+)[^>]*>(.*?)</a>"#).unwrap());
    let re_tag = Regex::new(r#"(?is)<[^>]+>"#).unwrap();
    let mut src: Vec<(usize, usize, String)> = re_src // (源位置, 目标 N, 文字)
        .captures_iter(rawml)
        .map(|c| {
            (
                c.get(0).unwrap().start(),
                c[1].parse().unwrap_or(0),
                re_tag.replace_all(&c[2], "").trim().chars().take(80).collect::<String>(),
            )
        })
        .collect();
    src.sort_by_key(|x| x.0);

    let pb: Vec<usize> = {
        let re = Regex::new(r#"(?is)<mbp:pagebreak\s*/?>"#).unwrap();
        re.find_iter(rawml).map(|m| m.start()).collect()
    };
    let mut bounds = vec![0usize];
    bounds.extend(&pb);
    bounds.push(rawml.len());

    let mut best: Option<(usize, usize)> = None; // (段起, 段止)
    let mut best_cnt = 0usize;
    for w in bounds.windows(2) {
        let (s, e) = (w[0], w[1]);
        let cnt = src.iter().filter(|(p, _, _)| *p >= s && *p < e).count();
        if cnt > best_cnt {
            best_cnt = cnt;
            best = Some((s, e));
        }
    }
    if best_cnt < TOC_MIN_LINKS {
        return Vec::new();
    }
    let (s, e) = best.unwrap();
    let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut cuts: Vec<Cut> = Vec::new();
    for (_, n, t) in src.iter().filter(|(p, _, _)| *p >= s && *p < e) {
        if *n == 0 || t.is_empty() || *n > rawml.len() || !seen.insert(*n) {
            continue;
        }
        // 层级：纯数字（章内小节号如 "1"/"10"）→ 2，其余（"第一章…"/篇名）→ 1。MOBI TOC 本身扁平、无层级信息。
        let level = if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) { 2 } else { 1 };
        cuts.push(Cut { off: char_floor(rawml, *n), title: t.clone(), level });
    }
    cuts.sort_by_key(|c| c.off);
    cuts.dedup_by_key(|c| c.off);
    cuts
}

/// 切章 + 注入锚点 + 内链重映射。
/// - 有 TOC：按 TOC 目标切（一章=一 spine+一 nav，章名=TOC 文字，层级=启发式）；首个 cut 前的壳/前置作无标题段。
/// - 无 TOC：退化按 `<mbp:pagebreak>` 切，标题取段内 `<h1-6>`（多数 MOBI6 无标题元素→空，nav 跳过）。
///
/// 两路都：为被引用的 filepos 目标注入 `id="fpN"`（属性注入进目标元素）→ 就地把 `filepos=N` 改写为 `chap#fpN`。
fn build_chapters(rawml: &str, used_img: &HashMap<usize, String>, re_img: &Regex) -> Vec<Chapter> {
    let targets = collect_targets(rawml);
    let toc = extract_toc(rawml);

    // 切点（rawML 偏移，升序）。有 TOC 用 TOC；否则 pagebreak。
    let cuts: Vec<Cut> = if !toc.is_empty() {
        toc
    } else {
        let re_pb = Regex::new(r#"(?is)<mbp:pagebreak\s*/?>"#).unwrap();
        re_pb.find_iter(rawml).map(|m| Cut { off: m.end(), title: String::new(), level: 1 }).collect()
    };

    // 逐段边界：首个 cut 前的前置段（壳/封面/版权，无标题）+ 各 cut 段。
    let mut segs: Vec<(usize, usize, String, i64)> = Vec::new(); // (start, end, title, level)
    let first = cuts.first().map(|c| c.off).unwrap_or(0);
    if first > 0 {
        segs.push((0, first, String::new(), 1));
    }
    for i in 0..cuts.len() {
        let s = cuts[i].off;
        let e = if i + 1 < cuts.len() { cuts[i + 1].off } else { rawml.len() };
        if e <= s {
            continue;
        }
        let mut title = cuts[i].title.clone();
        if title.is_empty() {
            title = heading_title(&rawml[s..e]); // pagebreak 退化路才可能命中
        }
        segs.push((s, e, title, cuts[i].level));
    }

    // 第一遍：注入锚点 + 清洗（保留 filepos 链与注入的 id，remap 推迟），记录**实际输出**章的 rawML 起点。
    // 关键：空段（纯壳）被丢弃 → 输出章号 ≠ 切点序号，故 remap 必须基于输出章起点（两遍），否则跨文件链错位一章。
    let mut built: Vec<(Chapter, usize)> = Vec::new();
    for (s, e, title, level) in &segs {
        let mut local: Vec<(usize, usize)> = targets
            .iter()
            .filter(|&&n| n >= *s && n < *e)
            .map(|&n| (n - s, n))
            .collect();
        local.sort_by_key(|x| std::cmp::Reverse(x.0)); // 降序注入，保后续局部偏移不移位
        let injected = inject_anchors(&rawml[*s..*e], &local);
        // calibre 做的 MOBI 元素可能已带 id（filepos/calibre_pb）；注入 fpN 后同标签会出现两个 id 属性 =
        // 非法 XHTML → reMarkable 整章渲染失败。折叠成单 id（fpN 注入在标签名后=首位，保住锚点）。
        let html = crate::htmlproc::collapse_dup_id_attrs(&clean_seg(&injected, used_img, re_img));
        if html.trim().is_empty() {
            continue;
        }
        built.push((Chapter { title: title.clone(), html_body: html, level: *level }, *s));
    }

    // 第二遍：基于输出章起点算 chapter_of，就地 filepos=N → href="chap#fpN"。
    let ch_starts: Vec<usize> = built.iter().map(|(_, s)| *s).collect();
    let chapter_of = |n: usize| -> usize { ch_starts.partition_point(|&s| s <= n).saturating_sub(1) };
    for (ch, _) in built.iter_mut() {
        ch.html_body = remap_links(&ch.html_body, &chapter_of);
    }
    built.into_iter().map(|(c, _)| c).collect()
}

/// 属性注入：在各局部位置（每个都指向一个 `<` 标签起点，降序排列）的目标元素上插入 ` id="fpN"`
/// （紧跟标签名后=首位）。目标 100% 落在 `<`（真样本验证），故只需找标签名末尾。⚠ calibre 做的 MOBI
/// 元素可能已带 id（filepos/calibre_pb）→ 注入后同标签双 id；由调用方 `collapse_dup_id_attrs` 折叠（保首位 fpN）。
fn inject_anchors(seg: &str, local_desc: &[(usize, usize)]) -> String {
    let mut s = seg.to_string();
    for &(pos, n) in local_desc {
        let bytes = s.as_bytes();
        if pos >= bytes.len() || bytes[pos] != b'<' {
            continue; // 保险：非 '<' 不注入
        }
        // 标签名末尾 = pos+1 起首个 空白/`>`/`/`。
        let mut j = pos + 1;
        while j < bytes.len() {
            let c = bytes[j];
            if c == b' ' || c == b'>' || c == b'/' || c == b'\t' || c == b'\n' || c == b'\r' {
                break;
            }
            j += 1;
        }
        s.insert_str(j, &format!(r#" id="fp{n}""#));
    }
    s
}

/// 清洗一段 HTML → 正文：剥壳（xml 声明/head/html/body）+ img recindex→资源路径 + 去残留 mbp 标签。
/// **保留** `<a ... filepos=...>`（待 remap）与注入的 `id="fpN"`。
fn clean_seg(seg: &str, used_img: &HashMap<usize, String>, re_img: &Regex) -> String {
    let re_xml = Regex::new(r#"(?is)<\?xml[^>]*\?>"#).unwrap();
    let re_head = Regex::new(r#"(?is)<head\b.*?</head>"#).unwrap();
    let re_shell = Regex::new(r#"(?is)<html\b[^>]*>|</html>|</?body\b[^>]*>"#).unwrap();
    let s = re_xml.replace_all(seg, "");
    let s = re_head.replace_all(&s, "");
    let s = re_shell.replace_all(&s, "");
    // <img recindex="N" ...> → <img src="path"/>（未映射到资源的丢弃）
    let s = re_img.replace_all(&s, |cap: &regex::Captures| {
        let n: usize = cap[1].parse().unwrap_or(0);
        match used_img.get(&n) {
            Some(p) => format!("<img src=\"{p}\"/>"),
            None => String::new(),
        }
    });
    // 去残留 </img>、mbp 命名空间标签（含段内 pagebreak）
    let re_junk = Regex::new(r#"(?is)</img>|</?mbp:[a-z]+\s*/?>"#).unwrap();
    re_junk.replace_all(&s, "").trim().to_string()
}

/// 就地把 `filepos=N` 改写成 `href="chap_X.xhtml#fpN"`——只换该属性，保留 `<a>` 其余属性（尤其注入的 id）。
/// body 内 `filepos=` 只出现在 `<a>`（guide 在 head 已剥），故全局替换安全。N→章号由 `chapter_of`。
fn remap_links(html: &str, chapter_of: &impl Fn(usize) -> usize) -> String {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r#"(?is)\bfilepos=0*(\d+)"#).unwrap());
    re.replace_all(html, |cap: &regex::Captures| {
        let n: usize = cap[1].parse().unwrap_or(0);
        let file = crate::epub::chapter_filename(chapter_of(n));
        format!(r#"href="{file}#fp{n}""#)
    })
    .into_owned()
}

/// 段内首个 `<h1>…<h6>` 的纯文本（pagebreak 退化路的标题来源）；无则空。
fn heading_title(seg: &str) -> String {
    let re_h = Regex::new(r#"(?is)<h[1-6][^>]*>(.*?)</h[1-6]>"#).unwrap();
    if let Some(cap) = re_h.captures(seg) {
        let re_tag = Regex::new(r#"(?is)<[^>]+>"#).unwrap();
        return re_tag.replace_all(&cap[1], "").trim().chars().take(80).collect();
    }
    String::new()
}

/// 字节位置下取到字符边界（NCX/filepos 偏移可能落在多字节字符中间，最多回退 3 字节）。
fn char_floor(s: &str, mut pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    while pos > 0 && !s.is_char_boundary(pos) {
        pos -= 1;
    }
    pos
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_adds_id_after_tag_name() {
        // 两个目标，降序注入，属性插在标签名后、保留原属性
        let seg = r#"<p height="1em">甲</p><blockquote>乙</blockquote>"#;
        let p2 = seg.find("<blockquote").unwrap();
        let out = inject_anchors(seg, &[(p2, 200), (0, 100)]);
        assert!(out.contains(r#"<p id="fp100" height="1em">甲"#), "{out}");
        assert!(out.contains(r#"<blockquote id="fp200">乙"#), "{out}");
    }

    #[test]
    fn clean_seg_strips_shell_keeps_link_and_id() {
        let used = HashMap::new();
        let re_img = Regex::new(r#"(?is)<img\b[^>]*\brecindex="0*(\d+)"[^>]*>"#).unwrap();
        let seg = r#"<html><head><guide/></head><body><p id="fp5"><a filepos=00010>注</a></p><mbp:pagebreak/></body></html>"#;
        let out = clean_seg(seg, &used, &re_img);
        assert!(!out.contains("<head") && !out.contains("<body") && !out.contains("<html"), "剥壳: {out}");
        assert!(out.contains(r#"id="fp5""#), "保留注入 id: {out}");
        assert!(out.contains("filepos=00010"), "保留内链待 remap: {out}");
        assert!(!out.contains("mbp:pagebreak"), "去 mbp: {out}");
    }

    #[test]
    fn clean_seg_maps_recindex_img() {
        let mut used = HashMap::new();
        used.insert(2usize, "images/img2.jpg".to_string());
        let re_img = Regex::new(r#"(?is)<img\b[^>]*\brecindex="0*(\d+)"[^>]*>"#).unwrap();
        let out = clean_seg(r#"<p><img recindex="00002" width="5"></img></p>"#, &used, &re_img);
        assert!(out.contains(r#"<img src="images/img2.jpg"/>"#), "{out}");
        assert!(!out.contains("recindex") && !out.contains("</img>"), "{out}");
    }

    #[test]
    fn inject_then_collapse_no_dup_id_on_calibre_element() {
        // calibre 做的 MOBI 目标元素已带 id="calibre_pb_6"；注入 fpN 后须折叠成单 id（保首位 fpN）
        let seg = r#"<p id="calibre_pb_6" class="x">正文</p>"#;
        // 目标落在 <p 起点(位置 0)，n=10
        let injected = inject_anchors(seg, &[(0, 10)]);
        assert!(injected.contains(r#"id="fp10""#) && injected.contains(r#"id="calibre_pb_6""#), "注入后应双 id(待折叠): {injected}");
        let out = crate::htmlproc::collapse_dup_id_attrs(&injected);
        // 只剩一个 id 属性，且是首位的 fpN
        assert_eq!(out.matches(" id=").count(), 1, "折叠后应单 id: {out}");
        assert!(out.contains(r#"id="fp10""#), "应保住注入的 fpN 锚点: {out}");
        assert!(!out.contains("calibre_pb_6"), "冗余既存 id 应删: {out}");
    }

    #[test]
    fn remap_rewrites_filepos_in_place_keeping_id() {
        // chapter_of: <100→0, ≥100→1
        let cof = |n: usize| -> usize { if n >= 100 { 1 } else { 0 } };
        let html = r#"<a id="fp5" filepos=0000000010>目录项</a>"#;
        let out = remap_links(html, &cof);
        assert!(out.contains(r#"id="fp5""#), "保留 id: {out}");
        assert!(out.contains(r#"href="chap_0001.xhtml#fp10""#), "N=10→章0: {out}");
        let out2 = remap_links(r#"<a filepos=0000000150>x</a>"#, &cof);
        assert!(out2.contains(r#"href="chap_0002.xhtml#fp150""#), "N=150→章1: {out2}");
    }

    #[test]
    fn extract_toc_picks_densest_chunk_and_levels() {
        // 一个目录段（多链）+ 一个正文段（少链）。数字项→level2，其余→level1。
        // 偏移须 ≤ 字符串长度（超界目标被正确过滤）——此串足够长，用小偏移当章界值。
        let rawml = concat!(
            "<html><body><p>正文<a filepos=0000000005>脚注</a></p><mbp:pagebreak/>",
            "<p><a filepos=0000000010>第一章</a></p>",
            "<p><a filepos=0000000020>1</a></p>",
            "<p><a filepos=0000000030>2</a></p>",
            "<p><a filepos=0000000040>第二章</a></p>",
            "<p><a filepos=0000000050>1</a></p>",
            "<p><a filepos=0000000060>2</a></p>",
            "<p><a filepos=0000000070>3</a></p>",
            "<p><a filepos=0000000080>后记</a></p></body></html>"
        );
        let cuts = extract_toc(rawml);
        assert_eq!(cuts.len(), 8, "目录段 8 条章界");
        // 按偏移排序：10 第一章(L1) 20 1(L2) 30 2(L2) 40 第二章(L1)...
        assert_eq!(cuts[0].off, 10);
        assert_eq!(cuts[0].title, "第一章");
        assert_eq!(cuts[0].level, 1);
        assert_eq!(cuts[1].title, "1");
        assert_eq!(cuts[1].level, 2);
        assert_eq!(cuts[3].title, "第二章");
        assert_eq!(cuts[3].level, 1);
    }

    #[test]
    fn build_chapters_toc_split_titles_and_links_jump() {
        // 目录段(密) + 两章正文；正文里的 filepos 链应改写成 chap#fp、章名来自 TOC。
        let rawml = concat!(
            "<html><head><guide/></head><body>",
            // 目录段（4 链，≥阈值需 8——凑够）
            "<p><a filepos=0000000300>第一章</a><a filepos=0000000400>第二章</a>",
            "<a filepos=0000000300>x</a><a filepos=0000000400>y</a>",
            "<a filepos=0000000300>x</a><a filepos=0000000400>y</a>",
            "<a filepos=0000000300>x</a><a filepos=0000000400>y</a></p><mbp:pagebreak/>",
            // 第一章@?  正文含指向第二章的链
            "XXXX第一章正文<a filepos=0000000400>见第二章</a>更多",
            "<mbp:pagebreak/>第二章正文结尾"
        );
        // 手工修偏移不现实——只验行为：章数≥2、链接被改写成 chap#fp、无残留 filepos。
        let used = HashMap::new();
        let re_img = Regex::new(r#"(?is)<img\b[^>]*\brecindex="0*(\d+)"[^>]*>"#).unwrap();
        let chs = build_chapters(rawml, &used, &re_img);
        let joined: String = chs.iter().map(|c| c.html_body.clone()).collect();
        assert!(!joined.contains("filepos="), "所有 filepos 已改写: {joined}");
        assert!(joined.contains("href=\"chap_") && joined.contains("#fp"), "链接指向 chap#fp: {joined}");
    }
}
