//! 有文字层 PDF → EPUB（正文、标题、图片、公式区域截图）。
use super::*;

/// 把 lopdf 的 `PdfImage`（第三方 PDF 里的原始图片流）解成可以喂给 `imgopt` 裁边缩放/
/// `pdfwrite::image_from_bytes` 的通用 JPEG/PNG 字节。**这次只稳妥处理两种最常见的情况**：
/// `/DCTDecode`（JPEG，原样透传，不解码不重编码）和 `/FlateDecode` 的 8-bit 灰度/RGB 原始像素
/// （解压后重新编码成 PNG）。CCITTFax/JBIG2/JPX/索引色/非 8-bit 这些少见情况直接报错跳过这页
/// （不是这次范围内要支持的全部 PDF 图片编码，`hayro-ccitt`/`hayro-jbig2` 这两个传递依赖理论上
/// 能补上，留作已知的后续扩展点，不在这次实现）。
/// **不能自己手撸 zlib inflate**——真机踩过：pdflatex/pdftex 产出的 `/FlateDecode` 图片流
/// 常见带 `/DecodeParms << /Predictor 10 ... >>`（PNG 逐行预测器），裸 inflate 出来的字节
/// 每行多一个过滤类型前缀字节（真实样本：200×100×3=60000 应有字节，裸 inflate 出 60100，
/// 多出来的 100 字节精确等于行数）——`lopdf::Stream::decompressed_content()` 已经正确处理
/// 了这个预测器（含 PNG 10-15 与 TIFF 2 两种），必须重新按 `img.id` 取回原始 `Stream` 对象
/// 调它，不能图省事直接对 `img.content`（未解预测器的裸字节）手动 inflate。
pub(super) fn decode_pdf_image_to_bytes(doc: &lopdf::Document, img: &lopdf::xobject::PdfImage) -> Result<Vec<u8>, String> {
    let filters = img.filters.clone().unwrap_or_default();
    if filters.iter().any(|f| f == "DCTDecode") {
        return Ok(img.content.to_vec());
    }
    if filters.is_empty() || filters.iter().any(|f| f == "FlateDecode") {
        let obj = doc.get_object(img.id).map_err(|e| format!("重取图片对象失败: {e}"))?;
        let stream = obj.as_stream().map_err(|e| format!("图片对象不是 stream: {e}"))?;
        let raw = if filters.is_empty() { img.content.to_vec() } else { stream.decompressed_content().map_err(|e| format!("PDF 图片解压失败: {e}"))? };
        let is_gray = img.color_space.as_deref() == Some("DeviceGray");
        return encode_raw_pixels_png(&raw, img.width as u32, img.height as u32, is_gray);
    }
    Err(format!("暂不支持的 PDF 图片编码: {filters:?}"))
}

/// 按字节内容嗅探出该用 `.jpg` 还是 `.png`：`decode_pdf_image_to_bytes` 的 `DCTDecode` 分支原样
/// 透传 JPEG 字节（SOI 魔数 `FF D8`），`FlateDecode` 分支永远重编码成 PNG——文件名后缀必须跟这个
/// 判断结果一致，此前后缀写死 `.png`、只有 `media_type` 按内容嗅探，两者对不上时 OPF 里 media-type
/// 虽然标对了，但真机渲染器很可能按文件后缀走快速路径去解码/探测尺寸，一个 `.png` 后缀的真 JPEG
/// 字节流会解码失败或读不出尺寸，导致图片整个不显示（2026-09-23 真机投一本真实 PDF 手册发现"手册里
/// 无图形"，排查到这里——之前只测过 pdflatex 合成样本，那份图片凑巧走 FlateDecode/PNG，没暴露过
/// DCTDecode/JPEG 这条分支）。
pub(super) fn image_ext_and_media_type(raw: &[u8]) -> (&'static str, &'static str) {
    if raw.len() >= 2 && raw[0] == 0xFF && raw[1] == 0xD8 {
        ("jpg", "image/jpeg")
    } else {
        ("png", "image/png")
    }
}

pub(super) fn encode_raw_pixels_png(pixels: &[u8], w: u32, h: u32, gray: bool) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(if gray { png::ColorType::Grayscale } else { png::ColorType::Rgb });
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().map_err(|e| format!("PNG 编码头失败: {e}"))?;
        writer.write_image_data(pixels).map_err(|e| format!("PNG 编码数据失败: {e}"))?;
    }
    Ok(out)
}

// ============================================================================
// PDF → EPUB（有文字层）
// ============================================================================

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdfToEpubReport {
    pub pages: usize,
    pub chapters: usize,
    pub images: usize,
    pub formula_blocks: usize,
}

/// 页面 `/Resources/XObject` 资源名（`Do` 算子里写的名字，如 `Im1`）→ 对象 id 的映射——图片事件
/// （`ImageEvent::xobject_name`）只带资源名，不带对象 id，要靠这个反查到 `doc.get_page_images`
/// 返回的 `PdfImage.id`（逐字提取与这里用的是同一份 `lopdf::Document`，对象 id 天然一致）。不处理继承自父页面节点的 `/Resources`
/// （少见；页面自身没有 XObject 资源字典时这张图直接查不到，跳过不崩——图片渲染缺一张比整本
/// 转换失败更可接受，判不准选保守分支）。
pub(super) fn page_image_ids(doc: &lopdf::Document, page_id: lopdf::ObjectId) -> std::collections::HashMap<Vec<u8>, lopdf::ObjectId> {
    let mut out = std::collections::HashMap::new();
    let Ok(page) = doc.get_dictionary(page_id) else { return out };
    let Ok(resources) = doc.get_dict_in_dict(page, b"Resources") else { return out };
    let Ok(xobject) = doc.get_dict_in_dict(resources, b"XObject") else { return out };
    for (name, value) in xobject.iter() {
        if let Ok(id) = value.as_reference() {
            out.insert(name.clone(), id);
        }
    }
    out
}

/// PDF 链接注释的目标：外链 URI，或书内某一页（0 起页序）。
#[derive(Clone, Debug, PartialEq)]
pub(super) enum LinkTarget {
    Uri(String),
    Page(usize),
}

/// 一页上的一个 `Link` 注释：矩形（PDF 用户空间，已规整成 x0<x1、y0<y1）+ 目标。
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PageLink {
    pub rect: [f64; 4],
    pub target: LinkTarget,
}

fn deref<'a>(doc: &'a lopdf::Document, obj: &'a lopdf::Object) -> &'a lopdf::Object {
    match obj {
        lopdf::Object::Reference(id) => doc.get_object(*id).unwrap_or(obj),
        _ => obj,
    }
}

fn num(o: &lopdf::Object) -> Option<f64> {
    match o {
        lopdf::Object::Integer(i) => Some(*i as f64),
        lopdf::Object::Real(r) => Some(*r as f64),
        _ => None,
    }
}

/// 具名目标的 `/Names` 名字树查找（`Kids` 递归 + `Names` 键值对数组），深度封顶防环。
fn name_tree_lookup<'a>(doc: &'a lopdf::Document, node: &'a lopdf::Dictionary, key: &[u8], depth: usize) -> Option<&'a lopdf::Object> {
    if depth > 32 {
        return None;
    }
    if let Ok(names) = node.get(b"Names").map(|o| deref(doc, o)).and_then(|o| o.as_array()) {
        for pair in names.chunks(2) {
            if let [k, v] = pair {
                if let Ok(kb) = deref(doc, k).as_str() {
                    if kb == key {
                        return Some(v);
                    }
                }
            }
        }
    }
    if let Ok(kids) = node.get(b"Kids").map(|o| deref(doc, o)).and_then(|o| o.as_array()) {
        for kid in kids {
            if let Ok(d) = deref(doc, kid).as_dict() {
                if let Some(v) = name_tree_lookup(doc, d, key, depth + 1) {
                    return Some(v);
                }
            }
        }
    }
    None
}

/// 把一个目标（`/Dest` 或 GoTo 的 `/D`）解析成 0 起页序。支持显式数组 `[页对象 /XYZ …]`、
/// 具名目标（catalog `/Dests` 字典与 `/Names`→`/Dests` 名字树两种存法）、`<< /D … >>` 字典包装。
/// 解析不了就 `None`——这条链接就不做成书内跳转，文字照常保留（lopdf 自带的具名目标解析在畸形
/// 输入上会 `unwrap()` panic，不能用）。
fn resolve_dest(doc: &lopdf::Document, obj: &lopdf::Object, page_index_of: &std::collections::HashMap<lopdf::ObjectId, usize>, depth: usize) -> Option<usize> {
    if depth > 8 {
        return None;
    }
    match deref(doc, obj) {
        lopdf::Object::Array(a) => match a.first()? {
            lopdf::Object::Reference(id) => page_index_of.get(id).copied(),
            _ => None,
        },
        lopdf::Object::Dictionary(d) => resolve_dest(doc, d.get(b"D").ok()?, page_index_of, depth + 1),
        o @ (lopdf::Object::Name(_) | lopdf::Object::String(..)) => {
            let key: &[u8] = match o {
                lopdf::Object::Name(n) => n,
                lopdf::Object::String(s, _) => s,
                _ => unreachable!(),
            };
            let catalog = doc.catalog().ok()?;
            if let Ok(dests) = catalog.get(b"Dests").map(|o| deref(doc, o)).and_then(|o| o.as_dict()) {
                if let Ok(v) = dests.get(key) {
                    return resolve_dest(doc, v, page_index_of, depth + 1);
                }
            }
            let names = catalog.get(b"Names").map(|o| deref(doc, o)).and_then(|o| o.as_dict()).ok()?;
            let tree = names.get(b"Dests").map(|o| deref(doc, o)).and_then(|o| o.as_dict()).ok()?;
            let v = name_tree_lookup(doc, tree, key, 0)?;
            resolve_dest(doc, v, page_index_of, depth + 1)
        }
        _ => None,
    }
}

/// 读一页上所有 `Link` 注释（外链 `/A /URI`、书内 `/A /GoTo /D` 或直接 `/Dest`）。其它注释类型
/// （高亮、批注等）不管；目标解析不出的链接丢弃（文字本身不受影响）。
pub(super) fn page_links(doc: &lopdf::Document, page_id: lopdf::ObjectId, page_index_of: &std::collections::HashMap<lopdf::ObjectId, usize>) -> Vec<PageLink> {
    let mut out = Vec::new();
    for a in doc.get_page_annotations(page_id).unwrap_or_default() {
        if a.get(b"Subtype").ok().and_then(|s| s.as_name().ok()) != Some(b"Link".as_slice()) {
            continue;
        }
        let Some(rect) = a.get(b"Rect").ok().map(|o| deref(doc, o)).and_then(|o| o.as_array().ok()).filter(|r| r.len() == 4).and_then(|r| {
            let v: Vec<f64> = r.iter().filter_map(|x| num(deref(doc, x))).collect();
            (v.len() == 4).then(|| [v[0].min(v[2]), v[1].min(v[3]), v[0].max(v[2]), v[1].max(v[3])])
        }) else {
            continue;
        };
        let action = a.get(b"A").ok().map(|o| deref(doc, o)).and_then(|o| o.as_dict().ok());
        let target = match action {
            Some(act) => match act.get(b"S").ok().and_then(|s| s.as_name().ok()) {
                Some(b"URI") => act.get(b"URI").ok().map(|o| deref(doc, o)).and_then(|o| o.as_str().ok()).map(|s| LinkTarget::Uri(String::from_utf8_lossy(s).trim().to_string())),
                Some(b"GoTo") => act.get(b"D").ok().and_then(|d| resolve_dest(doc, d, page_index_of, 0)).map(LinkTarget::Page),
                _ => None,
            },
            None => a.get(b"Dest").ok().and_then(|d| resolve_dest(doc, d, page_index_of, 0)).map(LinkTarget::Page),
        };
        match target {
            Some(LinkTarget::Uri(u)) if u.is_empty() => {}
            Some(t) => out.push(PageLink { rect, target: t }),
            None => {}
        }
    }
    out
}

/// 字符是否落在链接矩形里：取字符基线往上约三成字号处当代表点（基线本身常贴着矩形下沿，
/// 直接用会因为浮点误差漏判），外扩 1pt 容差。
pub(super) fn char_in_rect(c: &PositionedChar, r: &[f64; 4]) -> bool {
    let (px, py) = (c.x + c.font_size.max(1.0) * 0.1, c.y + c.font_size.max(1.0) * 0.3);
    px >= r[0] - 1.0 && px <= r[2] + 1.0 && py >= r[1] - 1.0 && py <= r[3] + 1.0
}

/// 书内跳转目标页的锚点 id。
pub(super) fn page_anchor_id(page: usize) -> String {
    format!("pdf-p{}", page + 1)
}

/// 章节切好后把占位的 `href="#pdf-pN"` 改写成真实位置：目标页在本章→保留同文件裸锚点（xochitl
/// 确定会跳的形态）；在别章→`chap_xxxx.xhtml#pdf-pN`（标准 EPUB 跨文件链接，KOReader 能跳；
/// xochitl 能不能跳跨文件链接待真机验证）。`page_chapter[p]`＝第 p 页（0 起）所在的第一个章节。
pub(super) fn relink_page_anchors(chapters: &mut [Chapter], page_chapter: &[usize]) {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| regex::Regex::new(r##"href="#pdf-p(\d+)""##).unwrap());
    for (ci, ch) in chapters.iter_mut().enumerate() {
        if !ch.html_body.contains("href=\"#pdf-p") {
            continue;
        }
        ch.html_body = re
            .replace_all(&ch.html_body, |c: &regex::Captures| {
                let n: usize = c[1].parse().unwrap_or(1);
                match page_chapter.get(n.saturating_sub(1)) {
                    Some(&tc) if tc != ci => format!("href=\"{}#pdf-p{n}\"", crate::epub::chapter_filename(tc)),
                    _ => c[0].to_string(),
                }
            })
            .into_owned();
    }
}

/// 一批落在同一插入点的图片：上沿相差不到 8pt 的相邻图片算同一视觉行，放进同一个段落并排
/// （原书并排的两个二维码此前被上下叠放，2026-09-23 真机《移动互联软件安装使用手册》）。
pub(super) fn image_rows(batch: &[(f64, String)]) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < batch.len() {
        let mut j = i + 1;
        while j < batch.len() && (batch[j].0 - batch[i].0).abs() < 8.0 {
            j += 1;
        }
        out.push_str("<p>");
        out.push_str(&batch[i..j].iter().map(|b| b.1.as_str()).collect::<Vec<_>>().join(" "));
        out.push_str("</p>");
        i = j;
    }
    out
}

/// 本页的典型行距（相邻两行基线差的中位数，只统计 0.5–3 倍字号之间的，排除同行重定位与段间
/// 大空隙）与正文右边界（字符右边缘 98% 分位）。样本太少返回 `None`，调用方退回按字号的老判据。
pub(super) fn page_line_stats(chars: &[PositionedChar]) -> (Option<f64>, Option<f64>) {
    let vis: Vec<&PositionedChar> = chars.iter().filter(|c| c.ch != '\u{0}' && !c.ch.is_whitespace()).collect();
    let mut dys: Vec<f64> = vis
        .windows(2)
        .filter_map(|w| {
            let dy = (w[0].y - w[1].y).abs();
            let fs = w[1].font_size.max(1.0);
            (dy > fs * 0.5 && dy < fs * 3.0).then_some(dy)
        })
        .collect();
    let pitch = (dys.len() >= 3).then(|| {
        dys.sort_by(f64::total_cmp);
        dys[dys.len() / 2]
    });
    let mut rights: Vec<f64> = vis.iter().map(|c| c.x + c.font_size * if is_cjk(c.ch) { 1.0 } else { 0.5 }).collect();
    let right = (rights.len() >= 20).then(|| {
        rights.sort_by(f64::total_cmp);
        rights[((rights.len() - 1) as f64 * 0.98) as usize]
    });
    (pitch, right)
}

/// 把连续几页的 HTML 接起来；上一页最后一段没以句末标点结尾、下一页以正文段落开头时，两段接成
/// 一段——PDF 的页边界不是段落边界，一句话跨页此前会被切成两段（2026-09-23 真机《移动互联软件安装
/// 使用手册》"点击获取验 / 证码"）。下一页首段带书内跳转锚点 id 的不接（接了 id 会丢）。
pub(super) fn join_pages(pages: &[String]) -> String {
    static ANCHOR: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    // 只接下一页以普通 `<p>` 开头的；带锚点 id 的段落（书内跳转目标）不接，否则 id 会丢。
    let anchor_re = ANCHOR.get_or_init(|| regex::Regex::new(r#"^()?<p>"#).unwrap());
    let mut out = String::new();
    for page in pages {
        let open_ends = out.ends_with("</p>") && {
            let before = &out[..out.len() - 4];
            let last = crate::util::strip_tags_tail_char(before);
            last.map(|c| !matches!(c, '。' | '！' | '？' | '!' | '?' | '.' | '…' | '：' | ':' | '；' | ';' | '”' | '"' | '」' | '』')).unwrap_or(false)
        };
        match anchor_re.captures(page) {
            Some(m) if open_ends && !page[m.get(0).unwrap().end()..].starts_with("<img") => {
                out.truncate(out.len() - 4);
                if let Some(a) = m.get(1) {
                    out.push_str(a.as_str());
                }
                out.push_str(&page[m.get(0).unwrap().end()..]);
            }
            _ => out.push_str(page),
        }
    }
    out
}

/// 目录条目的起始页（0 起，按条目顺序）→ 每章的页范围：**每页只进一章、一页不丢**。
/// - 书签往回指（起始页小于前面条目已到达的页）的条目当分组标题，起点顺延到下一个正常条目；
/// - 多个条目起点落在同一页：内容归最后一个（最具体的那一条，通常是文章），前面的得到空范围——
///   不往空章里补原书没有的文字，跟 `promote_heading` 同一原则；
/// - 第一个条目之前的页（封面、目录页）并入第一章。
///
/// 此前按"本条起始页到下一条起始页"取范围、再 `max(start + 1)` 兜底，遇到往回指的书签会把一大段
/// 页重复塞进多个章节：2026-09-23 真机《2026-09-19 T.E.双语》（calibre 导出的经济学人，一级栏目
/// 书签全指向第 2 页目录页、二级文章书签才指向正文）540 页产出约 604 万字符、1208 处图片引用
/// （原书文字层约 50 万字符、129 张图），渲染检查预估 4978 页。
pub(super) fn partition_chapters(starts: &[usize], page_count: usize) -> Vec<std::ops::Range<usize>> {
    let n = starts.len();
    if n == 0 || page_count == 0 {
        return Vec::new();
    }
    let mut eff: Vec<Option<usize>> = vec![None; n];
    let mut cur = 0usize;
    for (i, &t) in starts.iter().enumerate() {
        let t = t.min(page_count - 1);
        if t >= cur {
            eff[i] = Some(t);
            cur = t;
        }
    }
    let mut next = page_count;
    let mut eff: Vec<usize> = eff
        .into_iter()
        .rev()
        .map(|e| match e {
            Some(v) => {
                next = v;
                v
            }
            None => next,
        })
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    eff[0] = 0;
    (0..n).map(|i| eff[i]..eff.get(i + 1).copied().unwrap_or(page_count)).collect()
}

/// 图片的视觉锚点：图片插在"它上沿以上最近的那一行文字"之后。返回值是"排在它前面的最后一个
/// 字符的 seq + 1"（本页没有任何文字在它上方＝0，排在页首）。为什么不用绘制顺序 `seq`：Word 导出
/// 的 PDF 每页先画完所有文字、最后才画图片（2026-09-23 真机《移动互联软件安装使用手册》坐实：第 1
/// 页二维码 seq 排在全部文字之后，但视觉上夹在"扫描如下二维码"与红字"注意"之间），按绘制顺序
/// 排会把图片全堆到页尾、甚至插进一句话中间。calibre 导出的 PDF（《T.E.双语》）绘制顺序本来就跟
/// 视觉一致，视觉锚点对它结果不变。
pub(super) fn image_visual_pos(ctm: &[f64; 6], chars: &[PositionedChar]) -> (usize, f64, f64, f64) {
    let [a, b, c, d, e, f] = *ctm;
    let ys = [f, b + f, d + f, b + d + f];
    let xs = [e, a + e, c + e, a + c + e];
    let top = ys.iter().cloned().fold(f64::MIN, f64::max);
    let left = xs.iter().cloned().fold(f64::MAX, f64::min);
    let width = xs.iter().cloned().fold(f64::MIN, f64::max) - left;
    let above = chars.iter().filter(|ch| ch.ch != '\u{0}' && !ch.ch.is_whitespace() && ch.y >= top - 0.5);
    let nearest = above.clone().map(|ch| ch.y).fold(f64::MAX, f64::min);
    let pos = if nearest == f64::MAX {
        0
    } else {
        above.filter(|ch| (ch.y - nearest).abs() <= 1.0).map(|ch| ch.seq).max().map(|s| s + 1).unwrap_or(0)
    };
    (pos, top, left, width)
}

/// 全书正文栏宽（pt）：非空白字符左边缘的 2% 分位到右边缘的 98% 分位（去掉页眉页脚、页码这类零星
/// 离群位置）。字符不带自身宽度，右边缘按中日韩 1 个字号、其它半个字号估。文字太少/栏宽不合理返回
/// `None`（调用方就不缩放，维持原始像素尺寸）。
pub(super) fn text_column_width(pages: &[PageContent]) -> Option<f64> {
    let mut lefts: Vec<f64> = Vec::new();
    let mut rights: Vec<f64> = Vec::new();
    for c in pages.iter().flat_map(|p| p.chars.iter()).filter(|c| c.ch != '\u{0}' && !c.ch.is_whitespace()) {
        lefts.push(c.x);
        rights.push(c.x + c.font_size * if is_cjk(c.ch) { 1.0 } else { 0.5 });
    }
    if lefts.len() < 50 {
        return None;
    }
    lefts.sort_by(f64::total_cmp);
    rights.sort_by(f64::total_cmp);
    let pick = |v: &[f64], q: f64| v[((v.len() - 1) as f64 * q) as usize];
    let w = pick(&rights, 0.98) - pick(&lefts, 0.02);
    (w > 50.0).then_some(w)
}

/// 图片在原 PDF 里占正文栏宽的比例 → 5% 一档的宽度 class（`cj-wN`，N∈5..=100）。xochitl 不认内联
/// `style`，只能外链 class；取档是为了全书 class 数量有限。比栏宽还宽的（跨栏大图）封顶 100%。
pub(super) fn width_class(img_width_pt: f64, column: Option<f64>) -> Option<u32> {
    let col = column?;
    if img_width_pt.is_nan() || img_width_pt <= 0.0 {
        return None;
    }
    let pct = (img_width_pt / col * 100.0 / 5.0).round() as u32 * 5;
    Some(pct.clamp(5, 100))
}

/// 有文字层的 PDF → EPUB：结构解析（页树/书签）→ 逐页文字+颜色+图片位置提取（`pdf-extract-cj`
/// fork，`seq` 是三者共用的文档真实顺序坐标）→ 公式区域探测→ 按 `seq` 交织重排成段落（文字/
/// 图片顺序跟原书一致，颜色跟随原书，不是"图片统一放段末"的近似）→ 公式块用 hayro 整页渲染后
/// 裁剪成图片→ 章节/TOC 构建（书签优先，没有书签按字号识别标题，标题也识别不出来就按固定页数
/// 分块）→ `epub::assemble_pdf_derived`（颜色与图片宽度的 CSS 规则通过返回值第三项交给调用方拼进外链样式表——
/// xochitl 不认内联 `style=`）。
pub fn optimize_pdf_to_epub(src: &Path, mut on_progress: impl FnMut(usize, usize)) -> Result<(Book, PdfToEpubReport, String), String> {
    // 原始字节解析完即释放（见 `load_pdf`）；只有带公式的书才需要把字节交给 hayro 再解析，那时再从磁盘读一次
    // （host 实测 139MB 扫描 PDF 转换峰值 557→431MB，连同组装时逐张释放资源）。
    let doc = load_pdf(src)?;
    let pages_map = doc.get_pages();
    let page_count = pages_map.len();
    if page_count == 0 {
        return Err("PDF 没有可用页面".into());
    }
    let page_ids: Vec<lopdf::ObjectId> = pages_map.values().copied().collect();

    on_progress(0, page_count.max(1) * 2);
    let text_pages = extract_positioned_text_doc(&doc)?;
    if text_pages.len() != page_count {
        return Err(format!("文字提取页数 {} 跟结构解析页数 {} 对不上", text_pages.len(), page_count));
    }

    // 公式区域（每页一份，可能为空）。
    let formula_regions: Vec<Vec<BBox>> = text_pages.iter().map(|p| detect_formula_regions(&p.chars)).collect();
    let total_formula_blocks: usize = formula_regions.iter().map(|v| v.len()).sum();

    let toc = doc.get_toc().ok();
    let mut resources: Vec<Resource> = Vec::new();
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut total_images = 0usize;
    // 全书颜色去重表：同一个 RGB 只生成一条 CSS 规则，不管出现在哪页——`resolve_fill_rgb` 头注释
    // 解释过为什么纯黑（`[0,0,0]`）不算"有颜色"：默认字色已经是黑，包 span 只会让 markup 膨胀。
    let mut used_colors: std::collections::HashMap<[u8; 3], usize> = std::collections::HashMap::new();
    // 图片缩放：按原 PDF 里图片占正文栏宽的比例给 EPUB 里的 `<img>` 定宽（2026-09-23 用户要求"图片可
    // 适当缩放，保障显示效果与原 PDF 基本一致"）——此前只有 `img{max-width:100%}`，图片按自身像素
    // 显示，小图标被放大、低分辨率截图缩成一小块，跟原书版面比例对不上。
    let column = text_column_width(&text_pages);
    // 全书典型行距：页内行数太少（样本不足）时的兜底。
    let doc_pitch = {
        let mut v: Vec<f64> = text_pages.iter().filter_map(|p| page_line_stats(&p.chars).0).collect();
        v.sort_by(f64::total_cmp);
        v.get(v.len() / 2).copied()
    };
    let mut used_widths: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    // 链接：全书先扫一遍（书内跳转要知道哪些页是目标，才能在那些页首放锚点）。
    let page_index_of: std::collections::HashMap<lopdf::ObjectId, usize> = page_ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
    let links_by_page: Vec<Vec<PageLink>> = page_ids.iter().map(|pid| page_links(&doc, *pid, &page_index_of)).collect();
    let targeted_pages: std::collections::HashSet<usize> = links_by_page
        .iter()
        .flatten()
        .filter_map(|l| match l.target {
            LinkTarget::Page(p) => Some(p),
            _ => None,
        })
        .collect();

    // 章节结构先定下来（书签→按字号识别的标题→固定页数兜底），逐页生成 HTML 之前就要知道：
    // 是不是单文件、哪些页首要放锚点。(标题, 层级, 起始页) 经 `partition_chapters` 切成互不重叠、
    // 覆盖全书的页范围。
    let mut entries: Vec<(String, i64, usize)> = match toc {
        Some(toc) if !toc.toc.is_empty() => toc.toc.into_iter().map(|e| (e.title, e.level as i64, e.page.saturating_sub(1))).collect(),
        _ => detect_headings_by_font_size(&text_pages).into_iter().map(|h| (h.title, h.level, h.page)).collect(),
    };
    // 兜底：按固定页数分块，如实标注不是真实章节结构——这种情况正文前不补 `<h2>` 标题（"第 N 部分"
    // 不是从内容里识别出来的，硬加会显得像是真的检测到了结构）。
    let is_fallback = entries.is_empty();
    if is_fallback {
        entries = (0..page_count).step_by(FALLBACK_CHUNK_PAGES).enumerate().map(|(i, start)| (format!("第 {} 部分", i + 1), 1, start)).collect();
    }
    let ranges = partition_chapters(&entries.iter().map(|e| e.2).collect::<Vec<_>>(), page_count);
    // 多文件布局下每页所在的文件（只有非空范围才成文件；空范围是分组标题，只进目录）。
    let mut page_file = vec![0usize; page_count];
    for (fi, r) in ranges.iter().filter(|r| !r.is_empty()).enumerate() {
        for p in r.clone() {
            page_file[p] = fi;
        }
    }
    // 有跨文件的书内跳转就合成单文件：xochitl 只把同一文件内的 `#锚点` 当书内跳转，跨文件链接
    // （不管带不带锚点）一律当外部网址、点了不跳（2026-09-23 真机实验：投一本 3 章测试书量预览
    // PDF 的链接注释坐实）；而它的目录完全支持文件内锚点、能精确定位到页（同一实验的单文件版，
    // `.epubindex` 里 `chap_0001.xhtml#c3` 映射到第 7 页）。没有跨文件跳转的书维持多文件，不动。
    let single_file = links_by_page.iter().enumerate().any(|(p, ls)| ls.iter().any(|l| matches!(l.target, LinkTarget::Page(t) if page_file[t] != page_file[p])));
    let mut anchor_pages = targeted_pages;
    if single_file {
        anchor_pages.extend(ranges.iter().map(|r| r.start).filter(|&s| s < page_count));
    }

    // 公式渲染缓存：同页多个公式块只渲染一次整页。
    let render_settings = hayro::RenderSettings { x_scale: 2.0, y_scale: 2.0, ..Default::default() };

    // 只有真有公式块才需要 hayro 再解析一遍（原始字节上面已释放，这里重读；读失败就当渲染不了公式——公式图片只是
    // 文字之外的补充，缺了不丢内容）。
    let hayro_pdf = if total_formula_blocks > 0 { std::fs::read(src).ok().and_then(|b| hayro::hayro_syntax::Pdf::new(std::sync::Arc::new(b)).ok()) } else { None };

    let mut page_html: Vec<String> = Vec::with_capacity(page_count);
    for (idx, page) in text_pages.iter().enumerate() {
        on_progress(page_count + idx, page_count * 2);
        let chars = &page.chars;
        let regions = &formula_regions[idx];
        // 公式块先渲染成图片（同页多块共用一次整页渲染）。图片只是**补充**：文字流里不删任何字符。
        let mut region_imgs: Vec<Option<String>> = vec![None; regions.len()];
        if !regions.is_empty() {
            if let Some(pdf) = &hayro_pdf {
                if let Some(hpage) = pdf.pages().get(idx) {
                    let cache = hayro::RenderCache::new();
                    let pixmap = hayro::render(hpage, &cache, &hayro::hayro_interpret::InterpreterSettings::default(), &render_settings);
                    for (bi, region) in regions.iter().enumerate() {
                        if let Some(png) = crop_pixmap_to_png(&pixmap, region, hpage) {
                            let path = format!("images/pdf_p{}_f{}.png", idx + 1, bi + 1);
                            // 没有 `../`：本模块的章节文件跟 images/ 是同级（都直接落 OEBPS/ 下，不像
                            // 部分导入格式章节在 OEBPS/Text/ 子目录），`../images/...` 会往上多跳一层
                            // 指到 zip 里不存在的路径——真机投一本真实 PDF 手册坐实：图片在 EPUB 里
                            // 打包正确、`<img>` 标签也在，但 xochitl 内部渲染出的预览 PDF 一张图都没有，
                            // 因为 src 从来没解析到过真实文件（2026-09-23，见 fb2.rs 同级布局的对照测试
                            // `src="images/pic1.png"` 不带 `../`）。
                            let cls = match width_class(region.x1 - region.x0, column) {
                                Some(w) => {
                                    used_widths.insert(w);
                                    format!(" class=\"cj-w{w}\"")
                                }
                                None => String::new(),
                            };
                            region_imgs[bi] = Some(format!("<p><img{cls} src=\"{path}\" alt=\"formula\"/></p>"));
                            resources.push(Resource { path, media_type: "image/png".to_string(), bytes: png });
                        }
                    }
                }
            }
        }
        // 本页真实内容图片（跟文字混排的插图/照片/二维码）：先拿到名字→对象 id、对象 id→PdfImage
        // 两张表，图片事件靠这两张表反查解码。位置按**视觉锚点**（`image_visual_pos`：插在图片上沿
        // 以上最近一行文字之后）排，不按绘制顺序 seq——见该函数头注释的 Word PDF 真机样本。同一锚点
        // 的多张图按上沿从高到低、再从左到右（网格排布的截图按行读）。
        let id_by_name = page_image_ids(&doc, page_ids[idx]);
        let page_images = doc.get_page_images(page_ids[idx]).unwrap_or_default();
        let images_by_id: std::collections::HashMap<lopdf::ObjectId, &lopdf::xobject::PdfImage> = page_images.iter().map(|im| (im.id, im)).collect();
        let mut placed: Vec<(usize, f64, f64, &ImageEvent, f64)> = page
            .images
            .iter()
            .map(|im| {
                let (pos, top, left, width) = image_visual_pos(&im.ctm, chars);
                (pos, top, left, im, width)
            })
            .collect();
        placed.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.total_cmp(&a.1)).then(a.2.total_cmp(&b.2)));
        let mut img_iter = placed.into_iter().peekable();
        let mut img_counter = 0usize;
        // 本页链接：字符落在哪个链接矩形里（按链接下标），书内跳转先写成占位 `#pdf-pN`，章节切好后
        // 由 `relink_page_anchors` 改成同章裸锚点或跨章 `chap_xxxx.xhtml#pdf-pN`。
        let links = &links_by_page[idx];
        let link_href = |li: usize| -> String {
            match &links[li].target {
                LinkTarget::Uri(u) => crate::util::xml_escape(u),
                LinkTarget::Page(p) => format!("#{}", page_anchor_id(*p)),
            }
        };

        // 换行不等于换段——PDF 里一段话正常会自动折成好几个视觉行，只有行间垂直间距明显
        // 大于普通行高（约 1.5 倍字号，同一段落内的换行通常间距≈1 倍字号）才算真的换段。
        // 只按 `line` 变化就切 `<p>` 会把每一行拆成单独一段，读起来像分行诗不是正常段落
        // （2026-09-19 真机样本核对时发现）。
        //
        // **不允许变动书籍内容**（用户 2026-09-20 明确要求）：文字层里的每个字符都进文字流，包括公式
        // 外接框内的——此前把框内字符整体丢掉，实测会把紧贴公式的正文单词一并吞掉（"the identity iπ
        // e+1=0" 丢了 identity，"A final short section … symbol," 整半句消失）。公式图片在其所属段落
        // 结束处补一张（`pending` 记录本段落里出现过的公式块，段落收尾时统一输出）。
        let mut html = String::new();
        // 书内跳转的目标页：页首放一个锚点（空 `<a id>` 是 calibre 等工具的通行写法；不把 id 挂到本页
        // 第一个 `<p>` 上——`promote_heading` 按 `<p>标题</p>` 原文匹配升级章标题，加了属性就匹配不上）。
        html.push_str("<p>");
        let mut in_para = false;
        let mut last_line: Option<usize> = None;
        let mut last_y: Option<f64> = None;
        let mut last_ch: Option<char> = None;
        let mut last_end_x = 0.0f64;
        let mut last_fs = 0.0f64;
        let (pitch, right_edge) = page_line_stats(chars);
        let pitch = pitch.or(doc_pitch);
        let mut pending: Vec<usize> = Vec::new();
        let mut flushed = vec![false; regions.len()];
        // 行内状态：当前打开的 `<a>`（链接下标）与 `<span class="cj-cN">`（颜色），`<a>` 永远在外层。
        // 颜色不能用内联 `style=`（xochitl 七条实测规则之一：不认内联样式），只能外链 class。段落被
        // 打断（换段/插图片）时两者都先关掉，下一个字符按它自己的链接/颜色重新决定要不要开——每个
        // 字符的信息完整保留在 `PositionedChar` 与链接矩形里，重新判定不会丢东西，也不产生跨 `</p>`
        // 的非法嵌套。
        let mut open_link: Option<usize> = None;
        let mut open_color: Option<[u8; 3]> = None;
        let close_inline = |html: &mut String, open_link: &mut Option<usize>, open_color: &mut Option<[u8; 3]>| {
            if open_color.take().is_some() {
                html.push_str("</span>");
            }
            if open_link.take().is_some() {
                html.push_str("</a>");
            }
        };
        // 结束当前段落：刚开的 `<p>` 还是空的就撤掉（不留 `<p></p>`，也不产出 `<p><p>` 嵌套）。
        let end_para = |html: &mut String| {
            if html.ends_with("<p>") {
                html.truncate(html.len() - 3);
            } else {
                html.push_str("</p>");
            }
        };
        let flush_pending = |html: &mut String, pending: &mut Vec<usize>, flushed: &mut Vec<bool>| {
            for bi in pending.drain(..) {
                if !flushed[bi] {
                    flushed[bi] = true;
                    if let Some(img) = &region_imgs[bi] {
                        html.push_str(img);
                    }
                }
            }
        };
        // 解码并登记一张内容图片，返回 `<img>` 标签；反查失败（页面没有该资源名/解码失败）返回
        // `None`——缺一张图比整本转换失败更可接受。
        let mut emit_image = |im: &ImageEvent, width_pt: f64, resources: &mut Vec<Resource>, total_images: &mut usize, used_widths: &mut std::collections::BTreeSet<u32>| -> Option<String> {
            let id = id_by_name.get(&im.xobject_name)?;
            let pimg = images_by_id.get(id)?;
            let raw = decode_pdf_image_to_bytes(&doc, pimg).ok()?;
            let (ext, media_type) = image_ext_and_media_type(&raw);
            img_counter += 1;
            // 不带 `../`，理由同上面公式块图片那处注释：章节文件跟 images/ 同级在 OEBPS/ 下。
            let path = format!("images/pdf_p{}_img{}.{ext}", idx + 1, img_counter);
            let cls = match width_class(width_pt, column) {
                Some(w) => {
                    used_widths.insert(w);
                    format!(" class=\"cj-w{w}\"")
                }
                None => String::new(),
            };
            let tag = format!("<img{cls} src=\"{path}\" alt=\"\"/>");
            resources.push(Resource { path, media_type: media_type.to_string(), bytes: raw });
            *total_images += 1;
            Some(tag)
        };
        for c in chars {
            // 视觉位置排在这个字符之前的图片先落地：独立成段，打断当前段落。
            let mut batch: Vec<(f64, String)> = Vec::new();
            while img_iter.peek().map(|p| p.0 <= c.seq).unwrap_or(false) {
                let (_, top, _, im, w) = img_iter.next().unwrap();
                if let Some(tag) = emit_image(im, w, &mut resources, &mut total_images, &mut used_widths) {
                    batch.push((top, tag));
                }
            }
            if !batch.is_empty() {
                close_inline(&mut html, &mut open_link, &mut open_color);
                end_para(&mut html);
                html.push_str(&image_rows(&batch));
                html.push_str("<p>");
                in_para = false;
            }
            if let Some(ll) = last_line {
                if c.line != ll {
                    // `line` 变了不等于换了视觉行（逐字定位的排版每个字 `line` 都不同，见
                    // `PositionedChar::line`）——看基线：动得比行距大一截＝换段；动了但在行距内＝
                    // 段内折行（补一个空格，中文接中文除外）；基线没动＝同一行里的重新定位，什么都
                    // 不补（真正的词间空格要么是原书显式的空格字符，要么已由 `TextCollector` 按
                    // 位置缝隙补过）。此前一律补空格，《T.E.双语》标题/目录/日期被逐字拆成
                    // "C a n  t h e""能  否  阻  止"（2026-09-23 真机，全书约 500 处）。
                    let gap = last_y.map(|ly| (ly - c.y).abs()).unwrap_or(0.0);
                    let fs = c.font_size.max(1.0);
                    let wrapped = gap > fs * 0.3;
                    // 换段两条线索：行距明显大于本页典型行距；或者上一行离右边界还远就结束了（短行＝
                    // 段落末行）。只看"行距 > 1.5 倍字号"会把 1.5 倍行距的 Word 文档拆成一行一段（2026-09-23
                    // 真机《移动互联软件安装使用手册》"需要自行删 / 除。"），而相邻两段往往也是标准行距，
                    // 单看行距又会把标题和正文粘成一段——所以两条都要。
                    let big_gap = match pitch {
                        Some(pt) => gap > (pt * 1.3).max(fs * 1.2),
                        None => gap > fs * 1.5,
                    };
                    let short_line = wrapped && right_edge.map(|r| last_end_x < r - fs * 4.0).unwrap_or(false);
                    // 第三条：上下两行字号明显不同（居中的大号标题接正文，标题行尾不一定离右边界远）。
                    let size_change = wrapped && (last_fs - fs).abs() > fs * 0.15;
                    if big_gap || short_line || size_change {
                        close_inline(&mut html, &mut open_link, &mut open_color);
                        end_para(&mut html);
                        flush_pending(&mut html, &mut pending, &mut flushed);
                        html.push_str("<p>");
                        in_para = false;
                    } else if in_para && wrapped {
                        // 折行接续：中文接中文不补；行尾是连字符/斜杠不补——原书的复合词
                        // `artificial-intelligence`、网址 `…/the-world-this-week` 在行尾断开，
                        // 补了空格就变成 "artificial- intelligence"、网址被截断（2026-09-23《T.E.双语》）。
                        let joined = match last_ch {
                            Some(l) => (is_cjk(l) && is_cjk(c.ch)) || matches!(l, '-' | '/' | '\u{2010}'),
                            None => true,
                        };
                        if !joined {
                            html.push(' ');
                        }
                    }
                }
            }
            if c.ch == '\u{0}' {
                continue;
            }
            if let Some(bi) = regions.iter().position(|b| point_in_bbox(c.x, c.y, b)) {
                if !flushed[bi] && !pending.contains(&bi) {
                    pending.push(bi);
                }
            }
            // 链接/颜色只在真的变化时关/开，不是每个字符包一层。颜色：纯黑（含解析不出的 `None`）当
            // 默认色处理，不包 span——见上面 `used_colors` 注释。无彩色的灰字也当黑字（墨水屏提对比，
            // 与 EPUB 优化线的 `boost_text_contrast` 同一判据 `achromatic_dark`；彩色字照样保留）——
            // 2026-09-23 用户拍板两条线统一（规范白皮书第 8 章 T2）。
            // 空白字符不单独触发开合、沿用当前状态：链接矩形常把行尾空白也框进去，否则会冒出
            // 只包着空格的 `<a>`。
            let (want_link, want_color) = if c.ch.is_whitespace() {
                (open_link, open_color)
            } else {
                (links.iter().position(|l| char_in_rect(c, &l.rect)), c.color.filter(|&[r, g, b]| [r, g, b] != [0, 0, 0] && !crate::htmlproc::achromatic_dark(r, g, b)))
            };
            if want_link != open_link {
                close_inline(&mut html, &mut open_link, &mut open_color);
                if let Some(li) = want_link {
                    html.push_str(&format!("<a href=\"{}\">", link_href(li)));
                    open_link = Some(li);
                }
            }
            if want_color != open_color {
                if open_color.take().is_some() {
                    html.push_str("</span>");
                }
                if let Some(rgb) = want_color {
                    let next_idx = used_colors.len();
                    let ci = *used_colors.entry(rgb).or_insert(next_idx);
                    html.push_str(&format!("<span class=\"cj-c{ci}\">"));
                    open_color = Some(rgb);
                }
            }
            crate::util::push_xml_escaped(&mut html, c.ch); // 此前 `xml_escape(&c.ch.to_string())`，每个字符两次临时分配
            in_para = true;
            last_line = Some(c.line);
            last_y = Some(c.y);
            last_ch = Some(c.ch);
            last_end_x = c.x + c.font_size * if is_cjk(c.ch) { 1.0 } else { 0.5 };
            if !c.ch.is_whitespace() {
                last_fs = c.font_size.max(1.0);
            }
        }
        close_inline(&mut html, &mut open_link, &mut open_color);
        end_para(&mut html);
        flush_pending(&mut html, &mut pending, &mut flushed);
        // 兜底：没有任何字符落进去的公式块（理论上不会）也别丢图。
        for (bi, img) in region_imgs.iter().enumerate() {
            if !flushed[bi] {
                if let Some(img) = img {
                    html.push_str(img);
                }
            }
        }
        // 视觉位置在本页最后一行文字之后的图片。
        let tail: Vec<(f64, String)> = img_iter.filter_map(|(_, top, _, im, w)| emit_image(im, w, &mut resources, &mut total_images, &mut used_widths).map(|t| (top, t))).collect();
        html.push_str(&image_rows(&tail));
        // 书内跳转目标页：锚点 id 挂在本页第一个有内容的段落上（文字段或图片段；`end_para` 已保证
        // 不留空 `<p>`）。不能用空的 `<a id></a>`：xochitl 只给有内容的元素登记跳转目标，空元素上的
        // id 不进它的目标表，指向它的链接点了没反应（2026-09-23 真机：《T.E.双语》预览 PDF 具名目标
        // 0 个、对照能跳的《甲午》331 个；单文件测试书里 `<p id>` 登记了、空 `<a id>` 一个没登记）。
        // 目录不受影响（目录走 `.epubindex`，空元素也能定位），所以此前只验证目录时没暴露。
        if anchor_pages.contains(&idx) {
            let id = page_anchor_id(idx);
            if html.contains("<p>") {
                html = html.replacen("<p>", &format!("<p id=\"{id}\">"), 1);
            } else {
                html.insert_str(0, &format!("<p id=\"{id}\">&#160;</p>"));
            }
        }
        page_html.push(html);
    }
    on_progress(page_count * 2, page_count * 2);

    // 章节正文前补一个 `<h2>` 标题元素——书签/字号识别出的标题这时候只是 nav.xhtml 的 TOC
    // 条目文字，正文本身原样含着那行字但没有任何视觉强调（等同于普通段落），读起来不像真实
    // 书籍章节。真实书籍章节页顶部同时有 TOC 条目和正文内可见的标题是标准约定，不是重复。
    let with_heading = |title: &str, body: String| -> String { promote_heading(title, body) };

    let body_of = |title: &str, r: &std::ops::Range<usize>| -> String {
        let seg = join_pages(&page_html[r.clone()]);
        if is_fallback { seg } else { with_heading(title, seg) }
    };
    let mut nav: Vec<NavEntry> = Vec::new();
    if single_file {
        let body: String = entries.iter().zip(&ranges).map(|((t, _, _), r)| body_of(t, r)).collect();
        chapters.push(Chapter { title: String::new(), html_body: body, level: 1 });
        for ((title, level, _), r) in entries.iter().zip(&ranges) {
            let s = r.start.min(page_count - 1);
            nav.push(NavEntry { title: title.clone(), level: *level, href: format!("{}#{}", crate::epub::chapter_filename(0), page_anchor_id(s)) });
        }
    } else {
        for ((title, level, _), r) in entries.iter().zip(&ranges) {
            if !r.is_empty() {
                chapters.push(Chapter { title: title.clone(), html_body: body_of(title, r), level: *level });
            }
        }
        // 分组标题（空范围）也进目录，指向它起点页所在的文件——不再为它单独生成一个空白章节。
        for ((title, level, _), r) in entries.iter().zip(&ranges) {
            let f = page_file[r.start.min(page_count - 1)];
            nav.push(NavEntry { title: title.clone(), level: *level, href: crate::epub::chapter_filename(f) });
        }
        // 书内跳转：占位 `#pdf-pN` → 同章裸锚点 / 跨章 `chap_xxxx.xhtml#pdf-pN`（走到这里说明没有跨章
        // 跳转，改写实际只会命中同章的；保留这一步是防御，免得以后放宽单文件条件时漏改）。
        relink_page_anchors(&mut chapters, &page_file);
    }
    if chapters.is_empty() {
        chapters.push(Chapter { title: "正文".to_string(), html_body: page_html.concat(), level: 1 });
        nav.clear();
    }
    let nav_entries = if nav.is_empty() { chapters.len() } else { nav.len() };

    let title = pdf_doc_title(&doc).unwrap_or_else(|| src.file_stem().and_then(|s| s.to_str()).unwrap_or("PDF").to_string());
    let book = Book {
        meta: BookMeta { book_id: format!("{PDF_BOOK_ID_PREFIX}{title}"), title, author: String::new(), language: "zh".to_string(), publisher: String::new(), cover: None, cover_ext: String::new(), cover_media_type: String::new() },
        chapters,
        resources,
        nav,
    };
    let report = PdfToEpubReport { pages: page_count, chapters: nav_entries, images: total_images, formula_blocks: total_formula_blocks };
    // 颜色 CSS：`used_colors` 插入顺序＝class 编号顺序（`HashMap::entry` 首次插入即分配的 `idx`），
    // 排个序只是让产物字节稳定可测，顺序本身不影响渲染。
    let mut colors_sorted: Vec<([u8; 3], usize)> = used_colors.into_iter().collect();
    colors_sorted.sort_unstable_by_key(|(_, idx)| *idx);
    let mut color_css = String::new();
    for (rgb, idx) in colors_sorted {
        color_css.push_str(&format!(".cj-c{idx}{{color:#{:02x}{:02x}{:02x};}}\n", rgb[0], rgb[1], rgb[2]));
    }
    for w in used_widths {
        color_css.push_str(&format!(".cj-w{w}{{width:{w}%;height:auto;}}\n"));
    }
    Ok((book, report, color_css))
}

/// 把章节正文里**原有**的标题段落升级成 `<h2>`，不额外添加文字（此前 `<h2>标题</h2>` + 正文里原有的
/// 标题段落，同一个标题在章内出现两次，属于改动书籍内容）。匹配 `<p>标题</p>`（整段就是标题）或
/// `<p>标题 …`（标题与后文同段，拆成 `<h2>` + 剩余 `<p>`）；章内找不到就**不加**——宁可标题只留在
/// 目录里也不往正文里塞原书没有的字。
pub(super) fn promote_heading(title: &str, body: String) -> String {
    let t = crate::util::xml_escape(title.trim());
    if t.is_empty() {
        return body;
    }
    // 段落可能带页锚点 id（`<p id="pdf-pN">`，书内跳转目标），升级成 `<h2>` 时 id 跟着走。
    let re = regex::Regex::new(&format!(r#"<p( id="[^"]*")?>{}(</p>| )"#, regex::escape(&t))).unwrap();
    if let Some(c) = re.captures(&body) {
        let id = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let rep = if &c[2] == "</p>" { format!("<h2{id}>{t}</h2>") } else { format!("<h2{id}>{t}</h2><p>") };
        let r = c.get(0).unwrap().range();
        let mut out = body.clone();
        out.replace_range(r, &rep);
        return out;
    }
    body
}

pub(super) fn point_in_bbox(x: f64, y: f64, b: &BBox) -> bool {
    x >= b.x0 && x <= b.x1 && y >= b.y0 && y <= b.y1
}

pub(super) fn pdf_doc_title(doc: &lopdf::Document) -> Option<String> {
    // `/Info` 与 `/Title` 都可能是间接对象（也可能直接内嵌）：此前只认"Info 是引用、Title 是直接字符串"，
    // 其它写法一律取不到书名、退回文件名（2026-09-25 审计）。
    let info = deref(doc, doc.trailer.get(b"Info").ok()?).as_dict().ok()?;
    let bytes = deref(doc, info.get(b"Title").ok()?).as_str().ok()?;
    Some(decode_pdf_text_string(bytes).trim().to_string()).filter(|s| !s.is_empty())
}

/// PDF 文本字符串解码（PDF 32000-1 §7.9.2.2 / PDF 2.0）：`FE FF` 开头＝UTF-16BE，`EF BB BF` 开头＝
/// UTF-8，否则 PDFDocEncoding（ASCII 与 Latin-1 大体重合，0x80–0x9F 这段是 PDF 自己的标点表）。
/// 此前一律按 UTF-8 解，2026-09-23 真机《T.E.双语》（calibre 导出，标题 UTF-16BE）解出一串 `\0`，
/// 写进 OPF 后 xochitl 解析失败、整本只渲染出 1 页。
pub(super) fn decode_pdf_text_string(b: &[u8]) -> String {
    if let Some(rest) = b.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&units);
    }
    if let Some(rest) = b.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    const HI: [char; 32] = [
        '•', '†', '‡', '…', '—', '–', 'ƒ', '⁄', '‹', '›', '−', '‰', '„', '“', '”', '‘',
        '’', '‚', '™', 'ﬁ', 'ﬂ', 'Ł', 'Œ', 'Š', 'Ÿ', 'Ž', 'ı', 'ł', 'œ', 'š', 'ž', '\u{FFFD}',
    ];
    b.iter()
        .map(|&x| match x {
            0x80..=0x9F => HI[(x - 0x80) as usize],
            0xA0 => '€',
            _ => x as char,
        })
        .collect()
}

/// 把整页位图裁到某个公式包围盒对应的像素区域，编码成 PNG。PDF 用户空间→像素空间的换算用
/// `page.render_dimensions()`/MediaBox 尺寸算缩放比（不精确到子像素，公式裁剪本来就不需要）。
pub(super) fn crop_pixmap_to_png(pixmap: &hayro::vello_cpu::Pixmap, region: &BBox, page: &hayro::hayro_syntax::page::Page) -> Option<Vec<u8>> {
    let (pw, ph) = page.render_dimensions();
    let (pix_w, pix_h) = (pixmap.width() as f64, pixmap.height() as f64);
    let sx = pix_w / pw.max(1.0) as f64;
    let sy = pix_h / ph.max(1.0) as f64;
    let x0 = (region.x0 * sx).max(0.0) as u32;
    let y1_img = pix_h - (region.y0 * sy); // PDF y 轴向上，图片 y 轴向下
    let y0_img = pix_h - (region.y1 * sy);
    let x1 = ((region.x1 * sx).min(pix_w)) as u32;
    let y0 = y0_img.max(0.0) as u32;
    let y1 = (y1_img.min(pix_h)) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = (pixmap.width() as u32, pixmap.height() as u32);
    let cropped = crop_rgba(pixmap.data_as_u8_slice(), w, h, x0, y0, (x1 - x0).max(1), (y1 - y0).max(1))?;
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(cropped).write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
}

/// 从整页 RGBA 像素（`w×h`，行优先）里只拷出 `(x, y, cw, ch)` 这一块（越界部分按 `crop_imm` 的规则收窄）。
/// 此前先把整页 `to_vec()` 成 `RgbaImage` 再裁：每个公式块都复制一整页（2 倍渲染的 A4 约 8MB），同页几个块就复制几遍
/// （2026-09-25 审计）。像素长度与宽高对不上 → `None`（同此前 `RgbaImage::from_raw` 的判据）。
pub(super) fn crop_rgba(data: &[u8], w: u32, h: u32, x: u32, y: u32, cw: u32, ch: u32) -> Option<image::RgbaImage> {
    if data.len() != (w as usize) * (h as usize) * 4 {
        return None;
    }
    let (x, y) = (x.min(w), y.min(h));
    let (cw, ch) = (cw.min(w - x), ch.min(h - y));
    let mut buf = Vec::with_capacity(cw as usize * ch as usize * 4);
    for row in y..y + ch {
        let start = (row as usize * w as usize + x as usize) * 4;
        buf.extend_from_slice(&data[start..start + cw as usize * 4]);
    }
    image::RgbaImage::from_raw(cw, ch, buf)
}
