//! 文件格式白名单的**单一事实源**：母版库收的书籍格式、字体、StarDict 词典、壁纸图片。
//! 网关 UI（`accept=` + 选中即拦）、各服务上传门（`AssetStore::allowed_ext`）、inbox 追平、CLI 都从这里派生，
//! 改一处全链同步（2026-09-05 用户定：所有上传口都要有格式限制，且网页与服务端同一份）。
//! 扩展名一律**小写、不带点**。

/// 原生 xochitl 直读（两个读器都能去）。
pub const NATIVE_EXTS: &[&str] = &["epub", "pdf"];
/// 电脑 `shelf push` 能转成 EPUB 进原生的源格式（= host `push.py` 的 `WASH_EXT` ∪ txt；两处同一份，改一处另一处同步）。
/// txt：中文网文，host `txt_to_epub.py` 按「第X章」切章建目录再洗（2026-09-06）；直接上传仍只能加入 KOReader（无章节）。
pub const HOST_CONVERTIBLE_EXTS: &[&str] = &["azw3", "mobi", "azw", "prc", "fb2", "txt"];
/// 只能加入 KOReader 的格式（设备装的 KOReader v2026.07.1 `documentregistry` 真机核对：crengine 收 txt/html/rtf/doc/docx/chm，
/// mupdf 收 cbz/cbr(libarchive 带 rar)/xps，djvu 引擎收 djvu）。
pub const KOREADER_ONLY_EXTS: &[&str] = &["cbz", "cbr", "djvu", "html", "htm", "rtf", "doc", "docx", "chm", "xps"];
/// 母版库收的书籍格式 = 上面三档之并（有测试钉死一致）。能投哪个读器按格式在落库时门控。
pub const BOOK_EXTS: &[&str] = &["epub", "pdf", "azw3", "mobi", "azw", "prc", "fb2", "txt", "cbz", "cbr", "djvu", "html", "htm", "rtf", "doc", "docx", "chm", "xps"];
/// TrueType / OpenType 字体（原生 fontconfig 与 KOReader 同一份）。
pub const FONT_EXTS: &[&str] = &["ttf", "otf", "ttc"];
/// StarDict 词典的组成文件。
pub const DICT_EXTS: &[&str] = &["ifo", "idx", "dict", "dz", "syn", "oft"];
/// 壁纸源图。
pub const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png"];

/// 文件名的扩展名（小写、不带点）；无扩展名 → 空串。
pub fn ext_of(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => ext.to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// 文件名扩展名是否在白名单里。**空白名单＝接受任意文件**（含无扩展名），对齐 KOReader books「任意格式原样」语义。
pub fn has_ext(name: &str, exts: &[&str]) -> bool {
    exts.is_empty() || exts.contains(&ext_of(name).as_str())
}

/// 白名单的展示形（带点、空格分隔），给拒收提示用：`.epub .pdf …`。
pub fn dotted(exts: &[&str]) -> String {
    exts.iter().map(|e| format!(".{e}")).collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ext_and_whitelist() {
        assert_eq!(ext_of("A.EPUB"), "epub");
        assert_eq!(ext_of("noext"), "");
        assert_eq!(ext_of(".hidden"), "", "点开头无主干不算扩展名");
        assert!(has_ext("x.Pdf", BOOK_EXTS) && !has_ext("x.jpg", BOOK_EXTS) && !has_ext("x", BOOK_EXTS));
        assert!(has_ext("anything", &[]), "空白名单收任意");
        assert_eq!(dotted(&["a", "b"]), ".a .b");
    }

    #[test]
    fn book_exts_is_union_of_tiers() {
        let mut tiers: Vec<&str> = [NATIVE_EXTS, HOST_CONVERTIBLE_EXTS, KOREADER_ONLY_EXTS].concat();
        let mut all: Vec<&str> = BOOK_EXTS.to_vec();
        tiers.sort();
        all.sort();
        assert_eq!(all, tiers, "BOOK_EXTS 必须等于三档之并");
        assert_eq!(all.len(), BOOK_EXTS.len(), "无重复");
    }
}
