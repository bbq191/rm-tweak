//! 单篇 markdown → 设备笔记本段落列表（`rmv6::write::Paragraph`）。跟 `marker`（从 OCR 纯文本反推
//! 行首标记、剥掉标记留正文）方向相反：这里输入已经是规范 markdown 语法，直接按行识别块级结构、
//! 映射到 xochitl 原生打字样式。逐行处理——一行 markdown 对应一段 `.rm` 段落，跟 xochitl 打字文本
//! 本身"段落=一行"的结构对齐，不做跨行的块级合并。
//!
//! **已知限制**（xochitl 打字格式的协议层限制，不是这里能绕开的实现选择）：
//! - 行内 `**加粗**`/`*斜体*`/`` `代码` ``：xochitl 的样式表是**段落粒度**、不是字符粒度，一行文字
//!   没法一半加粗一半不加粗。这里只剥离**配对**出现的分隔符号（保留中间文字，不产生任何加粗/斜体
//!   视觉效果）；不成对的分隔符号原样保留，不强行猜测意图。
//! - `- [x]` 已勾选待办：写入器造不出勾选态（`rmv6::write` 只支持未勾选 `CHECKBOX`，勾选态要靠原生
//!   手指点方框），一律降级成未勾选 `- [ ]`，正文不保留 "x" 字样。
//! - 四级及以上标题（`####` 起）没有第三级原生小节样式，统一降级成 Subheading 2（跟三级标题一样）。
//! - 有序列表编号是 xochitl 按"连续几个 NUMBERED 段落"渲染时自动算的，这里只剥离 markdown 里写的
//!   数字本身，不做任何编号相关处理（连续性被打断则从 1 重来，这是 `rmv6::write` 早就记录的限制）。
use rmv6::v6::scene_item::text::ParagraphStyle;
use rmv6::write::Paragraph;

/// 逐行转换。空行（含只有空白字符的行）不产生段落——`.rm` 段落本身自带换行，纯粹的空行在这里
/// 只是视觉分隔，不需要专门生成一个空段落。剥完标记/行内符号后如果整行变成空字符串（比如
/// `"# "`只有标题符号没有正文），同样丢弃这一行，不生成空段落（`rmv6::write` 不接受空段落）。
pub fn markdown_to_paragraphs(md: &str) -> Vec<Paragraph> {
    md.lines().filter_map(line_to_paragraph).collect()
}

fn line_to_paragraph(line: &str) -> Option<Paragraph> {
    if line.trim().is_empty() {
        return None;
    }
    // 只删前导空白：标题判定需要"井号后面紧跟一个空格"这个信号，先整行 trim() 会把这个空格连带
    // 行尾一起吃掉（"# " → "#"），误判成"没有空格、不是标题"。各分支自己再对提取出的正文 trim()。
    let s = line.trim_start();
    let p = if let Some(p) = heading_paragraph(s) {
        p
    } else if let Some(text) = checkbox_text(s) {
        Paragraph::new(ParagraphStyle::CHECKBOX, strip_inline(text.trim()))
    } else if let Some(text) = bullet_text(s) {
        Paragraph::new(ParagraphStyle::BULLET, strip_inline(text.trim()))
    } else if let Some(text) = numbered_text(s) {
        Paragraph::new(ParagraphStyle::NUMBERED, strip_inline(text.trim()))
    } else {
        Paragraph::new(ParagraphStyle::PLAIN, strip_inline(s.trim()))
    };
    (!p.text.trim().is_empty()).then_some(p)
}

/// `#`＝大标题（`HEADING`，跟章名同一档）；`##`＝大字号小节（`Subheading 1`）；`###` 起统统降级成
/// 小字号小节（`Subheading 2`，跟三级标题一样处理——见模块文档"已知限制"）。必须是"井号+空格"
/// （`"# 文字"`），光秃秃的 `"#标签"` 不算标题，避免跟话题标签这类用法混淆。
fn heading_paragraph(s: &str) -> Option<Paragraph> {
    let hashes = s.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = s[hashes..].strip_prefix(' ')?;
    let text = strip_inline(rest.trim());
    Some(match hashes {
        1 => Paragraph::new(ParagraphStyle::HEADING, text),
        2 => Paragraph::subheading1(text),
        _ => Paragraph::new(ParagraphStyle::BOLD, text),
    })
}

/// checkbox 判定必须在 `bullet_text` 之前调用——两者前缀重叠（`"- "`），checkbox 更具体。
fn checkbox_text(s: &str) -> Option<String> {
    for prefix in ["- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] ", "* [X] "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return Some(rest.to_string());
        }
    }
    None
}

fn bullet_text(s: &str) -> Option<String> {
    for prefix in ["- ", "* ", "+ "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            return Some(rest.to_string());
        }
    }
    None
}

/// `"1. 文字"`/`"1) 文字"`——数字本身丢掉（xochitl 自动编号，见模块文档）。
fn numbered_text(s: &str) -> Option<String> {
    let digits: String = s.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() {
        return None;
    }
    let rest = &s[digits.len()..];
    let rest = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") "))?;
    Some(rest.to_string())
}

/// 剥离配对出现的行内分隔符号，保留中间文字；不成对的原样留着。粗体分隔符（两字符）先处理，
/// 避免 `**粗体**` 被误当成两对单字符 `*斜体*` 标记。
fn strip_inline(s: &str) -> String {
    let s = strip_paired(s.to_string(), "**");
    let s = strip_paired(s, "__");
    let s = strip_paired(s, "`");
    let s = strip_paired(s, "*");
    strip_paired(s, "_")
}

fn strip_paired(mut s: String, delim: &str) -> String {
    while let Some(start) = s.find(delim) {
        let after = start + delim.len();
        let Some(rel_end) = s[after..].find(delim) else { break };
        let end = after + rel_end;
        s.replace_range(end..end + delim.len(), ""); // 先删后面那个，前面的索引不受影响
        s.replace_range(start..after, "");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn styles(md: &str) -> Vec<(ParagraphStyle, String)> {
        markdown_to_paragraphs(md).into_iter().map(|p| (p.style, p.text)).collect()
    }

    #[test]
    fn blank_lines_produce_no_paragraphs() {
        assert!(markdown_to_paragraphs("\n\n   \n").is_empty());
        assert_eq!(markdown_to_paragraphs("正文\n\n下一段").len(), 2, "空行只是分隔，不生成空段落");
    }

    #[test]
    fn heading_levels_map_to_the_three_available_styles() {
        let ps = markdown_to_paragraphs("# 大标题\n## 大字号小节\n### 小字号小节\n#### 降级也是小字号");
        assert_eq!(ps[0].style, ParagraphStyle::HEADING);
        assert_eq!(ps[0].text, "大标题");
        assert_eq!(ps[1].style, ParagraphStyle::BOLD, "## 是 Subheading 1，跟三级共用 BOLD wire 码");
        assert_eq!(ps[2].style, ParagraphStyle::BOLD);
        assert_eq!(ps[3].style, ParagraphStyle::BOLD, "四级起统统降级");
    }

    #[test]
    fn bare_hash_without_space_is_not_a_heading() {
        assert_eq!(styles("#标签"), vec![(ParagraphStyle::PLAIN, "#标签".to_string())]);
    }

    #[test]
    fn heading_with_only_symbol_and_no_text_is_dropped() {
        assert!(markdown_to_paragraphs("# \n正文").len() == 1, "光秃秃的标题符号剥完是空段落，该丢掉");
    }

    #[test]
    fn bullets_numbers_and_checkboxes_strip_their_markers() {
        assert_eq!(styles("- 无序项"), vec![(ParagraphStyle::BULLET, "无序项".to_string())]);
        assert_eq!(styles("* 也是无序"), vec![(ParagraphStyle::BULLET, "也是无序".to_string())]);
        assert_eq!(styles("1. 有序项"), vec![(ParagraphStyle::NUMBERED, "有序项".to_string())]);
        assert_eq!(styles("12) 两位数序号"), vec![(ParagraphStyle::NUMBERED, "两位数序号".to_string())]);
        assert_eq!(styles("- [ ] 待办"), vec![(ParagraphStyle::CHECKBOX, "待办".to_string())]);
    }

    #[test]
    fn checked_checkbox_degrades_to_unchecked_without_x_literal() {
        assert_eq!(styles("- [x] 已完成"), vec![(ParagraphStyle::CHECKBOX, "已完成".to_string())], "写入器造不出勾选态，降级成未勾选、正文不留 x 字样");
        assert_eq!(styles("- [X] 大写也一样"), vec![(ParagraphStyle::CHECKBOX, "大写也一样".to_string())]);
    }

    #[test]
    fn checkbox_takes_priority_over_plain_bullet_due_to_shared_prefix() {
        // "- [ ] " 和 "- " 前缀重叠，必须先判 checkbox，不然会被 bullet 分支当成
        // "无序项：字面文字「[ ] 待办」"。
        assert_eq!(styles("- [ ] 待办")[0].0, ParagraphStyle::CHECKBOX);
    }

    #[test]
    fn inline_bold_italic_code_markers_are_stripped_paired_only() {
        assert_eq!(styles("这是**加粗**文字"), vec![(ParagraphStyle::PLAIN, "这是加粗文字".to_string())]);
        assert_eq!(styles("这是*斜体*文字"), vec![(ParagraphStyle::PLAIN, "这是斜体文字".to_string())]);
        assert_eq!(styles("这是`代码`片段"), vec![(ParagraphStyle::PLAIN, "这是代码片段".to_string())]);
        assert_eq!(styles("单独一个 * 星号不成对，原样留着"), vec![(ParagraphStyle::PLAIN, "单独一个 * 星号不成对，原样留着".to_string())]);
    }

    #[test]
    fn inline_markers_inside_list_and_heading_are_also_stripped() {
        assert_eq!(styles("- **重点**列表项"), vec![(ParagraphStyle::BULLET, "重点列表项".to_string())]);
        assert_eq!(styles("## **重点**标题"), vec![(ParagraphStyle::BOLD, "重点标题".to_string())]);
    }

    #[test]
    fn mixed_document_end_to_end() {
        let md = "# 读书笔记\n\n## 第一节\n\n- 要点一\n- 要点二\n\n1. 步骤一\n2. 步骤二\n\n- [ ] 待办事项\n- [x] 已完成事项\n\n普通段落，含**加粗**与*斜体*。";
        let got = styles(md);
        assert_eq!(
            got,
            vec![
                (ParagraphStyle::HEADING, "读书笔记".to_string()),
                (ParagraphStyle::BOLD, "第一节".to_string()),
                (ParagraphStyle::BULLET, "要点一".to_string()),
                (ParagraphStyle::BULLET, "要点二".to_string()),
                (ParagraphStyle::NUMBERED, "步骤一".to_string()),
                (ParagraphStyle::NUMBERED, "步骤二".to_string()),
                (ParagraphStyle::CHECKBOX, "待办事项".to_string()),
                (ParagraphStyle::CHECKBOX, "已完成事项".to_string()),
                (ParagraphStyle::PLAIN, "普通段落，含加粗与斜体。".to_string()),
            ]
        );
    }
}
