//! PDF 入库单测（原 `pdf_ingest.rs` 内联的 `mod tests`）。
use super::*;

const SAMPLE_PDF: &[u8] = include_bytes!("../../tests/fixtures/sample.pdf");

fn char_at(ch: char, x: f64, y: f64, font_size: f64, line: usize) -> PositionedChar {
    PositionedChar { ch, x, y, font_size, line, color: None, seq: 0 }
}

// ---- 公式区域探测 ----

#[test]
fn formula_detection_single_inline_run() {
    let chars = vec![
        char_at('x', 0.0, 100.0, 10.0, 0),
        char_at('=', 10.0, 100.0, 10.0, 0),
        char_at('π', 20.0, 100.0, 10.0, 0), // 命中数学符号区块
    ];
    let blocks = detect_formula_regions(&chars);
    assert_eq!(blocks.len(), 1, "单行内联公式应该产出一个公式块");
}

#[test]
fn formula_detection_multiline_merges_into_one_block() {
    // 模拟分数/矩阵：多行都出现数学符号，行号相邻。
    let chars = vec![
        char_at('∫', 0.0, 200.0, 10.0, 0),
        char_at('0', 5.0, 200.0, 10.0, 0),
        char_at('√', 0.0, 190.0, 10.0, 1),
        char_at('π', 5.0, 190.0, 10.0, 1),
    ];
    let blocks = detect_formula_regions(&chars);
    assert_eq!(blocks.len(), 1, "垂直相邻的公式行应该合并成一个块，不是两个");
}

#[test]
fn formula_detection_plain_prose_has_zero_blocks() {
    let chars = vec![
        char_at('h', 0.0, 100.0, 10.0, 0),
        char_at('e', 5.0, 100.0, 10.0, 0),
        char_at('l', 10.0, 100.0, 10.0, 0),
        char_at('l', 15.0, 100.0, 10.0, 0),
        char_at('o', 20.0, 100.0, 10.0, 0),
    ];
    assert!(detect_formula_regions(&chars).is_empty(), "纯正文字符不该产出任何公式块");
}

#[test]
fn formula_detection_empty_input() {
    assert!(detect_formula_regions(&[]).is_empty());
}

#[test]
fn formula_detection_far_lines_not_merged() {
    let chars = vec![
        char_at('π', 0.0, 500.0, 10.0, 0),
        char_at('α', 0.0, 100.0, 10.0, 10), // 行号差很远，不该合并
    ];
    let blocks = detect_formula_regions(&chars);
    assert_eq!(blocks.len(), 2, "相隔很远的两处公式不该被误合并成一个块");
}

// ---- 字号识别标题 ----

#[test]
fn heading_detection_picks_larger_font_lines() {
    let mut page = Vec::new();
    // 正文：字号 10，多次出现占多数。
    for i in 0..20 {
        page.push(char_at('a', i as f64, 100.0, 10.0, 1));
    }
    // 标题行：字号 16（1.6 倍正文），字号更大。
    for i in 0..5 {
        page.push(char_at('T', i as f64, 200.0, 16.0, 0));
    }
    let headings = detect_headings_by_font_size(&[PageContent { chars: page, images: Vec::new() }]);
    assert_eq!(headings.len(), 1);
    assert_eq!(headings[0].level, 1);
}

#[test]
fn heading_detection_zero_candidates_when_uniform_size() {
    let page: Vec<PositionedChar> = (0..20).map(|i| char_at('a', i as f64, 100.0, 10.0, 0)).collect();
    assert!(detect_headings_by_font_size(&[PageContent { chars: page, images: Vec::new() }]).is_empty(), "字号完全统一时不该识别出标题");
}

#[test]
fn heading_detection_ranks_levels_by_size() {
    let mut p1 = Vec::new();
    for i in 0..10 {
        p1.push(char_at('a', i as f64, 100.0, 10.0, 1));
    }
    for i in 0..3 {
        p1.push(char_at('T', i as f64, 200.0, 20.0, 0)); // 最大字号 → level 1
    }
    let mut p2 = Vec::new();
    for i in 0..10 {
        p2.push(char_at('a', i as f64, 100.0, 10.0, 1));
    }
    for i in 0..3 {
        p2.push(char_at('S', i as f64, 200.0, 14.0, 0)); // 次大字号 → level 2
    }
    let headings = detect_headings_by_font_size(&[PageContent { chars: p1, images: Vec::new() }, PageContent { chars: p2, images: Vec::new() }]);
    assert_eq!(headings.len(), 2);
    assert_eq!(headings[0].level, 1);
    assert_eq!(headings[1].level, 2);
}

// ---- 按行分组的等价性（2026-09-25 审计：公式/标题两处由"逐行号全页扫描"改为一遍分组） ----

/// 改动前的公式探测（逐行号 `filter`），只作差分参照。
fn formula_regions_reference(chars: &[PositionedChar]) -> Vec<BBox> {
    if chars.is_empty() {
        return Vec::new();
    }
    let max_line = chars.iter().map(|c| c.line).max().unwrap_or(0);
    let mut formula_lines: Vec<(usize, BBox, f64)> = Vec::new();
    for line_no in 0..=max_line {
        let line_chars: Vec<&PositionedChar> = chars.iter().filter(|c| c.line == line_no).collect();
        if line_chars.is_empty() || !line_is_formula(&line_chars) {
            continue;
        }
        if let Some(bbox) = bbox_of(&line_chars) {
            formula_lines.push((line_no, bbox, avg_font_size(&line_chars)));
        }
    }
    if formula_lines.is_empty() {
        return Vec::new();
    }
    let mut blocks: Vec<BBox> = Vec::new();
    let (mut cur, mut cur_font, mut prev_line) = (formula_lines[0].1, formula_lines[0].2, formula_lines[0].0);
    for (line_no, bbox, font) in formula_lines.into_iter().skip(1) {
        let x_overlap = bbox.x0 <= cur.x1 + cur_font * 4.0 && bbox.x1 >= cur.x0 - cur_font * 4.0;
        if line_no.saturating_sub(prev_line) <= 2 && x_overlap {
            cur = BBox { x0: cur.x0.min(bbox.x0), y0: cur.y0.min(bbox.y0), x1: cur.x1.max(bbox.x1), y1: cur.y1.max(bbox.y1) };
            cur_font = cur_font.max(font);
        } else {
            blocks.push(cur);
            cur = bbox;
            cur_font = font;
        }
        prev_line = line_no;
    }
    blocks.push(cur);
    for b in blocks.iter_mut() {
        let pad = cur_font.max(4.0) * 0.08;
        *b = BBox { x0: b.x0 - pad, y0: b.y0 - pad, x1: b.x1 + pad, y1: b.y1 + pad };
    }
    blocks
}

/// 改动前的标题识别（逐行号 `filter`），只作差分参照。
fn headings_reference(pages: &[PageContent]) -> Vec<Heading> {
    let mut headings = Vec::new();
    let mut candidate_sizes: Vec<i64> = Vec::new();
    let mut per_page_body = Vec::new();
    for page in pages {
        let chars = &page.chars;
        let body = body_font_size(chars);
        per_page_body.push(body);
        let max_line = chars.iter().map(|c| c.line).max().unwrap_or(0);
        for line_no in 0..=max_line {
            let lc: Vec<&PositionedChar> = chars.iter().filter(|c| c.line == line_no && !c.ch.is_whitespace()).collect();
            if !lc.is_empty() && avg_font_size(&lc) >= body * HEADING_SIZE_RATIO {
                candidate_sizes.push((avg_font_size(&lc) * 2.0).round() as i64);
            }
        }
    }
    if candidate_sizes.is_empty() {
        return headings;
    }
    candidate_sizes.sort_unstable();
    candidate_sizes.dedup();
    candidate_sizes.reverse();
    let level_of = |size: f64| -> i64 {
        let bucket = (size * 2.0).round() as i64;
        (candidate_sizes.iter().position(|&s| s == bucket).unwrap_or(candidate_sizes.len() - 1) as i64 + 1).min(3)
    };
    for (page_idx, page) in pages.iter().enumerate() {
        let chars = &page.chars;
        let body = per_page_body[page_idx];
        let max_line = chars.iter().map(|c| c.line).max().unwrap_or(0);
        let vis = |ln: usize| -> Vec<&PositionedChar> { chars.iter().filter(|c| c.line == ln && !c.ch.is_whitespace()).collect() };
        let title_line = |ln: usize| -> String { chars.iter().filter(|c| c.line == ln).map(|c| c.ch).collect::<String>().trim().to_string() };
        let mut i = 0usize;
        while i <= max_line {
            let lc = vis(i);
            if lc.is_empty() || avg_font_size(&lc) < body * HEADING_SIZE_RATIO {
                i += 1;
                continue;
            }
            let level = level_of(avg_font_size(&lc));
            let mut title = title_line(i);
            let mut j = i + 1;
            while j <= max_line {
                let nc = vis(j);
                if nc.is_empty() || avg_font_size(&nc) < body * HEADING_SIZE_RATIO || level_of(avg_font_size(&nc)) != level {
                    break;
                }
                title.push(' ');
                title.push_str(&title_line(j));
                j += 1;
            }
            headings.push(Heading { page: page_idx, line: i, level, title: title.trim().to_string() });
            i = j;
        }
    }
    headings
}

/// 确定性伪随机页：行号有跳号、有乱序（`line` 不保证单调也要对）、有纯空白行、有多档字号与公式字符。
fn pseudo_random_page(seed: u64, n: usize) -> Vec<PositionedChar> {
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    let pool = ['a', 'b', ' ', '中', '∫', 'π', 'T', '1', '\u{0}'];
    let sizes = [10.0, 10.0, 10.0, 12.5, 16.0, 20.0];
    (0..n)
        .map(|k| {
            let r = next();
            let line = if r % 11 == 0 { (r as usize >> 8) % 40 } else { k / 7 + (r as usize >> 20) % 2 };
            let ch = pool[(r >> 32) as usize % pool.len()];
            let fs = sizes[(r >> 40) as usize % sizes.len()];
            char_at(ch, (r >> 16) as f64 % 400.0, 700.0 - line as f64 * 12.0, fs, line)
        })
        .collect()
}

#[test]
fn grouped_line_scan_matches_old_per_line_scan_exactly() {
    let (mut n_heads, mut n_blocks) = (0usize, 0usize);
    for seed in 0..60u64 {
        let pages: Vec<PageContent> = (0..3).map(|p| PageContent { chars: pseudo_random_page(seed * 7 + p, 30 + (seed as usize * 13) % 300), images: Vec::new() }).collect();
        for p in &pages {
            let got = detect_formula_regions(&p.chars);
            n_blocks += got.len();
            assert_eq!(got, formula_regions_reference(&p.chars), "seed {seed}");
        }
        let got = detect_headings_by_font_size(&pages);
        n_heads += got.len();
        assert_eq!(got, headings_reference(&pages), "seed {seed}");
    }
    assert!(n_heads > 20 && n_blocks > 20, "样本要真覆盖到标题与公式：{n_heads} / {n_blocks}");
    let real = extract_positioned_text(SAMPLE_PDF).unwrap();
    for p in &real {
        assert_eq!(detect_formula_regions(&p.chars), formula_regions_reference(&p.chars));
    }
    assert_eq!(detect_headings_by_font_size(&real), headings_reference(&real));
}

// ---- 分类（拿真实 pdflatex 样本核对，不是拍脑袋） ----

#[test]
fn classify_real_pdflatex_sample_is_text_layer() {
    assert_eq!(classify_pdf_bytes(SAMPLE_PDF), PdfKind::TextLayer, "真实 pdflatex 文字样本应该判定为有文字层");
}

#[test]
fn classify_one_image_per_page_pdf_is_comic() {
    const RED_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    // `images_to_pdf` 的 MediaBox 直接等于图片像素尺寸（1px=1pt，见其文档注释），宽高比
    // 天然跟页面一致——不需要专门构造特定尺寸的图片，这条判据本来就该命中。
    let img = pdfwrite::image_from_bytes(RED_PNG).unwrap();
    let comic_pdf = pdfwrite::images_to_pdf(&[img.clone(), img.clone(), img]).unwrap();
    assert_eq!(classify_pdf_bytes(&comic_pdf), PdfKind::Comic);
}

#[test]
fn classify_unparseable_bytes_defaults_to_no_text_layer() {
    assert_eq!(classify_pdf_bytes(b"not a pdf"), PdfKind::NoTextLayer, "解析失败要退到保守默认（只裁边），不能默认转换");
}

// ---- 端到端：真实样本转 EPUB ----

#[test]
fn optimize_pdf_to_epub_real_sample_produces_chapters_and_resources() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sample.pdf");
    std::fs::write(&src, SAMPLE_PDF).unwrap();
    let (book, report, _color_css) = optimize_pdf_to_epub(&src, |_, _| {}).unwrap();
    assert_eq!(report.pages, 3, "样本是 3 页");
    assert!(report.chapters >= 2, "没有书签，应该靠字号识别出至少 2 个标题层级的章节，实际 {}", report.chapters);
    assert!(report.formula_blocks >= 1, "样本含多处公式，应该探测到至少一个公式块");
    assert!(!book.resources.is_empty(), "样本含嵌入图片+公式块渲染图，resources 不该是空的");
    let bytes = crate::epub::assemble(&mut { book }).unwrap();
    assert!(!bytes.is_empty());
}

/// 章节里每个 `<img src="...">` 引用，按 zip 内相对路径解析后必须真的指向压缩包里存在的条目——不只
/// 是"字符串里有 <img> 标签"这种表面检查。2026-09-23 真机踩过：`src="../images/x.png"` 在 `assemble`
/// 后压缩包实际结构里章节文件跟 `images/` 是同级（都在 `OEBPS/` 下），多打的这个 `../` 会把路径指到
/// 压缩包里根本不存在的位置，导致图片在 EPUB 里打包正确、标签也在，但 xochitl 真机渲染出来一张图都
/// 没有——`report.images`/`!book.resources.is_empty()` 这类"数量对不对"的检查完全测不出这类路径错位。
#[test]
fn embedded_image_src_paths_resolve_to_real_zip_entries() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sample.pdf");
    std::fs::write(&src, SAMPLE_PDF).unwrap();
    let (mut book, report, color_css) = optimize_pdf_to_epub(&src, |_, _| {}).unwrap();
    assert!(report.images >= 1, "样本至少含一张嵌入图片，前提不满足测试就没意义");
    let bytes = crate::epub::assemble_pdf_derived(&mut book, &color_css).unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let names: std::collections::HashSet<String> = (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();

    let img_src_re = regex::Regex::new(r#"<img\b[^>]*\bsrc="([^"]+)""#).unwrap();
    let chapter_names: Vec<String> = names.iter().filter(|n| n.starts_with("OEBPS/") && n.ends_with(".xhtml") && n.as_str() != "OEBPS/nav.xhtml").cloned().collect();
    assert!(!chapter_names.is_empty());
    let mut checked = 0;
    for chap_name in &chapter_names {
        let mut f = zip.by_name(chap_name).unwrap();
        let mut html = String::new();
        std::io::Read::read_to_string(&mut f, &mut html).unwrap();
        for cap in img_src_re.captures_iter(&html) {
            let src = &cap[1];
            let resolved = resolve_zip_relative(chap_name, src);
            assert!(names.contains(&resolved), "章节 {chap_name} 里 <img src=\"{src}\"> 解析成 {resolved}，压缩包里不存在这个条目。全部条目: {names:?}");
            checked += 1;
        }
    }
    assert!(checked >= 1, "至少要真的检查过一个 <img src>，不然这条测试测了个寂寞");
}

/// 极简 zip 内相对路径解析：`base` 是引用方文件在 zip 里的完整路径（如 "OEBPS/chap_0001.xhtml"），
/// `rel` 是它 html 里写的相对 href/src。只处理 `../`/`./`/普通段，够测试用，不追求通用 URL 解析器的完备性。
fn resolve_zip_relative(base: &str, rel: &str) -> String {
    let mut stack: Vec<&str> = base.rsplit_once('/').map(|(dir, _)| dir.split('/').collect()).unwrap_or_default();
    for seg in rel.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                stack.pop();
            }
            s => stack.push(s),
        }
    }
    stack.join("/")
}

/// 章节 HTML 的可见文字（去标签、去空白、还原 `&amp;` 等）。
fn visible(html: &str) -> String {
    let no_tags = regex::Regex::new(r"(?s)<[^>]*>").unwrap().replace_all(html, "");
    let t = no_tags.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"");
    t.chars().filter(|c| !c.is_whitespace()).collect()
}

/// **不允许变动书籍内容**的核心不变量：PDF 文字层提取出的每个字符，按顺序原样出现在 EPUB 文字流里，
/// 不多不少（此前公式外接框内字符被整体丢弃，吞掉了 identity / 整半句正文；标题还被重复输出一遍）。
#[test]
fn epub_text_flow_equals_pdf_text_layer_exactly() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sample.pdf");
    std::fs::write(&src, SAMPLE_PDF).unwrap();
    let expected: String = extract_positioned_text(SAMPLE_PDF)
        .unwrap()
        .iter()
        .flat_map(|p| p.chars.iter())
        .filter(|c| c.ch != '\u{0}')
        .map(|c| c.ch)
        .filter(|c| !c.is_whitespace())
        .collect();
    let (book, _, _) = optimize_pdf_to_epub(&src, |_, _| {}).unwrap();
    let got: String = book.chapters.iter().map(|c| visible(&c.html_body)).collect();
    assert_eq!(got, expected, "EPUB 文字流必须与 PDF 文字层逐字一致");
    // 具体回归点：紧贴公式的正文单词与整半句不能丢；标题只出现一次。
    assert!(got.contains("identity") && got.contains("quadraticformula"), "行内公式旁的单词被吞了");
    assert!(got.contains("Afinalshortsection") && got.contains("onemoreinlinesymbol"), "整半句正文被吞了");
    assert_eq!(got.matches("1Introduction").count(), 1, "章节标题不该重复输出");
}

// ---- 图片文件名后缀必须跟嗅探出的 media-type 一致（2026-09-23 真机踩坑：sample.pdf 的嵌入图片凑巧
//      走 FlateDecode/PNG，没覆盖过 DCTDecode/JPEG 这条分支，导致后缀写死 .png 的 bug 一直没被测出）----

#[test]
fn image_ext_and_media_type_detects_jpeg_by_soi_marker() {
    let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
    assert_eq!(image_ext_and_media_type(&jpeg), ("jpg", "image/jpeg"));
}

#[test]
fn image_ext_and_media_type_defaults_to_png_for_everything_else() {
    let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
    assert_eq!(image_ext_and_media_type(&png), ("png", "image/png"));
    assert_eq!(image_ext_and_media_type(&[]), ("png", "image/png"), "空字节/太短判不出来时不该 panic，落回默认值");
    assert_eq!(image_ext_and_media_type(&[0xFF]), ("png", "image/png"), "只有一个字节、够不成 SOI 魔数");
}

#[test]
fn promote_heading_upgrades_existing_paragraph_without_adding_text() {
    assert_eq!(promote_heading("1 Intro", "<p>1 Intro</p><p>body</p>".into()), "<h2>1 Intro</h2><p>body</p>");
    assert_eq!(promote_heading("1 Intro", "<p>1 Intro Some text</p>".into()), "<h2>1 Intro</h2><p>Some text</p>");
    // 章内找不到标题段落：不往正文里塞原书没有的字。
    assert_eq!(promote_heading("Missing", "<p>body</p>".into()), "<p>body</p>");
}

#[test]
fn looks_like_pdf_derived_epub_detects_marker() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("sample.pdf");
    std::fs::write(&src, SAMPLE_PDF).unwrap();
    let (mut book, _, _) = optimize_pdf_to_epub(&src, |_, _| {}).unwrap();
    let bytes = crate::epub::assemble(&mut book).unwrap();
    let out = dir.path().join("out.epub");
    std::fs::write(&out, &bytes).unwrap();
    assert!(looks_like_pdf_derived_epub(&out));
}

#[test]
fn looks_like_pdf_derived_epub_false_for_unrelated_zip() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("not_epub.zip");
    let file = std::fs::File::create(&p).unwrap();
    let mut zip = zip::ZipWriter::new(file);
    zip.start_file::<_, ()>("hello.txt", Default::default()).unwrap();
    std::io::Write::write_all(&mut zip, b"hi").unwrap();
    zip.finish().unwrap();
    assert!(!looks_like_pdf_derived_epub(&p));
}

// ---- 裁边路径 ----

#[test]
fn optimize_pdf_trim_only_comic_shaped_fixture_roundtrips() {
    // sample.pdf 是文字样本，不代表"裁边"路径的真实输入形状（裁边只服务一页一图的扫描件/
    // 漫画 PDF）——这里用既有的 `images_to_pdf`（漫画 EPUB→PDF 那条产线复用的写手，已经
    // 有自己的测试覆盖）现造一份"每页一张图"的合成 PDF，形状上才贴近这条路径真正会遇到的
    // 输入。
    const RED_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];
    let img = pdfwrite::image_from_bytes(RED_PNG).unwrap();
    let comic_pdf = pdfwrite::images_to_pdf(&[img.clone(), img.clone(), img]).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("comic.pdf");
    std::fs::write(&src, &comic_pdf).unwrap();
    let dst = dir.path().join("out.pdf");
    let report = optimize_pdf_trim_only(&src, &dst, |_, _| {}).unwrap();
    assert_eq!(report.pages, 3);
    assert!(pdfwrite::looks_like_own_bookconv_pdf(&dst), "裁边输出应该能被识别成自产 PDF");
    // 要有目录：源 PDF 没有书签 → 按页分段兜底，不是空的。
    let titles = pdfwrite::PdfFileReader::open(&dst).unwrap().outline_titles().unwrap();
    assert_eq!(titles, vec![(0, "第 1–3 页".to_string())]);
}

fn red_png_pdf(pages: usize, titles: &[(usize, String)]) -> Vec<u8> {
    // 与设备页面(954×1696)同宽高比的整页图，才满足"单张整页图"判据。
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(95, 169, image::Rgb([200, 30, 30])))
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let img = pdfwrite::image_from_bytes(&png).unwrap();
    let imgs: Vec<_> = (0..pages).map(|_| img.clone()).collect();
    pdfwrite::images_to_pdf_with_toc(&imgs, titles).unwrap()
}

#[test]
fn trim_only_preserves_original_bookmarks() {
    // 原书签必须保留（不许变动书籍内容 + 要有目录），页码原样。
    let titles = vec![(0, "第一卷".to_string()), (2, "第二卷".to_string())];
    let pdf = red_png_pdf(4, &titles);
    let dir = tempfile::tempdir().unwrap();
    let (src, dst) = (dir.path().join("a.pdf"), dir.path().join("o.pdf"));
    std::fs::write(&src, &pdf).unwrap();
    optimize_pdf_trim_only(&src, &dst, |_, _| {}).unwrap();
    assert_eq!(pdfwrite::PdfFileReader::open(&dst).unwrap().outline_titles().unwrap(), titles);
}

#[test]
fn trim_only_refuses_pdf_with_text_layer_and_leaves_it_untouched() {
    // 文字样本含大量文字：重写页面会丢文字层——必须拒绝，且不产出任何文件。
    let dir = tempfile::tempdir().unwrap();
    let (src, dst) = (dir.path().join("t.pdf"), dir.path().join("o.pdf"));
    std::fs::write(&src, SAMPLE_PDF).unwrap();
    let err = optimize_pdf_trim_only(&src, &dst, |_, _| {}).unwrap_err();
    assert!(err.contains("文字"), "应说明是因为文字层: {err}");
    assert!(!dst.exists());
}

// ---- 自研解释器：颜色 + 图片精确位置（2026-09-23，用户拍板"颜色/图片位置不能更改"后新增） ----
// 手搓最小合法 PDF（1 页，base14 Helvetica 不用嵌入字体/ToUnicode，可选带一张 1×1 RGB 图片
// XObject `/Im1`），content stream 由调用方给——只测新功能要用到的最小子集，不追求覆盖 PDF
// 规范全貌，跟 `pdfwrite.rs`（只会写纯图片 PDF，没有文字算子）互补。

fn build_synthetic_pdf(content: &str, with_image: bool) -> Vec<u8> {
    build_synthetic_pdf_pages(&[SynPage { content, with_image, links: vec![] }])
}

enum SynLink<'a> {
    Uri(&'a str),
    /// 目标页（0 起）。
    Page(usize),
}

struct SynPage<'a> {
    content: &'a str,
    with_image: bool,
    links: Vec<([f64; 4], SynLink<'a>)>,
}

fn build_synthetic_pdf_pages(pages: &[SynPage]) -> Vec<u8> {
    use lopdf::{dictionary, Document, Object, Stream, StringFormat};
    let mut doc = Document::with_version("1.5");
    let mut next_id = 1u32;
    let mut alloc = |doc: &mut Document, obj: Object| -> lopdf::ObjectId {
        let id = (next_id, 0);
        next_id += 1;
        doc.objects.insert(id, obj);
        id
    };
    let font_id = alloc(
        &mut doc,
        Object::Dictionary(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Helvetica",
            "Encoding" => "WinAnsiEncoding",
        }),
    );
    // 页对象 id 先全部占位：链接的书内跳转要引用别的页。
    let page_ids: Vec<lopdf::ObjectId> = pages.iter().map(|_| alloc(&mut doc, Object::Dictionary(lopdf::Dictionary::new()))).collect();
    let pages_id = alloc(
        &mut doc,
        Object::Dictionary(dictionary! {
            "Type" => "Pages",
            "Kids" => Object::Array(page_ids.iter().map(|id| Object::Reference(*id)).collect()),
            "Count" => pages.len() as i64,
        }),
    );
    for (pi, pg) in pages.iter().enumerate() {
        let mut resources = dictionary! {
            "Font" => Object::Dictionary(dictionary! { "F1" => Object::Reference(font_id) }),
        };
        if pg.with_image {
            // 1×1 纯红 RGB 像素，8bpc，不压缩（省掉 Filter）——`decode_pdf_image_to_bytes` 的
            // "filters 为空就当裸像素走 PNG 重编码"分支正好吃这种最简单的形状。
            let img_id = alloc(
                &mut doc,
                Object::Stream(Stream::new(
                    dictionary! {
                        "Type" => "XObject",
                        "Subtype" => "Image",
                        "Width" => 1,
                        "Height" => 1,
                        "ColorSpace" => "DeviceRGB",
                        "BitsPerComponent" => 8,
                    },
                    vec![0xFFu8, 0x00, 0x00],
                )),
            );
            resources.set("XObject", Object::Dictionary(dictionary! { "Im1" => Object::Reference(img_id) }));
        }
        let content_id = alloc(&mut doc, Object::Stream(Stream::new(lopdf::Dictionary::new(), pg.content.as_bytes().to_vec())));
        let annots: Vec<Object> = pg
            .links
            .iter()
            .map(|(r, t)| {
                let mut a = dictionary! {
                    "Type" => "Annot",
                    "Subtype" => "Link",
                    "Rect" => Object::Array(r.iter().map(|v| Object::Real(*v as f32)).collect()),
                };
                match t {
                    SynLink::Uri(u) => a.set("A", Object::Dictionary(dictionary! { "S" => "URI", "URI" => Object::String(u.as_bytes().to_vec(), StringFormat::Literal) })),
                    SynLink::Page(p) => a.set("Dest", Object::Array(vec![Object::Reference(page_ids[*p]), "XYZ".into(), Object::Integer(0), Object::Integer(792), Object::Integer(0)])),
                }
                Object::Dictionary(a)
            })
            .collect();
        let mut page = dictionary! {
            "Type" => "Page",
            "Parent" => Object::Reference(pages_id),
            "MediaBox" => Object::Array(vec![Object::Integer(0), Object::Integer(0), Object::Integer(612), Object::Integer(792)]),
            "Resources" => Object::Dictionary(resources),
            "Contents" => Object::Reference(content_id),
        };
        if !annots.is_empty() {
            page.set("Annots", Object::Array(annots));
        }
        doc.objects.insert(page_ids[pi], Object::Dictionary(page));
    }
    let catalog_id = alloc(
        &mut doc,
        Object::Dictionary(dictionary! {
            "Type" => "Catalog",
            "Pages" => Object::Reference(pages_id),
        }),
    );
    doc.trailer.set("Root", Object::Reference(catalog_id));
    doc.max_id = next_id - 1;
    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();
    buf
}

fn convert_synthetic(pdf: &[u8]) -> (crate::epub::Book, PdfToEpubReport, String) {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("syn.pdf");
    std::fs::write(&src, pdf).unwrap();
    optimize_pdf_to_epub(&src, |_, _| {}).unwrap()
}

fn joined_body(book: &crate::epub::Book) -> String {
    book.chapters.iter().map(|c| c.html_body.clone()).collect::<Vec<_>>().join("")
}

/// 黑→红→黑：红色文字要包 `cj-cN` span、黑色（含默认色）不包——验证新补的 `g`/`rg`/`k` 颜色算子
/// 真的生效（此前 `pdf-extract` 上游对这三个算子只 `dlog!` 不处理，fill_color 从头到尾是空的）。
#[test]
fn color_operators_produce_span_only_for_non_black_text() {
    let content = "BT /F1 12 Tf 100 700 Td (Black) Tj ET\n1 0 0 rg\nBT /F1 12 Tf 100 680 Td (Red) Tj ET\n0 g\nBT /F1 12 Tf 100 660 Td (BlackAgain) Tj ET\n";
    let pdf = build_synthetic_pdf(content, false);
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("color.pdf");
    std::fs::write(&src, &pdf).unwrap();
    let (book, _report, color_css) = optimize_pdf_to_epub(&src, |_, _| {}).unwrap();
    let body = book.chapters.iter().map(|c| c.html_body.clone()).collect::<Vec<_>>().join("");
    assert!(body.contains("Red"), "红色文字必须原样出现在正文里");
    let red_wrapped = body.contains("<span class=\"cj-c0\">Red</span>") || body.contains("<span class=\"cj-c0\">Red");
    assert!(red_wrapped, "红色文字应该被包进颜色 span，正文: {body}");
    assert!(!body.contains("<span class=\"cj-c0\">Black<"), "黑色文字不该被包进颜色 span，正文: {body}");
    assert!(!body.contains("<span class=\"cj-c0\">BlackAgain<"), "第二段黑色文字不该被包进颜色 span（验证 g 算子能把颜色从红切回黑），正文: {body}");
    assert!(color_css.contains(".cj-c0{color:#ff0000;}"), "颜色 CSS 应该含红色规则，实际: {color_css}");
}

/// 图片事件落在正确的文档顺序位置（不是"统一堆段末/页末"）——内容流是 文字A → 图片 → 文字B，
/// 产出的正文里 `<img` 必须夹在两段文字之间，不能被挪到最后。
#[test]
fn image_do_operator_lands_between_surrounding_text_not_at_page_end() {
    let content = "BT /F1 12 Tf 100 700 Td (Before) Tj ET\nq 50 0 0 50 100 600 cm /Im1 Do Q\nBT /F1 12 Tf 100 550 Td (After) Tj ET\n";
    let pdf = build_synthetic_pdf(content, true);
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("img_order.pdf");
    std::fs::write(&src, &pdf).unwrap();
    let (book, report, _color_css) = optimize_pdf_to_epub(&src, |_, _| {}).unwrap();
    assert_eq!(report.images, 1, "应该识别出这一张图片");
    let body = book.chapters.iter().map(|c| c.html_body.clone()).collect::<Vec<_>>().join("");
    let before_pos = body.find("Before").expect("Before 应该在正文里");
    let img_pos = body.find("<img").expect("图片应该出现在正文里");
    let after_pos = body.find("After").expect("After 应该在正文里");
    assert!(before_pos < img_pos && img_pos < after_pos, "图片必须夹在 Before/After 两段文字之间，实际顺序错乱，正文: {body}");
}

/// Word 导出的 PDF 每页先画完所有文字、最后才画图片：图片必须按**视觉位置**（上沿以上最近一行文字
/// 之后）落位，不能按绘制顺序堆到页尾（2026-09-23 真机《移动互联软件安装使用手册》第 1 页二维码）。
#[test]
fn image_painted_after_all_text_still_lands_at_visual_position() {
    let content = "BT /F1 12 Tf 100 700 Td (Before) Tj ET\nBT /F1 12 Tf 100 550 Td (After) Tj ET\nq 50 0 0 50 100 600 cm /Im1 Do Q\n";
    let (book, report, _) = convert_synthetic(&build_synthetic_pdf(content, true));
    assert_eq!(report.images, 1);
    let body = joined_body(&book);
    let (b, i, a) = (body.find("Before").unwrap(), body.find("<img").unwrap(), body.find("After").unwrap());
    assert!(b < i && i < a, "图片视觉上夹在 Before/After 之间，正文顺序应一致: {body}");
    assert!(!body.contains("<p><p>") && !body.contains("<p></p>"), "图片插入不能产出嵌套/空段落: {body}");
}

/// 图片视觉上在全页文字之上（第 4 页截图在上、说明文字在页底那种）：排在页首。
#[test]
fn image_above_all_text_goes_first_even_if_painted_last() {
    let content = "BT /F1 12 Tf 100 94 Td (Caption) Tj ET\nq 150 0 0 300 90 400 cm /Im1 Do Q\n";
    let body = joined_body(&convert_synthetic(&build_synthetic_pdf(content, true)).0);
    assert!(body.find("<img").unwrap() < body.find("Caption").unwrap(), "图片在文字上方，应排在前面: {body}");
    assert!(body.starts_with("<p><img"), "页首直接是图片段落，不能有 <p><p> 嵌套: {body}");
}

/// 逐字定位（calibre 每个字形各一次 Td，每个字 `line` 都不同）同一基线上不能补空格——此前一律补，
/// 《T.E.双语》标题被拆成 "C a n  t h e"（2026-09-23 真机，全书约 500 处）。
#[test]
fn per_glyph_positioning_on_same_baseline_adds_no_spaces() {
    let content = "BT /F1 12 Tf 100 700 Td (C) Tj 8.5 0 Td (a) Tj 6.6 0 Td (n) Tj ET\n";
    let body = joined_body(&convert_synthetic(&build_synthetic_pdf(content, false)).0);
    assert!(body.contains("Can"), "同一基线逐字定位应拼回 Can，实际: {body}");
}

/// 段内折行接续：行尾连字符后不补空格（复合词/网址在行尾断开）；普通折行仍补一个空格。
#[test]
fn line_wrap_after_hyphen_joins_without_space() {
    let content = "BT /F1 12 Tf 100 700 Td (artificial-) Tj 0 -14 Td (intelligence and) Tj 0 -14 Td (more) Tj ET\n";
    let body = joined_body(&convert_synthetic(&build_synthetic_pdf(content, false)).0);
    assert!(body.contains("artificial-intelligence and more"), "实际: {body}");
}

#[test]
fn partition_chapters_backward_bookmarks_do_not_duplicate_pages() {
    // 《T.E.双语》形状：一级栏目书签都指回第 2 页（下标 1）目录页，二级文章书签才指正文。
    let starts = [1, 4, 12, 18, 1, 21, 29];
    let r = partition_chapters(&starts, 40);
    assert_eq!(r, vec![0..4, 4..12, 12..18, 18..21, 21..21, 21..29, 29..40]);
    // 每页恰好进一章。
    let mut seen = vec![0; 40];
    for rg in &r {
        for p in rg.clone() {
            seen[p] += 1;
        }
    }
    assert!(seen.iter().all(|&n| n == 1), "每页恰好出现一次: {seen:?}");
}

#[test]
fn partition_chapters_same_start_gives_content_to_last_and_leading_pages_to_first() {
    assert_eq!(partition_chapters(&[3, 3, 7], 10), vec![0..3, 3..7, 7..10]);
    assert_eq!(partition_chapters(&[2, 2, 2], 5), vec![0..2, 2..2, 2..5]);
    assert_eq!(partition_chapters(&[50], 5), vec![0..5], "越界书签夹到末页，整本仍归这一章");
    assert!(partition_chapters(&[], 5).is_empty());
}

/// 外链 URI：链接矩形里的文字包进 `<a href>`，颜色 span 在 `<a>` 里层。
#[test]
fn uri_link_annotation_wraps_text_in_anchor() {
    let content = "BT /F1 12 Tf 100 700 Td (See ) Tj ET\n0 0 1 rg\nBT /F1 12 Tf 130 700 Td (site) Tj ET\n";
    let pdf = build_synthetic_pdf_pages(&[SynPage { content, with_image: false, links: vec![([128.0, 695.0, 160.0, 712.0], SynLink::Uri("https://example.com/a?b=1&c=2"))] }]);
    let (book, _, css) = convert_synthetic(&pdf);
    let body = joined_body(&book);
    assert!(body.contains("<a href=\"https://example.com/a?b=1&amp;c=2\"><span class=\"cj-c0\">site</span></a>"), "实际: {body}");
    assert!(!body.contains("<a href=\"https://example.com/a?b=1&amp;c=2\">See"), "矩形外的文字不能进链接: {body}");
    assert!(css.contains("#0000ff"));
}

/// 书内跳转：目标页首有锚点，同章写裸 `#pdf-pN`；章节切开后跨章改成 `chap_xxxx.xhtml#pdf-pN`。
#[test]
fn goto_link_targets_page_anchor_same_chapter_and_cross_chapter() {
    let p0 = SynPage { content: "BT /F1 12 Tf 100 700 Td (Go to two) Tj ET\n", with_image: false, links: vec![([98.0, 695.0, 170.0, 712.0], SynLink::Page(1))] };
    let p1 = SynPage { content: "BT /F1 12 Tf 100 700 Td (Page two) Tj ET\n", with_image: false, links: vec![] };
    let (book, _, _) = convert_synthetic(&build_synthetic_pdf_pages(&[p0, p1]));
    let body = joined_body(&book);
    assert_eq!(book.chapters.len(), 1, "无书签无标题的 2 页落进同一个兜底分块");
    assert!(body.contains("<a href=\"#pdf-p2\">Go to two</a>"), "同章裸锚点: {body}");
    assert!(body.contains("<p id=\"pdf-p2\">Page two</p>"), "目标页首段落带锚点 id（空 <a id> xochitl 不登记跳转目标）: {body}");
    assert!(!body.contains("></a>"), "不能再出现空锚点元素: {body}");

    let mut chapters = vec![
        Chapter { title: "A".into(), html_body: "<p><a href=\"#pdf-p2\">x</a><a href=\"#pdf-p1\">y</a></p>".into(), level: 1 },
        Chapter { title: "B".into(), html_body: "<p id=\"pdf-p2\">z</p>".into(), level: 1 },
    ];
    relink_page_anchors(&mut chapters, &[0, 1]);
    assert_eq!(chapters[0].html_body, "<p><a href=\"chap_0002.xhtml#pdf-p2\">x</a><a href=\"#pdf-p1\">y</a></p>");
}

/// PDF 文本字符串三种编码（PDF 32000-1 §7.9.2.2）。UTF-16BE 那条就是《T.E.双语》标题的真实字节形状
/// ——此前按 UTF-8 解出一串 `\0`，OPF 不合法，xochitl 整本只渲染 1 页（2026-09-23 真机）。
#[test]
fn decode_pdf_text_string_handles_utf16be_utf8_bom_and_pdfdoc() {
    let mut utf16 = vec![0xFE, 0xFF];
    for u in "《经济学人》。2026 年".encode_utf16() {
        utf16.extend_from_slice(&u.to_be_bytes());
    }
    assert_eq!(decode_pdf_text_string(&utf16), "《经济学人》。2026 年");
    assert_eq!(decode_pdf_text_string(&[0xEF, 0xBB, 0xBF, 0xE4, 0xB8, 0xAD]), "中");
    // PDFDocEncoding 的弯引号在 0x8D/0x8E（不是 Windows-1252 的 0x93/0x94——那两个在这里是 ﬁ/ﬂ）。
    assert_eq!(decode_pdf_text_string(b"Caf\xe9 \x8dq\x8e \x84 \x93"), "Café “q” — ﬁ");
}

/// 有跨章的书内跳转 → 合成单文件，目录条目指向文件内锚点、正文链接是同文件裸锚点（xochitl 只认
/// 同文件 `#锚点` 为书内跳转，2026-09-23 真机实验坐实）。
#[test]
fn cross_chapter_goto_links_switch_to_single_file_with_anchor_nav() {
    let p0 = SynPage {
        content: "BT /F1 24 Tf 100 740 Td (Alpha) Tj ET\nBT /F1 12 Tf 100 700 Td (Go read the beta chapter now please) Tj ET\n",
        with_image: false,
        links: vec![([98.0, 695.0, 300.0, 712.0], SynLink::Page(1))],
    };
    let p1 = SynPage { content: "BT /F1 24 Tf 100 740 Td (Beta) Tj ET\nBT /F1 12 Tf 100 700 Td (This is the beta chapter body text) Tj ET\n", with_image: false, links: vec![] };
    let (book, report, _) = convert_synthetic(&build_synthetic_pdf_pages(&[p0, p1]));
    assert_eq!(book.chapters.len(), 1, "跨章跳转应合成单文件");
    assert_eq!(report.chapters, 2, "回执里的章数按目录条目算");
    let hrefs: Vec<&str> = book.nav.iter().map(|n| n.href.as_str()).collect();
    assert_eq!(hrefs, vec!["chap_0001.xhtml#pdf-p1", "chap_0001.xhtml#pdf-p2"]);
    let body = &book.chapters[0].html_body;
    assert!(!body.contains("></a>"), "不能有空锚点元素: {body}");
    assert!(body.contains("<a href=\"#pdf-p2\">"), "正文链接应是同文件裸锚点: {body}");
    assert!(body.contains("<h2 id=\"pdf-p1\">Alpha</h2>") && body.contains("<h2 id=\"pdf-p2\">Beta</h2>"), "章标题升级时锚点 id 跟到 <h2> 上: {body}");
}

#[test]
fn width_class_rounds_to_5_percent_steps_and_clamps() {
    assert_eq!(width_class(150.0, Some(400.0)), Some(40), "37.5% → 最近的 5% 档");
    assert_eq!(width_class(2.0, Some(400.0)), Some(5), "小图标下限 5%");
    assert_eq!(width_class(900.0, Some(400.0)), Some(100), "跨栏大图封顶 100%");
    assert_eq!(width_class(150.0, None), None, "栏宽未知就不缩放");
    assert_eq!(width_class(0.0, Some(400.0)), None);
}

/// 图片按原 PDF 占正文栏宽的比例定宽（外链 class，xochitl 不认内联 style）。
#[test]
fn image_gets_width_class_proportional_to_text_column() {
    let line = "This line of body text spans most of the column width here";
    let mut content = String::new();
    for i in 0..6 {
        content.push_str(&format!("BT /F1 12 Tf 100 {} Td ({line}) Tj ET\n", 700 - i * 14));
    }
    content.push_str("q 200 0 0 100 100 400 cm /Im1 Do Q\n");
    let (book, _, css) = convert_synthetic(&build_synthetic_pdf(&content, true));
    let body = joined_body(&book);
    let cls = regex::Regex::new(r#"<img class="cj-w(\d+)""#).unwrap().captures(&body).map(|c| c[1].parse::<u32>().unwrap()).expect(&body);
    let col = text_column_width(&extract_positioned_text(&build_synthetic_pdf(&content, true)).unwrap()).unwrap();
    assert_eq!(Some(cls), width_class(200.0, Some(col)), "栏宽 {col:.0}pt");
    assert!((40..=80).contains(&cls), "200pt 宽的图在约 330pt 的栏里应占一半多一点: {cls}");
    assert!(css.contains(&format!(".cj-w{cls}{{width:{cls}%;height:auto;}}")), "{css}");
}

#[test]
fn join_pages_continues_sentence_across_page_boundary_only_when_unfinished() {
    let p = |s: &str| s.to_string();
    assert_eq!(join_pages(&[p("<p>点击获取验</p>"), p("<p>证码后登陆。</p>")]), "<p>点击获取验证码后登陆。</p>");
    assert_eq!(join_pages(&[p("<p>句子结束了。</p>"), p("<p>新段落</p>")]), "<p>句子结束了。</p><p>新段落</p>");
    assert_eq!(join_pages(&[p("<p>unfinished <span class=\"cj-c0\">red</span></p>"), p("<p>tail</p>")]), "<p>unfinished <span class=\"cj-c0\">red</span>tail</p>");
    assert_eq!(join_pages(&[p("<p>unfinished</p>"), p("<p id=\"pdf-p2\">tail</p>")]), "<p>unfinished</p><p id=\"pdf-p2\">tail</p>", "跳转目标段落不接，id 保住");
    assert_eq!(join_pages(&[p("<p>图前</p>"), p("<p><img src=\"a.png\"/></p>")]), "<p>图前</p><p><img src=\"a.png\"/></p>", "下一页以图片开头不接");
    assert_eq!(join_pages(&[p("<h2>标题</h2>"), p("<p>正文</p>")]), "<h2>标题</h2><p>正文</p>");
}

/// 无彩色灰字按墨水屏提对比当黑字（不包 span），彩色字保留——与 EPUB 优化线同一判据（2026-09-23 T2）。
#[test]
fn gray_text_is_darkened_but_colored_text_kept() {
    let content = "0.5 g\nBT /F1 12 Tf 100 700 Td (GrayDate) Tj ET\n0.89 0.07 0.04 rg\nBT /F1 12 Tf 100 680 Td (RedHead) Tj ET\n";
    let (book, _, css) = convert_synthetic(&build_synthetic_pdf(content, false));
    let body = joined_body(&book);
    assert!(!body.contains("\">GrayDate"), "灰字不包颜色 span: {body}");
    assert!(body.contains("\">RedHead"), "彩色字保留: {body}");
    assert!(!css.contains("#808080") && !css.contains("#7f7f7f"), "{css}");
}

#[test]
fn crop_rgba_matches_image_crate_crop_including_clamped_edges() {
    let (w, h) = (7u32, 5u32);
    let full = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba([x as u8, y as u8, (x * y) as u8, 255]));
    for &(x, y, cw, ch) in &[(0, 0, 7, 5), (2, 1, 3, 2), (5, 3, 9, 9), (6, 4, 1, 1), (0, 4, 7, 1)] {
        let want = image::imageops::crop_imm(&full, x, y, cw, ch).to_image();
        assert_eq!(crop_rgba(full.as_raw(), w, h, x, y, cw, ch).unwrap(), want, "({x},{y},{cw},{ch})");
    }
    assert!(crop_rgba(&[0u8; 10], w, h, 0, 0, 1, 1).is_none(), "像素长度对不上 → None");
}

#[test]
fn pdf_doc_title_follows_indirect_info_and_title() {
    use lopdf::{dictionary, Document, Object, StringFormat};
    let title = || Object::String(b"Indirect Title".to_vec(), StringFormat::Literal);
    // Info 引用 + Title 引用
    let mut doc = Document::with_version("1.5");
    doc.objects.insert((1, 0), title());
    doc.objects.insert((2, 0), Object::Dictionary(dictionary! { "Title" => Object::Reference((1, 0)) }));
    doc.trailer.set("Info", Object::Reference((2, 0)));
    assert_eq!(pdf_doc_title(&doc).as_deref(), Some("Indirect Title"));
    // Info 直接内嵌
    let mut doc = Document::with_version("1.5");
    doc.trailer.set("Info", Object::Dictionary(dictionary! { "Title" => title() }));
    assert_eq!(pdf_doc_title(&doc).as_deref(), Some("Indirect Title"));
    // 原有写法照旧
    let mut doc = Document::with_version("1.5");
    doc.objects.insert((2, 0), Object::Dictionary(dictionary! { "Title" => title() }));
    doc.trailer.set("Info", Object::Reference((2, 0)));
    assert_eq!(pdf_doc_title(&doc).as_deref(), Some("Indirect Title"));
}
