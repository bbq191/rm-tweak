//! "PDF 转出的 EPUB"来源标记：`book_id` 前缀约定与识别。
use super::*;

/// PDF 转出的 EPUB 的 `BookMeta.book_id` 前缀（来源标记）。塞进 `publisher` 会污染真实元数据、往正文塞不可见
/// 占位不优雅，所以约定 `book_id` 前缀——跟 [`looks_like_pdf_derived_epub`] 配对识别。
pub(super) const PDF_BOOK_ID_PREFIX: &str = "pdf:";

/// 识别"这份 EPUB 是入库 PDF 转出来的"——检查 `content.opf` 里的 `dc:identifier` 是不是
/// `"pdf:"` 前缀（`optimize_pdf_to_epub` 用 `book_id: format!("pdf:{title}")` 构造，
/// `epub.rs::content_opf` 原样写进 `<dc:identifier id="pub-id">weread:{book_id}</dc:
/// identifier>`）。跟 `pdfwrite::looks_like_own_bookconv_pdf` 同构：开 zip、读一个文件、
/// 找标记字符串，不用完整解析 EPUB 结构。
pub fn looks_like_pdf_derived_epub(path: &Path) -> bool {
    let Ok(file) = std::fs::File::open(path) else { return false };
    let Ok(mut zip) = zip::ZipArchive::new(std::io::BufReader::new(file)) else { return false };
    let Ok(mut entry) = zip.by_name(crate::epub::OPF_PATH) else { return false };
    let mut buf = String::new();
    if std::io::Read::read_to_string(&mut entry, &mut buf).is_err() {
        return false;
    }
    buf.contains(&format!("{}{PDF_BOOK_ID_PREFIX}", crate::epub::ID_SCHEME))
}
