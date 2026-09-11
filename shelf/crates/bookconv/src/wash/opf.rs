//! OPF 视图：定位 OPF、解出 manifest/spine/nav/ncx，以及 OPF 里的标识符/书名。
use super::*;

pub(super) fn find_opf(entries: &[Entry]) -> Option<usize> {
    // container.xml 指向优先，否则第一个 .opf
    if let Some(c) = entries.iter().find(|e| e.name == "META-INF/container.xml") {
        let t = String::from_utf8_lossy(&c.data);
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r#"full-path="([^"]+)""#).unwrap());
        if let Some(m) = re.captures(&t) {
            let p = posix_norm(&m[1]);
            if let Some(i) = entries.iter().position(|e| e.name == p) {
                return Some(i);
            }
        }
    }
    entries.iter().position(|e| e.name.to_ascii_lowercase().ends_with(".opf"))
}

// ───────────────────────── OPF 视图 ─────────────────────────

pub(crate) struct Opf {
    pub(crate) index: usize,
    pub(crate) dir: String,
    /// manifest id → zip 路径
    #[allow(dead_code)]
    pub(crate) items: HashMap<String, String>,
    /// spine 顺序的 zip 路径
    pub(crate) spine: Vec<String>,
    pub(crate) nav_doc: Option<String>,
    #[allow(dead_code)]
    pub(crate) ncx: Option<String>,
}

/// 标签里的 `name="value"` 属性对（只认双引号，属性名原样返回、由调用方决定大小写比较）。
/// OPF manifest 项、`<meta name="cover">`、container.xml 的 `<rootfile>` 等共用这一条正则（此前 `parse_opf`、
/// `ensure_cover_declared` 各编一份，`placeholder` 更是每取一个属性现编一个正则）。
pub(crate) fn tag_attrs(tag: &str) -> impl Iterator<Item = (&str, &str)> {
    static ATTR: OnceLock<Regex> = OnceLock::new();
    let attr = ATTR.get_or_init(|| Regex::new(r#"([a-zA-Z:-]+)\s*=\s*"([^"]*)""#).unwrap());
    attr.captures_iter(tag).map(|a| (a.get(1).map_or("", |m| m.as_str()), a.get(2).map_or("", |m| m.as_str())))
}

/// 标签里第一个名为 `name`（不分大小写）的属性值。
pub(crate) fn tag_attr<'a>(tag: &'a str, name: &str) -> Option<&'a str> {
    tag_attrs(tag).find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v)
}

/// OPF manifest 的一项（`href` 未解码、未解析成 zip 路径；属性重复时后者为准，与此前 HashMap 收集的行为一致）。
pub(crate) struct ManifestItem<'a> {
    /// 整个 `<item …>` 标签原文（改写 OPF 时按原文定位）。
    pub(crate) tag: &'a str,
    pub(crate) id: &'a str,
    pub(crate) href: &'a str,
    pub(crate) media_type: &'a str,
    pub(crate) properties: &'a str,
}

/// OPF 文本里全部带 `id` 与 `href` 的 manifest 项（文档序）。`parse_opf`、`ensure_cover_declared`、占位封面探测共用。
pub(crate) fn manifest_items(opf_text: &str) -> Vec<ManifestItem<'_>> {
    static ITEM: OnceLock<Regex> = OnceLock::new();
    let item = ITEM.get_or_init(|| Regex::new(r#"(?s)<item\b[^>]*?/?>"#).unwrap());
    item.find_iter(opf_text)
        .filter_map(|m| {
            let tag = m.as_str();
            let (mut id, mut href, mut media_type, mut properties) = (None, None, "", "");
            for (k, v) in tag_attrs(tag) {
                match k.to_ascii_lowercase().as_str() {
                    "id" => id = Some(v),
                    "href" => href = Some(v),
                    "media-type" => media_type = v,
                    "properties" => properties = v,
                    _ => {}
                }
            }
            Some(ManifestItem { tag, id: id?, href: href?, media_type, properties })
        })
        .collect()
}

/// `<meta name="cover" …>` 标签（`ensure_cover_declared` 与占位封面探测共用）。
pub(crate) fn cover_meta_re() -> &'static Regex {
    static META: OnceLock<Regex> = OnceLock::new();
    META.get_or_init(|| Regex::new(r#"(?s)<meta\b[^>]*\bname\s*=\s*"cover"[^>]*?/?>"#).unwrap())
}

pub(crate) fn parse_opf(entries: &[Entry]) -> Option<Opf> {
    let index = find_opf(entries)?;
    let dir = dir_of(&entries[index].name).to_string();
    let text = String::from_utf8_lossy(&entries[index].data);
    static REF: OnceLock<Regex> = OnceLock::new();
    let iref = REF.get_or_init(|| Regex::new(r#"<itemref\b[^>]*\bidref="([^"]+)""#).unwrap());
    let mut items = HashMap::new();
    let mut nav_doc = None;
    let mut ncx = None;
    for it in manifest_items(&text) {
        let path = resolve(&dir, &percent_decode(it.href));
        if it.properties.split_whitespace().any(|x| x == "nav") {
            nav_doc = Some(path.clone());
        }
        if it.media_type.contains("dtbncx") {
            ncx = Some(path.clone());
        }
        items.insert(it.id.to_string(), path);
    }
    let spine: Vec<String> = iref.captures_iter(&text).filter_map(|c| items.get(&c[1]).cloned()).collect();
    Some(Opf { index, dir, items, spine, nav_doc, ncx })
}

/// OPF 的 `unique-identifier` 实际取值（`<package unique-identifier="X">` 指向的那个
/// `<dc:identifier id="X">` 元素的文本内容）。EPUB2 规范要求 `toc.ncx` 的 `dtb:uid` 跟这个值
/// 完全一致——真机《疯探》坐实：这本"番茄小说 EPUB Generator"产物的 `toc.ncx` navMap 结构完全
/// 正确（94 条 navPoint 全部可达），但 `dtb:uid` 是生成器随手写的另一个 uuid，跟 OPF 的
/// `dc:identifier` 对不上；reMarkable 原生目录面板遇到这种不匹配**直接不显示目录入口**（不是
/// 显示空列表），换一本 `dtb:uid` 匹配的书（《雪人》）目录入口就在。见 `fix_ncx_uid`。
pub(super) fn opf_unique_identifier(entries: &[Entry]) -> Option<String> {
    let i = find_opf(entries)?;
    let text = String::from_utf8_lossy(&entries[i].data);
    static PKG: OnceLock<Regex> = OnceLock::new();
    let pkg_re = PKG.get_or_init(|| Regex::new(r#"<package\b[^>]*\bunique-identifier="([^"]+)""#).unwrap());
    let uid_attr = &pkg_re.captures(&text)?[1];
    static ID: OnceLock<Regex> = OnceLock::new();
    let id_re = ID.get_or_init(|| Regex::new(r#"(?s)<dc:identifier\b[^>]*\bid="([^"]+)"[^>]*>([^<]*)</dc:identifier>"#).unwrap());
    id_re.captures_iter(&text).find(|c| &c[1] == uid_attr).map(|c| c[2].trim().to_string())
}

/// OPF `<dc:title>` 的纯文本内容，取不到时兜底"目录"。
pub(super) fn opf_book_title(entries: &[Entry], opf_index: usize) -> String {
    let t = String::from_utf8_lossy(&entries[opf_index].data);
    static T: OnceLock<Regex> = OnceLock::new();
    T.get_or_init(|| Regex::new(r#"(?s)<dc:title[^>]*>(.*?)</dc:title>"#).unwrap()).captures(&t).map(|c| plain_text(&c[1])).unwrap_or_else(|| "目录".into())
}
