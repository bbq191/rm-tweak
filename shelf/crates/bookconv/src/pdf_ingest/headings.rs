//! 按字号推断标题（章节切分依据）。
use super::*;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Heading {
    pub page: usize, // 0-indexed
    pub line: usize,
    pub level: i64, // 1=最大字号…最多 3 档
    pub title: String,
}

/// 标题候选字号相对正文基准字号的倍数下限。
pub const HEADING_SIZE_RATIO: f64 = 1.2;
/// 没有识别出任何标题候选时的兜底分块页数。
pub const FALLBACK_CHUNK_PAGES: usize = 20;

/// 一页的"正文基准字号"＝出现次数最多的字号（众数）；字号按 0.5pt 归并，避免浮点误差把同一个
/// 视觉字号拆成好几个桶。
pub(super) fn body_font_size(page: &[PositionedChar]) -> f64 {
    use std::collections::HashMap;
    let mut counts: HashMap<i64, usize> = HashMap::new();
    for c in page {
        if c.ch.is_whitespace() {
            continue;
        }
        let bucket = (c.font_size * 2.0).round() as i64;
        *counts.entry(bucket).or_insert(0) += 1;
    }
    counts.into_iter().max_by_key(|(_, n)| *n).map(|(b, _)| b as f64 / 2.0).unwrap_or(10.0)
}

/// 第 `ln` 行的非空白字符（行号没有字符 → 空）。
fn visible_of<'a>(lines: &std::collections::BTreeMap<usize, Vec<&'a PositionedChar>>, ln: usize) -> Vec<&'a PositionedChar> {
    lines.get(&ln).map(|v| v.iter().copied().filter(|c| !c.ch.is_whitespace()).collect()).unwrap_or_default()
}

/// 按字号识别标题：每页算正文基准字号，字号 ≥ 基准 × [`HEADING_SIZE_RATIO`] 的行判成标题候选；
/// 连续（同页、行号相邻）的候选行合并成一个标题；不同字号分档映射 `level`（最大字号＝1，最多
/// 3 档，超出封顶到 3）。**全书零候选**时不在这里兜底——那是调用方（`optimize_pdf_to_epub`）
/// 的责任：识别不出结构就按 [`FALLBACK_CHUNK_PAGES`] 固定页数分块，不假装有真实章节。
pub(crate) fn detect_headings_by_font_size(pages: &[PageContent]) -> Vec<Heading> {
    let mut headings = Vec::new();
    // 全书统一的"字号→level"映射：先收集所有页面里出现过的、判定为标题候选的字号，降序去重，
    // 取前 3 档；不同页各自独立判"是不是标题候选"（相对各自页的正文基准），但档位映射是全书
    // 统一的，不然同一本书不同页的"最大字号"却对应不同 level，nav.xhtml 嵌套会乱。
    let mut candidate_sizes: Vec<i64> = Vec::new();
    let mut per_page_body: Vec<f64> = Vec::with_capacity(pages.len());
    // 每页按行号分组一次（见 `group_by_line`：此前逐行号全页扫描，复杂度"行数 × 字符数"），两遍共用。
    let grouped: Vec<std::collections::BTreeMap<usize, Vec<&PositionedChar>>> = pages.iter().map(|p| group_by_line(&p.chars)).collect();
    for (page, lines) in pages.iter().zip(&grouped) {
        let body = body_font_size(&page.chars);
        per_page_body.push(body);
        for &line_no in lines.keys() {
            let line_chars = visible_of(lines, line_no);
            if line_chars.is_empty() {
                continue;
            }
            let avg = avg_font_size(&line_chars);
            if avg >= body * HEADING_SIZE_RATIO {
                candidate_sizes.push((avg * 2.0).round() as i64);
            }
        }
    }
    if candidate_sizes.is_empty() {
        return headings;
    }
    candidate_sizes.sort_unstable();
    candidate_sizes.dedup();
    candidate_sizes.reverse(); // 大字号在前＝level 1
    let level_of = |size: f64| -> i64 {
        let bucket = (size * 2.0).round() as i64;
        let rank = candidate_sizes.iter().position(|&s| s == bucket).unwrap_or(candidate_sizes.len() - 1);
        (rank as i64 + 1).min(3)
    };

    for (page_idx, lines) in grouped.iter().enumerate() {
        let body = per_page_body[page_idx];
        // 已被上一个标题合并掉的行号（`i = j` 跳过），没有字符的行号本来就不在 `lines` 里。
        let mut next_free = 0usize;
        for &i in lines.keys() {
            if i < next_free {
                continue;
            }
            let line_chars = visible_of(lines, i);
            if line_chars.is_empty() {
                continue;
            }
            let avg = avg_font_size(&line_chars);
            if avg < body * HEADING_SIZE_RATIO {
                continue;
            }
            // 连续的标题候选行合并成一个标题（同一个标题换行显示的情况）。标题文字要保留空格
            // （行内原有的词间空格），只有判"是不是标题候选"用的字号统计才该滤掉空白字符——
            // 之前误用同一份过滤后的 line_chars 拼标题，"1 Introduction" 会被拼成
            // "1Introduction"，2026-09-19 真机样本核对时发现。
            let title_line = |ln: usize| -> String { lines.get(&ln).map(|v| v.iter().map(|c| c.ch).collect::<String>().trim().to_string()).unwrap_or_default() };
            let mut title: String = title_line(i);
            let level = level_of(avg);
            let mut j = i + 1;
            loop {
                let next_chars = visible_of(lines, j);
                if next_chars.is_empty() {
                    break;
                }
                let next_avg = avg_font_size(&next_chars);
                if next_avg < body * HEADING_SIZE_RATIO || level_of(next_avg) != level {
                    break;
                }
                title.push(' ');
                title.push_str(&title_line(j));
                j += 1;
            }
            headings.push(Heading { page: page_idx, line: i, level, title: title.trim().to_string() });
            next_free = j;
        }
    }
    headings
}

// ============================================================================
// 裁边路径（Comic / NoTextLayer 共用，格式不变仍是 PDF）
// ============================================================================
