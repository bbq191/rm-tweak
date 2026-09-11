//! 行首手写标记（OCR 路）：转写文本开头的符号决定这条条目怎么归类。
//! 几何判定（ink-serve）优先；这里兜底：几何没认出来（仍是 Body）时，用转写结果再认一次，并把标记从正文剥掉
//! （笔记本样式自带项目符号/编号，正文里再留一份会重复）。多行文本只看第一行的行首。
//!
//! **2026-09-07 用户定案**：标记表参照 Markdown 标题级别，零学习成本——`### 文字`＝小节标题；
//! `#`（对应 Title）**这次不接**：Title 在当前设计里是整章级别的（一份生成的笔记本只有一个 Title，
//! 来自 epubmap 章名，不是哪条条目决定的），条目级 `#` 没有现成字段可落——用户明确这是留给以后
//! "纯手写笔记扫描"（会议/上课，没有 EPUB 章节可依附）那条还没做的线，不要为了这条书摘注释线
//! 硬造一个用不上的字段。真要接的时候再回来加。
//!
//! **2026-09-08 三期**：`## 文字`（分区头）连同"分区"整个概念一起被砍掉了（用户拍板——AI 触发早就
//! 是 `Entry.ask_ai`/`question` 的事，笔记本排版分组也不要了，条目按页序平铺）——`Marker::Section`
//! 这个变体删了，但 `## 文字` 这个**手写标记本身**不废：用户明确要求继续识别，跟 `### 文字` 合并
//! 成同一件事——两个/三个（及以上）`#` 都覆盖 `Entry.subhead`，不分层级。单 `#` 仍然不接（见上一段）。
use crate::model::Style;

/// 行首标记认出来的结果：内容样式（`Style`）或结构性标记（小节）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    /// `-`/`1.`/`口` 等：这条内容本身该用什么样式。
    Style(Style),
    /// `## 文字`/`### 文字`：这条的小节标题覆盖成这个（`Entry.subhead`，平时由 epubmap 自动填）——
    /// 两个/三个及以上 `#` 不分层级，都是同一件事（三期合并，见模块文档）。
    Subhead(String),
}

/// 拆出行首标记：返回（认出的标记, 去掉标记符号后的文本）。认不出 → `(None, 原文)`。
pub fn split_leading_marker(text: &str) -> (Option<Marker>, String) {
    let t = text.trim_start();
    let (first, rest) = match t.find('\n') {
        Some(i) => (&t[..i], &t[i..]),
        None => (t, ""),
    };
    let f = first.trim_start();
    let strip = |body: &str| format!("{}{}", body.trim_start(), rest);

    // `#` 计数一次性数完：两个及以上就认（`##`/`### 文字`＝小节标题，三期合并不分层级），单 `#`
    // 仍然不接（Title 是整章级别的，见模块文档）。
    let hashes = f.chars().take_while(|&c| c == '#').count();
    if hashes >= 2 {
        let name = f[hashes..].trim();
        if !name.is_empty() {
            return (Some(Marker::Subhead(name.to_string())), strip(&f[hashes..]));
        }
    }
    // 待办：空心方框（真方框或手写成的「口」字）
    for m in ["□", "☐", "口"] {
        if let Some(b) = f.strip_prefix(m) {
            if m != "口" || b.starts_with([' ', '\u{3000}']) || b.is_empty() {
                return (Some(Marker::Style(Style::Checkbox)), strip(b));
            }
        }
    }
    // 无序：短横 / 实心点 / 项目符号 + 空格（避免把负数、连字符吃掉）
    for m in ["- ", "• ", "· ", "* ", "—", "－"] {
        if let Some(b) = f.strip_prefix(m) {
            return (Some(Marker::Style(Style::Bullet)), strip(b));
        }
    }
    // 有序：`1.` `2、` `3)` `4．`
    let digits: String = f.chars().take_while(|c| c.is_ascii_digit()).collect();
    if !digits.is_empty() && digits.len() <= 3 {
        let after = &f[digits.len()..];
        for p in [".", "、", ")", "）", "．"] {
            if let Some(b) = after.strip_prefix(p) {
                return (Some(Marker::Style(Style::Numbered)), strip(b));
            }
        }
    }
    (None, text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_three_style_markers_and_strips() {
        assert_eq!(split_leading_marker("- 查作者"), (Some(Marker::Style(Style::Bullet)), "查作者".into()));
        assert_eq!(split_leading_marker("• 查作者\n第二行"), (Some(Marker::Style(Style::Bullet)), "查作者\n第二行".into()));
        assert_eq!(split_leading_marker("1. 背诵"), (Some(Marker::Style(Style::Numbered)), "背诵".into()));
        assert_eq!(split_leading_marker("12、要点"), (Some(Marker::Style(Style::Numbered)), "要点".into()));
        assert_eq!(split_leading_marker("□ 找原文"), (Some(Marker::Style(Style::Checkbox)), "找原文".into()));
        assert_eq!(split_leading_marker("口 找原文"), (Some(Marker::Style(Style::Checkbox)), "找原文".into()));
    }

    #[test]
    fn recognizes_subhead_marker_two_or_more_hashes_no_level_distinction() {
        assert_eq!(split_leading_marker("### 人物关系"), (Some(Marker::Subhead("人物关系".into())), "人物关系".into()));
        assert_eq!(split_leading_marker("###人物关系\n第二行"), (Some(Marker::Subhead("人物关系".into())), "人物关系\n第二行".into()), "没空格也认");
        assert_eq!(split_leading_marker("###"), (None, "###".into()), "光标记没文字，不算数");
        assert_eq!(split_leading_marker("#### 更深一层"), (Some(Marker::Subhead("更深一层".into())), "更深一层".into()), "四个及以上 # 也按小节处理，不单独开第三级");
        // 三期：`## 文字`（两个 #）跟 `### 文字` 合并成同一件事，都覆盖 subhead，不分层级（用户明确要求
        // 继续识别 `##`，只是不再驱动"分区"那套已删除的数据结构）。
        assert_eq!(split_leading_marker("## 查询相关"), (Some(Marker::Subhead("查询相关".into())), "查询相关".into()), "双 # 跟三个 # 是同一件事");
        assert_eq!(split_leading_marker("##查询相关"), (Some(Marker::Subhead("查询相关".into())), "查询相关".into()), "没空格也认");
        assert_eq!(split_leading_marker("##"), (None, "##".into()), "光标记没文字，不算数");
    }

    #[test]
    fn leaves_plain_text_alone() {
        assert_eq!(split_leading_marker("口渴了"), (None, "口渴了".into()), "「口」后面直接是字＝正常词");
        assert_eq!(split_leading_marker("-3 度"), (None, "-3 度".into()), "负数不是无序");
        assert_eq!(split_leading_marker("2024 年"), (None, "2024 年".into()));
        assert_eq!(split_leading_marker("没听懂"), (None, "没听懂".into()));
        assert_eq!(split_leading_marker("# 只是个标题符号，这条线暂不接"), (None, "# 只是个标题符号，这条线暂不接".into()), "单 # 明确不识别，见模块文档");
        assert_eq!(split_leading_marker(""), (None, String::new()));
    }
}
