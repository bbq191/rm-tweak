//! 漫画 / 无文字层 PDF 的裁边优化（格式不变）。
use super::*;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdfTrimReport {
    pub pages: usize,
}

/// 无文字层/漫画 PDF：逐页取主图片字节→ `imgopt::prepare_comic_page_for_pdf`（裁边+按需缩放，单趟）
/// →喂给 `PdfPieceWriter` 写出新 PDF（复用漫画 EPUB→PDF 那条产线的写手）。
///
/// **不允许变动书籍内容**（用户 2026-09-20 明确要求），所以这条路径**只处理"零文字、每页恰好一张
/// 整页图"的 PDF**——重写页面等于丢掉图片以外的一切，其它形状一律拒绝并保持原文件不动：
/// - 有任何可提取文字（含扫描件的 OCR 隐形文字层）：重写会丢文字层；
/// - 某页不是"单张整页图"（矢量图形、多图拼版、纯文字页）：重写会丢掉这一页的其余内容；
/// - 无法确认没有文字层（提取失败）：宁可不动。
///
/// 此前没有这些闸门，且 `encode_pdf_image` 名义上"给 trim_margins 用"却**从未调用 trim_margins**
/// （实测根本没裁边）、`finish(&[])` 还把原 PDF 的书签全部丢掉。
///
/// **目录**：保留原 PDF 书签（页码映射到输出页，层级压平）；原文件没有书签则按页分段兜底
/// （[`crate::comic_pdf::page_chunk_titles`]），保证输出一定有目录。
pub fn optimize_pdf_trim_only(src: &Path, dst_tmp: &Path, mut on_progress: impl FnMut(usize, usize)) -> Result<PdfTrimReport, String> {
    let doc = load_pdf(src)?;
    let pages = doc.get_pages();
    let page_count = pages.len();
    if page_count == 0 {
        return Err("PDF 没有可用页面".into());
    }
    let text_pages = extract_positioned_text_doc(&doc).map_err(|e| format!("无法确认这个 PDF 没有文字层，为保住书籍内容不做改动：{e}"))?;
    let text_chars: usize = text_pages.iter().map(|p| p.chars.iter().filter(|c| c.ch != '\u{0}' && !c.ch.is_whitespace()).count()).sum();
    if text_chars > 0 {
        return Err(format!("这个 PDF 含 {text_chars} 个可提取文字（文字层），改写页面会丢掉文字，保持原样"));
    }
    for (i, (_, page_id)) in pages.iter().enumerate() {
        let n_images = doc.get_page_images(*page_id).map(|v| v.len()).unwrap_or(0);
        if n_images != 1 || !page_covered_by_big_image(&doc, *page_id) {
            return Err(format!("第 {} 页不是\"单张整页图片\"（图片 {n_images} 张），改写页面会丢掉这一页的其余内容，保持原样", i + 1));
        }
    }
    let mut titles: Vec<(usize, String)> = doc
        .get_toc()
        .map(|t| t.toc.iter().map(|e| (e.page.saturating_sub(1).min(page_count - 1), e.title.clone())).collect())
        .unwrap_or_default();
    if titles.is_empty() {
        titles = crate::comic_pdf::page_chunk_titles(page_count);
    }
    let mut writer = PdfPieceWriter::begin(page_count, true);
    for (i, (_, page_id)) in pages.iter().enumerate() {
        on_progress(i, page_count);
        let images = doc.get_page_images(*page_id).map_err(|e| format!("读第 {} 页图片失败: {e}", i + 1))?;
        let raw = decode_pdf_image_to_bytes(&doc, &images[0])?;
        let sized = crate::imgopt::prepare_comic_page_for_pdf(&raw, pdfwrite::PDF_PAGE_W, pdfwrite::PDF_PAGE_H).unwrap_or(raw);
        writer.write_page(&pdfwrite::image_from_bytes(&sized)?)?;
    }
    on_progress(page_count, page_count);
    let out = writer.finish(&titles)?;
    std::fs::write(dst_tmp, &out).map_err(|e| format!("写出临时文件失败: {e}"))?;
    Ok(PdfTrimReport { pages: page_count })
}
