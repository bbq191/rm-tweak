//! EPUB 漫画 → PDF：真机反复实测坐实 xochitl 的 EPUB 渲染走"文字排版盒模型"，内容区相对物理
//! 页面有一个消不掉的固定内边距（`.content` 元数据的 `margins` 字段，UI 只给 28/56/112 三档
//! 离散预设；直接改文件也没用——xochitl 渲染文档时会用自己的逻辑把这个字段覆盖回去），文字页和
//! 图片页共享同一份限制，CSS 层面（`width` 超 100%、负 `margin`）也测过绕不开——这是 EPUB 渲染
//! 路径本身的硬限制，不是我们代码的 bug。同一批真机实测：PDF 直传（页面物理尺寸精确等于设备
//! 屏幕 954×1696px）左右留白量得 **0.00%**，且 `.content` 里 PDF 文档根本没有 `margins` 这个
//! 字段——PDF 走的是完全独立于 EPUB 文字排版盒模型的直接光栅化路径，从根上不受这个限制。
//!
//! 这个模块是"漫画类 EPUB 优化时改产出 PDF（带书签）"的实现，跟 `comic_split.rs`（EPUB→EPUB
//! 按卷拆分）平行独立、互不影响；复用它的 NCX 标题解析（`ncx_titles_in_range`）和 `imgs_
//! referenced`，图片处理用 `imgopt::prepare_comic_page_for_pdf`（裁边+缩放合成单趟，PDF 不需要靠补白像素控制留白分布，
//! 直接在页面里摆位置即可，摆位算法见 `convert::pdfwrite::place_image`）。也不碰 `convert::
//! pdfwrite::images_to_pdf`/`convert::cbz`（CBZ→PDF 现状路径），只用新增的 `images_to_pdf_
//! with_toc`/`extract_pages`/`page_count`。

use crate::convert::pdfwrite;
use crate::epubzip::{dir_of, is_html, Entry};
use crate::wash::parse_opf;
use std::path::Path;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PdfReport {
    pub pages: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
}

/// **当前生产路径不调用**（2026-09-20 用户拍板"优化不改格式"，漫画 EPUB 优化保持 EPUB，见
/// `book-serve::Staging::optimize`）。保留是因为"漫画 EPUB→整本 PDF"仍是有测试覆盖的可用能力（含 `PdfPieceWriter`
/// 流式写出与目录书签），日后若要重开这条路（如超限漫画改投 PDF）不必重写；本仓库内只有单测引用它。
/// 不标 `#[cfg(test)]`：book-serve 的测试也要跨 crate 调用它。
///
/// 读入方式仿 `comic_split::deliver_split_streaming` 阶段一：图片条目留空占位、只读 html/opf/ncx
/// 真实字节判断是不是漫画+抽标题；真正的图片字节按需读、处理完立刻编进 `PdfImage` 就丢原始字节，
/// 峰值内存是"处理到哪张图"而不是"全书图片"，跟现有 EPUB 拆分路径同一套纪律。
///
/// **一图一页**（不像 EPUB 那样允许一个 spine 页塞多张图）——漫画天然就是一页一图，PDF 场景更
/// 贴近这个语义；一个 spine 页如果引用了 N 张图，会展开成 N 个 PDF 页，NCX 标题落在这个 spine
/// 页对应的第一张图上。零图的纯文字页（如后记）在 PDF 场景没有对应物，直接跳过不产出页面——
/// PDF 不是文字排版容器，硬塞文字进去不是这次任务范围。
pub fn optimize_comic_epub_to_pdf_streaming(
    input_path: &Path,
    output_path: &Path,
    mut on_progress: impl FnMut(usize, usize),
) -> Result<PdfReport, String> {
    let bytes_before = std::fs::metadata(input_path).map(|m| m.len() as usize).unwrap_or(0);
    let file = std::fs::File::open(input_path).map_err(|e| format!("打开母版库文件失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file))
        .map_err(|e| format!("解 EPUB(非 zip?): {e}"))?;

    let entries: Vec<Entry> = crate::epubzip::read_skeleton(&mut zip)?.entries;
    drop(zip);

    if !crate::comic_detect::is_comic(&entries) {
        return Err("不是漫画书，漫画→PDF 这条路径不适用".into());
    }
    let (_, text_chars) = crate::comic_detect::epub_image_stats(&entries);
    if text_chars > 0 {
        // PDF 一图一页，文字页/图片页里夹的文字没有对应物、会被丢掉——不允许变动书籍内容，拒绝而不是静默丢字。
        return Err(format!("这本书含 {text_chars} 字正文文字（版权页/章节标题/台词等），转 PDF 会丢掉文字，保持 EPUB"));
    }
    let opf = parse_opf(&entries).ok_or("解不出 OPF/spine")?;
    let titles_by_spine_idx =
        crate::comic_split::ncx_titles_in_range(&entries, &opf, 0, opf.spine.len());

    // 先摸一遍每个 spine 项引用了几张图，凑出总图数给进度条用（不解码，只读 html 文本里的 <img> 引用）。
    let mut per_page_imgs: Vec<Vec<String>> = Vec::with_capacity(opf.spine.len());
    for p in &opf.spine {
        if !is_html(p) {
            per_page_imgs.push(Vec::new());
            continue;
        }
        let imgs = entries
            .iter()
            .find(|e| &e.name == p)
            .and_then(|e| std::str::from_utf8(&e.data).ok())
            .map(|html| crate::comic_split::imgs_referenced(html, dir_of(p)))
            .unwrap_or_default();
        per_page_imgs.push(imgs);
    }
    let total_imgs: usize = per_page_imgs.iter().map(|v| v.len()).sum();
    if total_imgs == 0 {
        return Err("没有找到任何图片，没法生成漫画 PDF".into());
    }

    let file2 = std::fs::File::open(input_path).map_err(|e| format!("重开母版库文件失败: {e}"))?;
    let mut zip2 = zip::ZipArchive::new(std::io::BufReader::new(file2)).map_err(|e| e.to_string())?;

    // 用 PdfPieceWriter 逐页读逐页写——`total_imgs`（总页数）已经在上面数出来了，不用先攒出
    // 整本书的 `Vec<PdfImage>` 才知道有几页。真机 245MB/600页 样本坐实过：先攒整本 `Vec<PdfImage>`
    // 再一次性序列化，`VmHWM` 峰值到过 595MB（images 副本 + 序列化中间态叠加）；这里改成一张图
    // 处理完立刻写进 writer 内部缓冲区、这张图的 `PdfImage`/原始字节就地释放，峰值只剩 writer
    // 自己那份累积输出（约等于最终 PDF 体积本身，不再叠加一份"全书图片"的额外副本）。
    // `has_toc` 恒为 true——下面兜底逻辑保证 `titles` 最终不可能是空的（至少有整本书名这一条）。
    let mut writer = pdfwrite::PdfPieceWriter::begin(total_imgs, true);
    let mut titles: Vec<(usize, String)> = Vec::new();
    let mut done = 0usize;
    let mut written = 0usize;
    for (spine_idx, imgs) in per_page_imgs.iter().enumerate() {
        if imgs.is_empty() {
            continue;
        }
        if let Some(title) = titles_by_spine_idx.get(&spine_idx) {
            titles.push((written, title.clone()));
        }
        for img_path in imgs {
            let raw = crate::epubzip::read_by_name(&mut zip2, img_path).map_err(|e| format!("读图片 {img_path} 失败: {e}"))?;
            // 单趟：裁边+一次缩到 PDF 实际绘制的整数像素尺寸+一次编码（见该函数文档：此前两道串联
            // 造成重采样两遍/JPEG 两代/灰度转 RGB）。返回 None = 无需处理，直接嵌原图字节零损失。
            let sized = crate::imgopt::prepare_comic_page_for_pdf(&raw, pdfwrite::PDF_PAGE_W, pdfwrite::PDF_PAGE_H).unwrap_or(raw);
            let pdf_img = pdfwrite::image_from_bytes(&sized)
                .map_err(|e| format!("图片 {img_path} 编不进 PDF: {e}"))?;
            writer.write_page(&pdf_img)?;
            written += 1;
            done += 1;
            on_progress(done, total_imgs);
        }
    }
    if titles.is_empty() {
        // 源书自己就没有目录（NCX 空/畸形；乱马、火影实测就是空 NCX）——按页分段给书签，至少能按段跳转，
        // 不是只有一条书名。如实标"第 N–M 页"，不假装是章节。
        titles = page_chunk_titles(written);
    }

    let pdf_bytes = writer.finish(&titles)?;
    let bytes_after = pdf_bytes.len();
    let pages = written;
    std::fs::write(output_path, &pdf_bytes).map_err(|e| format!("写出 PDF 失败: {e}"))?;
    Ok(PdfReport { pages, bytes_before, bytes_after })
}

/// 没有源目录时的兜底书签：每 [`FALLBACK_TOC_PAGES`] 页一条，标题"第 N–M 页"（页码 1 起）。
const FALLBACK_TOC_PAGES: usize = 20;
pub(crate) fn page_chunk_titles(total_pages: usize) -> Vec<(usize, String)> {
    (0..total_pages)
        .step_by(FALLBACK_TOC_PAGES)
        .map(|start| (start, format!("第 {}–{} 页", start + 1, (start + FALLBACK_TOC_PAGES).min(total_pages))))
        .collect()
}

/// 超预算时按卷拆分——**只处理"自己产出的漫画 PDF"**（没有书签目录，视为普通用户上传的原生 PDF，
/// 返回 `Ok(None)`，调用方按现状"整本拒绝"处理，不是新引入的失败模式）。
///
/// 先按书签（对应原书 NCX 顶层"卷"边界）分组；某一卷自己还超预算（罕见，如单卷本身就很大）时
/// 在这一卷内部按页贪心再切一层，不再往更深递归——跟 `comic_split::plan_pieces_sized` 2026-09-19
/// 简化"只切一层"是同一个理由：真实漫画每卷体积通常远低于上传预算，为这种边界情况维护一整套
/// 递归复杂度不值得。
///
/// **流式**（2026-09-19 用户反馈驱动，见下）：规划阶段只用 [`pdfwrite::PdfFileReader::
/// page_byte_span_len`] 这种纯查表的"体积代理"，不读任何图片字节；真正组包时逐份读（这一份
/// 引用到的图片才读进内存），`upload_piece` 回调处理完这一份、函数往下一份继续之前，这一份的
/// `Vec<PdfImage>` 出作用域即释放——峰值内存只有"一份的体积"（受 `budget` 钳制），不随全书
/// 页数/体积线性涨，跟 `comic_split::deliver_split_streaming` 是同一套纪律。
///
/// 前身是一次性 `std::fs::read` 整份文件 + `extract_pages` 把全书图片一次性抽成 `Vec<PdfImage>`
/// 再逐份切片——真机 245MB/600页 样本坐实这个前身版本 `VmHWM` 峰值到过 525MB，用户反馈后改成
/// 这一版。
pub fn deliver_split_pdf_streaming(
    pdf_path: &Path,
    budget: u64,
    mut upload_piece: impl FnMut(&str, &[u8], usize, usize) -> Result<(), String>,
) -> Result<Option<crate::comic_split::StreamSplitOutcome>, String> {
    let whole_file_size = std::fs::metadata(pdf_path).map(|m| m.len()).unwrap_or(0); // 跟调用方 `deliver()` 判超限用的同一个量
    let Ok(mut reader) = pdfwrite::PdfFileReader::open(pdf_path) else { return Ok(None) };
    let Ok(n) = reader.page_count() else { return Ok(None) };
    let Ok(mut all_titles) = reader.outline_titles() else { return Ok(None) };
    // 指向不存在页的书签（损坏文件）丢掉——此前原样当切分边界，`sizes[start..end]` 在 start>end 时直接 panic。
    all_titles.retain(|(idx, _)| *idx < n);
    if all_titles.is_empty() {
        return Ok(None); // 没有书签目录——不是我们自己产出的漫画 PDF，不拆。
    }
    if whole_file_size <= budget {
        return Ok(None); // 整本已经在预算内，调用方按"不用拆，直接投原生"处理
    }

    // 规划阶段：纯查表算每页"占多少字节"（对象在文件里的字节范围，不读实际图片内容），
    // 按卷边界贪心分组、卷内超预算再按页贪心细切——逻辑跟前身版本一致，只是数据来源变了。
    let sizes: Vec<u64> = (0..n).map(|p| reader.page_byte_span_len(p)).collect();
    let mut boundaries: Vec<usize> = all_titles.iter().map(|(idx, _)| *idx).collect();
    boundaries.sort_unstable();
    boundaries.dedup();
    if boundaries.first() != Some(&0) {
        boundaries.insert(0, 0);
    }
    let title_at: std::collections::HashMap<usize, String> = all_titles.into_iter().collect();

    let mut ranges: Vec<(usize, usize, String)> = Vec::new();
    for (i, &start) in boundaries.iter().enumerate() {
        let end = boundaries.get(i + 1).copied().unwrap_or(n);
        let vol_title = title_at.get(&start).cloned().unwrap_or_else(|| format!("第 {}-{} 页", start + 1, end));
        let vol_size: u64 = sizes[start..end].iter().sum();
        if vol_size <= budget {
            ranges.push((start, end, vol_title));
            continue;
        }
        // 这一卷本身还超预算：按页贪心再切一层。
        let (mut s, mut acc) = (start, 0u64);
        for (i, &size) in sizes.iter().enumerate().take(end).skip(start) {
            if acc > 0 && acc + size > budget {
                ranges.push((s, i, format!("{vol_title}（第 {}-{} 页）", s + 1, i)));
                s = i;
                acc = 0;
            }
            acc += size;
        }
        ranges.push((s, end, format!("{vol_title}（第 {}-{} 页）", s + 1, end)));
    }

    // 组包+上传阶段：逐份来，每份只读这份自己范围内的图片，处理完（无论成败）立刻释放。
    let total = ranges.len();
    let stem = pdf_path.file_stem().and_then(|s| s.to_str()).unwrap_or("漫画").to_string();
    let mut delivered = Vec::new();
    let mut failed = Vec::new();
    for (idx, (start, end, title)) in ranges.into_iter().enumerate() {
        // 用 PdfPieceWriter 逐页读逐页写：每页的 PdfImage 只在这一次循环迭代里活着，写进
        // writer 内部缓冲区后立刻释放——不再像之前那样先攒出这一份的 `Vec<PdfImage>`（那样峰值
        // 会贴着"这一份的体积"再乘二），峰值现在约等于"这一份的体积"本身。
        let mut writer = pdfwrite::PdfPieceWriter::begin(end - start, true);
        let mut read_err = None;
        for page_idx in start..end {
            match reader.read_page_image(page_idx).and_then(|img| writer.write_page(&img)) {
                Ok(()) => {}
                Err(e) => {
                    read_err = Some(e);
                    break;
                }
            }
        }
        if let Some(e) = read_err {
            failed.push(format!("{title}（读页失败：{e}）"));
            continue;
        }
        match writer.finish(&[(0, title.clone())]) {
            Ok(bytes) => {
                // 2026-09-19 真机撞过：原书名+分卷标题（源文件自己的目录/书签，可能整份就
                // 一条、内容等于原书名本身）两段各自独立超长，直接拼会撞 255 字节文件系统上限，
                // 表现成 xochitl 泛化的"Filesystem error"，见 `util::safe_piece_filename` 文档。
                let piece_name = crate::util::safe_piece_filename(&stem, &title, "pdf");
                match upload_piece(&piece_name, &bytes, idx + 1, total) {
                    Ok(()) => delivered.push(title),
                    Err(e) => failed.push(format!("{title}（上传失败：{e}）")),
                }
            }
            Err(e) => failed.push(format!("{title}（组包失败：{e}）")),
        }
        // images/bytes 出循环体作用域即释放，下一份开始前这一份占的内存已经收回。
    }
    Ok(Some(crate::comic_split::StreamSplitOutcome { delivered, failed }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::convert::pdfwrite::PdfImage;
    use std::io::Write;

    fn one_px_jpeg() -> Vec<u8> {
        // 复用 pdfwrite 测试里用过的最小 JPEG 骨架构造思路：只需 SOI+SOF0+EOI，宽高任意。
        vec![
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x10, 0x03, 0x01, 0x11,
            0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xFF, 0xD9,
        ]
    }

    fn build_test_epub(chapters: &[(&str, &[&str])]) -> Vec<u8> {
        build_test_epub_opts(chapters, "", true)
    }

    /// `text` 非空时塞进第一章 body（模拟版权页/台词）；`with_ncx=false` 时 NCX 为空 navMap（模拟乱马/火影）。
    fn build_test_epub_opts(chapters: &[(&str, &[&str])], text: &str, with_ncx: bool) -> Vec<u8> {
        // chapters: (spine 文件名, 引用的图片文件名列表)；每个 chapter 一个最小 xhtml。
        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut z = zip::ZipWriter::new(cursor);
            let opt = zip::write::SimpleFileOptions::default();
            z.start_file("mimetype", opt).unwrap();
            z.write_all(b"application/epub+zip").unwrap();
            z.start_file("META-INF/container.xml", opt).unwrap();
            z.write_all(br#"<?xml version="1.0"?><container><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#).unwrap();

            let mut manifest_items = String::new();
            let mut spine_items = String::new();
            let mut nav_points = String::new();
            let jpeg = one_px_jpeg();
            let mut img_names: Vec<String> = Vec::new();
            for (i, (chap, imgs)) in chapters.iter().enumerate() {
                let mut body: String = imgs
                    .iter()
                    .map(|img| format!(r#"<img src="{img}"/>"#))
                    .collect();
                if i == 0 && !text.is_empty() {
                    body.push_str(&format!("<p>{text}</p>"));
                }
                z.start_file(format!("OEBPS/{chap}"), opt).unwrap();
                z.write_all(format!("<html><body>{body}</body></html>").as_bytes()).unwrap();
                manifest_items.push_str(&format!(r#"<item id="c{i}" href="{chap}" media-type="application/xhtml+xml"/>"#));
                spine_items.push_str(&format!(r#"<itemref idref="c{i}"/>"#));
                if with_ncx { nav_points.push_str(&format!(
                    r#"<navPoint><navLabel><text>第{i}章</text></navLabel><content src="{chap}"/></navPoint>"#
                )); }
                for img in imgs.iter() {
                    if !img_names.contains(&img.to_string()) {
                        img_names.push(img.to_string());
                    }
                }
            }
            for img in &img_names {
                z.start_file(format!("OEBPS/{img}"), opt).unwrap();
                z.write_all(&jpeg).unwrap();
                manifest_items.push_str(&format!(r#"<item id="{img}" href="{img}" media-type="image/jpeg"/>"#));
            }
            manifest_items.push_str(r#"<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#);
            z.start_file("OEBPS/content.opf", opt).unwrap();
            z.write_all(format!(
                r#"<?xml version="1.0"?><package><metadata></metadata><manifest>{manifest_items}</manifest><spine toc="ncx">{spine_items}</spine></package>"#
            ).as_bytes()).unwrap();
            z.start_file("OEBPS/toc.ncx", opt).unwrap();
            z.write_all(format!(r#"<?xml version="1.0"?><ncx><navMap>{nav_points}</navMap></ncx>"#).as_bytes()).unwrap();
            z.finish().unwrap();
        }
        buf
    }

    #[test]
    fn optimize_comic_epub_to_pdf_produces_one_page_per_image_with_toc() {
        // is_comic 要求 >=20 张图——两章分别 10/11 张图，凑够 21 张触发漫画判定。
        let c1_imgs: Vec<String> = (1..=10).map(|i| format!("i{i}.jpg")).collect();
        let c2_imgs: Vec<String> = (11..=21).map(|i| format!("i{i}.jpg")).collect();
        let c1_refs: Vec<&str> = c1_imgs.iter().map(|s| s.as_str()).collect();
        let c2_refs: Vec<&str> = c2_imgs.iter().map(|s| s.as_str()).collect();
        let epub = build_test_epub(&[("c1.xhtml", &c1_refs), ("c2.xhtml", &c2_refs)]);
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("test.epub");
        let output = dir.path().join("test.pdf");
        std::fs::write(&input, &epub).unwrap();

        let mut calls = Vec::new();
        let rep = optimize_comic_epub_to_pdf_streaming(&input, &output, |d, t| calls.push((d, t))).unwrap();
        assert_eq!(rep.pages, 21, "21 张图应展开成 21 个 PDF 页");
        assert_eq!(calls.last(), Some(&(21, 21)));

        let mut reader = pdfwrite::PdfFileReader::open(&output).unwrap();
        assert_eq!(reader.page_count().unwrap(), 21);
        let titles = reader.outline_titles().unwrap();
        assert_eq!(titles, vec![(0, "第0章".to_string()), (10, "第1章".to_string())], "标题应落在各章第一张图对应的页码上");
    }

    fn write_comic(dir: &std::path::Path, text: &str, with_ncx: bool) -> (std::path::PathBuf, std::path::PathBuf) {
        let c1_imgs: Vec<String> = (1..=30).map(|i| format!("i{i}.jpg")).collect();
        let c2_imgs: Vec<String> = (31..=45).map(|i| format!("i{i}.jpg")).collect();
        let c1: Vec<&str> = c1_imgs.iter().map(|s| s.as_str()).collect();
        let c2: Vec<&str> = c2_imgs.iter().map(|s| s.as_str()).collect();
        let input = dir.join("t.epub");
        std::fs::write(&input, build_test_epub_opts(&[("c1.xhtml", &c1), ("c2.xhtml", &c2)], text, with_ncx)).unwrap();
        (input, dir.join("t.pdf"))
    }

    #[test]
    fn refuses_to_convert_comic_containing_any_body_text() {
        // 不允许变动书籍内容：PDF 一图一页，夹带的文字（版权/台词）会被丢掉——必须拒绝，不静默丢字。
        let dir = tempfile::tempdir().unwrap();
        let (input, output) = write_comic(dir.path(), "版权信息", true);
        let err = optimize_comic_epub_to_pdf_streaming(&input, &output, |_, _| {}).unwrap_err();
        assert!(err.contains("正文文字"), "错误应说明原因: {err}");
        assert!(!output.exists(), "拒绝时不该产出任何文件");
        assert!(!crate::comic_detect::is_text_free_comic_epub_file(&input));
        assert!(crate::comic_detect::is_comic_epub_file(&input), "它仍是漫画，只是不能转 PDF");
    }

    #[test]
    fn text_free_comic_is_convertible() {
        let dir = tempfile::tempdir().unwrap();
        let (input, _) = write_comic(dir.path(), "", true);
        assert!(crate::comic_detect::is_text_free_comic_epub_file(&input));
    }

    #[test]
    fn comic_without_source_toc_gets_page_range_bookmarks_not_a_single_one() {
        let dir = tempfile::tempdir().unwrap();
        let (input, output) = write_comic(dir.path(), "", false);
        optimize_comic_epub_to_pdf_streaming(&input, &output, |_, _| {}).unwrap();
        let titles = pdfwrite::PdfFileReader::open(&output).unwrap().outline_titles().unwrap();
        assert_eq!(titles, vec![(0, "第 1–20 页".to_string()), (20, "第 21–40 页".to_string()), (40, "第 41–45 页".to_string())]);
    }

    #[test]
    fn page_chunk_titles_boundaries() {
        assert_eq!(page_chunk_titles(1), vec![(0, "第 1–1 页".to_string())]);
        assert_eq!(page_chunk_titles(20).len(), 1);
        assert_eq!(page_chunk_titles(21).last(), Some(&(20, "第 21–21 页".to_string())));
    }

    #[test]
    fn optimize_comic_epub_to_pdf_rejects_non_comic() {
        // 图太少、判不成漫画（is_comic 需要 >=20 张图）。
        let epub = build_test_epub(&[("c1.xhtml", &["i1.jpg"])]);
        let dir = tempfile::tempdir().unwrap();
        let input = dir.path().join("test.epub");
        let output = dir.path().join("test.pdf");
        std::fs::write(&input, &epub).unwrap();
        assert!(optimize_comic_epub_to_pdf_streaming(&input, &output, |_, _| {}).is_err());
    }

    #[test]
    fn split_comic_pdf_returns_none_when_within_budget() {
        let images: Vec<PdfImage> = (0..5).map(|_| pdfwrite::image_from_bytes(&one_px_jpeg()).unwrap()).collect();
        let pdf = pdfwrite::images_to_pdf_with_toc(&images, &[(0, "卷一".into())]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("small.pdf");
        std::fs::write(&path, &pdf).unwrap();
        assert!(deliver_split_pdf_streaming(&path, 10_000_000, |_, _, _, _| Ok(())).unwrap().is_none());
    }

    #[test]
    fn split_comic_pdf_returns_none_for_pdf_without_outline() {
        let images: Vec<PdfImage> = (0..5).map(|_| pdfwrite::image_from_bytes(&one_px_jpeg()).unwrap()).collect();
        let pdf = pdfwrite::images_to_pdf_with_toc(&images, &[]).unwrap(); // 无书签 = 不是我们自己的漫画 PDF
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plain.pdf");
        std::fs::write(&path, &pdf).unwrap();
        assert!(deliver_split_pdf_streaming(&path, 1, |_, _, _, _| Ok(())).unwrap().is_none());
    }

    #[test]
    fn split_comic_pdf_splits_by_volume_boundary_when_oversized() {
        // 两卷各 3 张图；先用 PdfFileReader 量出单页真实的对象字节范围，budget 卡在刚好装不下
        // 6 页但装得下 3 页（不再靠猜 JPEG 骨架长度换算，直接用生产代码同一套量法）。
        let jpeg = one_px_jpeg();
        let images: Vec<PdfImage> = (0..6).map(|_| pdfwrite::image_from_bytes(&jpeg).unwrap()).collect();
        let titles = vec![(0, "卷一".to_string()), (3, "卷二".to_string())];
        let pdf = pdfwrite::images_to_pdf_with_toc(&images, &titles).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("big.pdf");
        std::fs::write(&path, &pdf).unwrap();
        let per_page = pdfwrite::PdfFileReader::open(&path).unwrap().page_byte_span_len(0);

        let mut collected: Vec<(String, Vec<u8>)> = Vec::new();
        let budget = per_page * 4; // 装得下一卷（3页），装不下两卷（6页）
        let outcome = deliver_split_pdf_streaming(&path, budget, |name, bytes, _idx, _total| {
            collected.push((name.to_string(), bytes.to_vec()));
            Ok(())
        })
        .unwrap()
        .unwrap();
        assert_eq!(outcome.delivered.len(), 2, "应该按卷边界切成两份: {outcome:?}");
        assert!(outcome.failed.is_empty(), "{outcome:?}");
        assert_eq!(collected.len(), 2);
        for (name, bytes) in &collected {
            assert!(name.contains("卷"));
            assert_eq!(pdfwrite::page_count(bytes).unwrap(), 3);
        }
    }

    /// 同长度改写书签的 `/Dest [页对象号` 字节（不动 xref 偏移），造"损坏/陌生"的书签。
    fn write_pdf_with_patched_dest(dir: &std::path::Path, from: &[u8], to: &[u8]) -> std::path::PathBuf {
        let images: Vec<PdfImage> = (0..4).map(|_| pdfwrite::image_from_bytes(&one_px_jpeg()).unwrap()).collect();
        let mut pdf = pdfwrite::images_to_pdf_with_toc(&images, &[(0, "卷一".into()), (3, "卷二".into())]).unwrap();
        let at = pdf.windows(from.len()).position(|w| w == from).expect("书签 /Dest 应在");
        pdf[at..at + from.len()].copy_from_slice(to);
        let path = dir.join("patched.pdf");
        std::fs::write(&path, &pdf).unwrap();
        path
    }

    /// 书签指向不存在的页（第 33 页，全书 4 页）：此前当切分边界，`sizes[32..4]` 直接 panic 摔掉投递线程。
    #[test]
    fn split_comic_pdf_ignores_bookmark_beyond_last_page_instead_of_panicking() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_pdf_with_patched_dest(dir.path(), b"/Dest [12 0 R", b"/Dest [99 0 R");
        let outcome = deliver_split_pdf_streaming(&path, 1, |_, _, _, _| Ok(())).unwrap().expect("还有一条有效书签，照常拆");
        assert!(outcome.delivered.iter().chain(&outcome.failed).all(|t| t.contains("卷一")), "越界书签不该成为一卷: {outcome:?}");
    }

    /// 书签指向对象号 <3（不是页对象）：此前 `(id-3)/3` 减法溢出。现在整份书签判不可用 → 不拆（`None`）。
    #[test]
    fn split_comic_pdf_with_non_page_bookmark_target_is_not_split() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_pdf_with_patched_dest(dir.path(), b"/Dest [12 0 R", b"/Dest [01 0 R");
        assert!(pdfwrite::PdfFileReader::open(&path).unwrap().outline_titles().is_err());
        assert!(deliver_split_pdf_streaming(&path, 1, |_, _, _, _| Ok(())).unwrap().is_none());
    }
}
