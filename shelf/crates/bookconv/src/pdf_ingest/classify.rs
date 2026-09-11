//! PDF 分类：漫画/扫描件（无文字层）/有文字层三类，决定「优化」走裁边还是转 EPUB。
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PdfKind {
    /// 漫画：≥[`COMIC_PAGE_RATIO`] 的页都被一张覆盖页面主体面积的图片占满。
    Comic,
    /// 没有可提取的文字层（扫描件、纯图片 PDF），不是漫画形状但也没有文字可转。
    NoTextLayer,
    /// 有真实可提取的文字层，走 PDF→EPUB 转换。
    TextLayer,
}

/// 判定漫画的页数比例——`≥` 这个比例的页都是"一张图基本铺满整页"才判漫画，镜像
/// `comic_detect::is_comic` 的"图多字少"哲学，只是漫画 EPUB 按"图片数量"计，PDF 天然以页为
/// 单位，改成"页比例"。
pub const COMIC_PAGE_RATIO: f64 = 0.9;
/// 单张图片相对页面面积的覆盖比例阈值——超过这个比例才算"铺满整页"。
pub const COMIC_IMAGE_AREA_RATIO: f64 = 0.85;
/// 判定"有文字层"的每页平均字符数下限（不含空白）。参考 `comic_detect::TEXT_PER_IMAGE=40`
/// 的量级换算成"每页"版本——具体数值已经拿 `tests/fixtures/sample.pdf`（真实 pdflatex 样本，
/// 每页几百字）核对过跟正常文字书的量级差距足够大，不是拍脑袋定的。
pub const MIN_CHARS_PER_PAGE: f64 = 40.0;

/// 判不准（打不开/解析失败）一律退到安全默认：[`PdfKind::NoTextLayer`]（只裁边，不转换）——
/// 跟 `comic_detect::is_comic_epub_file` "打不开当不是漫画、走现状老路径" 同一个"失败模式选
/// 更保守那条"原则：裁边是幂等、低风险操作，转 EPUB 是破坏性格式变更，判不准时不能选后者。
pub fn classify_pdf(path: &Path) -> PdfKind {
    load_pdf(path).map_or(PdfKind::NoTextLayer, |doc| classify_doc(&doc))
}

#[cfg(test)]
pub(super) fn classify_pdf_bytes(bytes: &[u8]) -> PdfKind {
    parse_pdf(bytes).map_or(PdfKind::NoTextLayer, |doc| classify_doc(&doc))
}

fn classify_doc(doc: &lopdf::Document) -> PdfKind {
    let pages = doc.get_pages();
    if pages.is_empty() {
        return PdfKind::NoTextLayer;
    }
    let total = pages.len();
    let mut comic_pages = 0usize;
    for page_id in pages.values() {
        if page_covered_by_big_image(doc, *page_id) {
            comic_pages += 1;
        }
    }
    if comic_pages as f64 / total as f64 >= COMIC_PAGE_RATIO {
        return PdfKind::Comic;
    }
    let Ok(text_pages) = extract_positioned_text_doc(doc) else { return PdfKind::NoTextLayer };
    let total_chars: usize = text_pages.iter().map(|p| p.chars.iter().filter(|c| !c.ch.is_whitespace()).count()).sum();
    let avg = total_chars as f64 / total as f64;
    if avg >= MIN_CHARS_PER_PAGE {
        PdfKind::TextLayer
    } else {
        PdfKind::NoTextLayer
    }
}

/// 这一页是不是被一张覆盖主体面积的图片占满——只看图片声明的像素宽高比 MediaBox 面积占比的
/// 粗略近似（不去解析内容流里的 `cm` 变换算精确摆放尺寸；漫画/扫描页几乎都是"一张图等比撑满
/// 整页"，用图片自身宽高比 vs 页面宽高比接近、且没有第二张显著大小的图这个粗判据足够）。
pub(super) fn page_covered_by_big_image(doc: &lopdf::Document, page_id: lopdf::ObjectId) -> bool {
    let Ok(images) = doc.get_page_images(page_id) else { return false };
    if images.is_empty() {
        return false;
    }
    let Ok(dict) = doc.get_dictionary(page_id) else { return false };
    let Some((pw, ph)) = media_box_size(doc, dict) else { return false };
    let page_area = pw * ph;
    if page_area <= 0.0 {
        return false;
    }
    images.iter().any(|img| {
        // 图片是位图，没有物理尺寸；用"宽高比跟页面宽高比接近"代替"面积占比"（面积单位不同、
        // 没法直接比较像素面积和 pt 面积）——比例接近说明这张图大概率是整页等比缩放摆放的。
        let img_ratio = img.width as f64 / img.height.max(1) as f64;
        let page_ratio = pw / ph.max(0.001);
        (img_ratio / page_ratio - 1.0).abs() < (1.0 - COMIC_IMAGE_AREA_RATIO)
    })
}

pub(super) fn media_box_size(doc: &lopdf::Document, page_dict: &lopdf::Dictionary) -> Option<(f64, f64)> {
    let mb = get_inherited_media_box(doc, page_dict)?;
    if mb.len() < 4 {
        return None;
    }
    let w = (mb[2].as_float().unwrap_or(0.0) - mb[0].as_float().unwrap_or(0.0)).abs();
    let h = (mb[3].as_float().unwrap_or(0.0) - mb[1].as_float().unwrap_or(0.0)).abs();
    Some((w as f64, h as f64))
}

pub(super) fn get_inherited_media_box(doc: &lopdf::Document, page_dict: &lopdf::Dictionary) -> Option<Vec<lopdf::Object>> {
    if let Ok(arr) = page_dict.get(b"MediaBox").and_then(|o| o.as_array()) {
        return Some(arr.clone());
    }
    let parent_ref = page_dict.get(b"Parent").ok()?.as_reference().ok()?;
    let parent = doc.get_dictionary(parent_ref).ok()?;
    get_inherited_media_box(doc, parent)
}

// ============================================================================
// 逐字符位置提取（pdf-extract OutputDev 驱动）
// ============================================================================
