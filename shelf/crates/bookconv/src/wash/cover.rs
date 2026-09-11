//! 封面声明：保证 OPF 声明了有效封面图。
use super::*;

// ───────────────────────── 封面声明 ─────────────────────────

/// 保证 OPF 声明了一个**有效的封面图**（xochitl 靠它生成书库里的封面缩略图）。2026-09-20 用户要求核查日志时发现：
/// 设备日志 `rm.epub.container Asked for unknown item ""` + `rm.docworker failed extracting cover: got null cover
/// image`——9 本已投的书里 7 本没有封面。原因有两类：①OPF 根本没有 `<meta name="cover">`（如火影）；②声明了但**指向的不是
/// 图片条目**（如 Calibre 产物 `<meta name="cover" content="cover.txt"/>`，指向一个 txt）。
///
/// 已有有效声明（`<meta name="cover">` 指向图片条目，或某图片条目带 `properties="cover-image"`）→ 不动，返回 `false`；
/// 否则取第一个 spine 页里的第一张图（漫画/画册的第一页就是封面），在 manifest 里找到对应条目：删掉旧的（无效）`cover`
/// meta，写入 `<meta name="cover" content="该条目 id"/>`，并给该条目补 `properties="cover-image"`（EPUB3），返回 `true`。
/// **必须在清洗（`wash_entries`）之前调用**：清洗会把只含 SVG 封面的 titlepage 当空页删掉（2026-09-20《镖人(卷四)》真机核对）。
/// 找不到候选也不动。只读 html/OPF 文本，不碰图片字节（流式优化阶段一时图片条目是空占位）。
pub fn ensure_cover_declared(entries: &mut [Entry]) -> bool {
    let Some(opf) = parse_opf(entries) else { return false };
    let text = String::from_utf8_lossy(&entries[opf.index].data).into_owned();
    struct It<'t> {
        tag: &'t str,
        id: &'t str,
        path: String,
        props: &'t str,
        image: bool,
    }
    let items: Vec<It> = manifest_items(&text)
        .into_iter()
        .map(|m| {
            let path = resolve(&opf.dir, &percent_decode(m.href));
            let image = m.media_type.starts_with("image/") || is_image_ext(&path);
            It { tag: m.tag, id: m.id, path, props: m.properties, image }
        })
        .collect();
    let meta_re = cover_meta_re();
    let has_prop = |i: &It| i.props.split_whitespace().any(|p| p == "cover-image");
    // meta 声明指向的图片条目（若有效）
    let meta_target = meta_re
        .find_iter(&text)
        .filter_map(|m| tag_attr(m.as_str(), "content"))
        .find_map(|id| items.iter().find(|i| i.id == id && i.image));
    // 目标封面条目：meta 指向的有效图片 → 已带 cover-image 属性的图片 → 前几页的第一张真实图片（下面找）。
    let existing = meta_target.or_else(|| items.iter().find(|i| i.image && has_prop(i)));
    // **meta 和 `properties="cover-image"` 必须同时有**（2026-09-20 真机对照实验：xochitl 对封面条目 id 带点的仅 meta 声明
    // ——如 Calibre/Sigil 产物的 `x00000001.jpg`——取不到封面，日志 `null cover image`；加上 `cover-image` 属性就取得到；
    // id 简单如 `cover` 时仅 meta 也行）。所以已有 meta 但条目缺属性也要补。
    if let Some(t) = existing {
        let meta_ok = meta_target.map(|m| m.id == t.id).unwrap_or(false);
        if meta_ok && has_prop(t) {
            return false;
        }
    }
    // 候选：前 12 个 spine 页（跳过导航页）里第一张能对上 manifest 图片条目的图。多看几页是因为封面页常常是只含一张
    // SVG `<image>` 的 titlepage；有的书（《镖人(卷四)》）连封面图本身都坏了——titlepage 和 cover.xhtml 引用的都是一个 239 字节的
    // `cover.txt` 文本残片，书里没有真封面——此时兜底用书里第一张真实图片（漫画的第一页）。
    static IMG: OnceLock<Regex> = OnceLock::new();
    let img_re = IMG.get_or_init(|| Regex::new(r##"(?is)<(?:img|image)\b[^>]*?(?:src|xlink:href|href)\s*=\s*"([^"#]+)""##).unwrap());
    let cand = existing.or_else(|| {
        opf.spine
            .iter()
            .filter(|p| Some(*p) != opf.nav_doc.as_ref())
            .take(12)
            .find_map(|page| {
                let e = entries.iter().find(|e| &e.name == page)?;
                let html = std::str::from_utf8(&e.data).ok()?;
                img_re.captures_iter(html).find_map(|c| {
                    let path = resolve(dir_of(page), &percent_decode(&c[1]));
                    items.iter().find(|i| i.image && i.path == path)
                })
            })
    });
    let Some(cover) = cand else { return false };
    let mut out = meta_re.replace_all(&text, "").into_owned();
    let new_tag = if has_prop(cover) {
        cover.tag.to_string()
    } else if cover.props.is_empty() {
        cover.tag.replacen(&format!(r#"id="{}""#, cover.id), &format!(r#"id="{}" properties="cover-image""#, cover.id), 1)
    } else {
        cover.tag.replacen(&format!(r#"properties="{}""#, cover.props), &format!(r#"properties="{} cover-image""#, cover.props), 1)
    };
    out = out.replacen(cover.tag, &new_tag, 1);
    let Some(pos) = out.find("</metadata>") else { return false };
    out.insert_str(pos, &format!(r#"<meta name="cover" content="{}"/>"#, cover.id));
    entries[opf.index].data = out.into_bytes();
    true
}
