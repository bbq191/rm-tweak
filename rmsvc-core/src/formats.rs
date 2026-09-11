//! 文件格式白名单的**单一事实源**：母版库收的书籍格式、字体、StarDict 词典、壁纸图片。
//! 网关 UI（`accept=` + 选中即拦）、各服务上传门（`AssetStore::allowed_ext`）、inbox 追平、CLI 都从这里派生，
//! 改一处全链同步（2026-09-05 用户定：所有上传口都要有格式限制，且网页与服务端同一份）。
//! 扩展名一律**小写、不带点**。

/// 原生 xochitl 直读（两个读器都能去）。母版库收的书籍格式与这份完全相同（见 `BOOK_EXTS`）。
pub const NATIVE_EXTS: &[&str] = &["epub", "pdf"];
/// 母版库收的书籍格式。⚠ 2026-09-18 用户明确要求"从此开始入库只入 PDF 和 EPUB，不论格式是否
/// 支持"——**这是策略收紧，不是技术能力判断**：CBZ/CBR/DJVU/HTML/HTM/RTF/DOC/DOCX/CHM/XPS 这些
/// 格式设备装的 KOReader 本来能读（真机 `documentregistry` 核对过：crengine 收 txt/html/rtf/doc/
/// docx/chm，mupdf 收 cbz/cbr(libarchive 带 rar)/xps，djvu 引擎收 djvu），但用户不想再维护"仅
/// KOReader 能读"这一档，一律拒收，只留原生两读器都能去的 EPUB/PDF。2026-09-17 那次已经砍掉的
/// azw3/mobi/azw/prc/fb2/txt（原"电脑可转"档，靠已砍的 host `shelf push` Calibre 管线转 EPUB）
/// 保持不收，这次是在那次基础上把仅 KOReader 那一档也砍掉，两档收成一档。
pub const BOOK_EXTS: &[&str] = NATIVE_EXTS;
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
    fn book_exts_equals_native_exts() {
        // 2026-09-18 起两档收成一档：母版库只收 EPUB/PDF，不再区分"仅 KOReader"。
        assert_eq!(BOOK_EXTS, NATIVE_EXTS);
    }

    #[test]
    fn retired_host_convertible_exts_no_longer_accepted() {
        // 2026-09-17 EPUB 线架构调整：azw3/mobi/azw/prc/fb2/txt 不再自动转 EPUB，母版库也不收。
        for ext in ["azw3", "mobi", "azw", "prc", "fb2", "txt"] {
            assert!(!has_ext(&format!("x.{ext}"), BOOK_EXTS), "{ext} 应已从 BOOK_EXTS 退役");
        }
    }

    #[test]
    fn retired_koreader_only_tier_no_longer_accepted() {
        // 2026-09-18 用户明确要求"入库只入 PDF 和 EPUB，不论格式是否支持"——KOReader 技术上能读
        // 这些格式，但策略上不再收，母版库直接拒收。
        for ext in ["cbz", "cbr", "djvu", "html", "htm", "rtf", "doc", "docx", "chm", "xps"] {
            assert!(!has_ext(&format!("x.{ext}"), BOOK_EXTS), "{ext} 应已从 BOOK_EXTS 退役");
        }
    }
}
