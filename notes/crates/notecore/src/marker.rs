//! 行首手写标记（OCR 路）：转写文本开头的符号决定这条条目怎么归类。
//! 目前只有这一条判定路（没有按笔迹几何认标记）：条目样式仍是 Body 时，用转写结果认一次，并把标记从正文剥掉
//! （笔记本样式自带项目符号/编号，正文里再留一份会重复）。多行文本只看第一行的行首。
//!
//! **2026-09-07 用户定案**：标记表参照 Markdown 标题级别，零学习成本——`### 文字`＝小节标题；
//! `#`（对应 Title）**这次不接**：Title 在当前设计里是整章级别的（一份生成的笔记本只有一个 Title，
//! 来自 epubmap 章名，不是哪条条目决定的），条目级 `#` 没有现成字段可落——用户明确这是留给以后
//! "纯手写笔记扫描"（会议/上课，没有 EPUB 章节可依附）那条还没做的线，不要为了这条书摘注释线
//! 硬造一个用不上的字段。真要接的时候再回来加。
//!
//! **2026-09-08 三期**：`## 文字`（分区头）连同"分区"整个概念一起被砍掉；`##`/`###` 曾合并成"覆盖 `Entry.subhead`"。
//!
//! **2026-09-25 用户定案：行首标记与设备内置打字样式一一对应**（跟 md 导入 `mdimport.rs` 同一套）：
//! `## 文字`＝Subheading 1、`### 文字`（三个及以上 `#`）＝Subheading 2、`1.`＝编号列表、`-`＝圆点列表、
//! `- [ ]`/`- []`/`[ ]`/`- [x]`/`口`/`□`＝复选框（勾选态写不出，一律未勾选）。标题从此是条目自己的样式，
//! 不再改 `Entry.subhead`——`subhead` 只装书的小节名（epubmap 按目录自动填），由投影在小节变化时输出。
//! 单 `#`（Title）仍然不接：Title 是整章级别的（章名），条目级不落。
use crate::model::Style;

/// 行首标记认出来的结果：这条内容该用的样式。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    Style(Style),
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

    // `#` 计数一次性数完：`##`＝Subheading 1，`###` 及以上＝Subheading 2；单 `#` 不接（Title 是章名，见模块文档）。
    let hashes = f.chars().take_while(|&c| c == '#').count();
    if hashes >= 2 {
        let name = f[hashes..].trim();
        if !name.is_empty() {
            let style = if hashes == 2 { Style::Heading1 } else { Style::Heading2 };
            return (Some(Marker::Style(style)), strip(&f[hashes..]));
        }
    }
    // 待办：Markdown 复选框（`- [ ]`、`- []`、`[ ]`、`- [x]`…，必须在"无序 `- `"之前判），或空心方框（真方框/手写成的「口」字）
    if let Some(b) = checkbox_body(f) {
        return (Some(Marker::Style(Style::Checkbox)), strip(b));
    }
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
            // 紧跟数字的是小数/版本号（`3.14`、`1.5倍`），不是编号：此前会被剥成编号条目"14 是圆周率"，
            // 网页里手打的正文也走这条规则，等于改了用户的字（2026-09-25 第四轮审计）。
            if let Some(b) = after.strip_prefix(p).filter(|b| !b.starts_with(|c: char| c.is_ascii_digit())) {
                return (Some(Marker::Style(Style::Numbered)), strip(b));
            }
        }
    }
    (None, text.to_string())
}

/// Markdown 复选框前缀：可选的 `-`/`*` + 可选空格 + `[` + 空 / 空格 / x/X + `]` + 空格或行尾；返回其后的正文。
/// 手写转写常见 `-[ ]`、`- []`、`[]` 这类不规范写法，一并认；方括号里是别的字（`[注]`）不算。
fn checkbox_body(f: &str) -> Option<&str> {
    let t = f.strip_prefix(['-', '*']).map(str::trim_start).unwrap_or(f);
    let t = t.strip_prefix(['[', '［'])?;
    let t = t.strip_prefix([' ', 'x', 'X', '\u{3000}']).unwrap_or(t);
    let t = t.strip_prefix([']', '］'])?;
    (t.is_empty() || t.starts_with([' ', '\u{3000}'])).then_some(t)
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
    fn hashes_map_to_device_subheadings() {
        assert_eq!(split_leading_marker("## 查询相关"), (Some(Marker::Style(Style::Heading1)), "查询相关".into()), "## ＝ Subheading 1");
        assert_eq!(split_leading_marker("##查询相关"), (Some(Marker::Style(Style::Heading1)), "查询相关".into()), "没空格也认");
        assert_eq!(split_leading_marker("### 人物关系"), (Some(Marker::Style(Style::Heading2)), "人物关系".into()), "### ＝ Subheading 2");
        assert_eq!(split_leading_marker("###人物关系\n第二行"), (Some(Marker::Style(Style::Heading2)), "人物关系\n第二行".into()));
        assert_eq!(split_leading_marker("#### 更深一层"), (Some(Marker::Style(Style::Heading2)), "更深一层".into()), "四个及以上 # 也是 Subheading 2（设备只有两级小标题）");
        assert_eq!(split_leading_marker("##"), (None, "##".into()), "光标记没文字，不算数");
        assert_eq!(split_leading_marker("###"), (None, "###".into()));
    }

    #[test]
    fn markdown_checkboxes_map_to_device_checkbox_before_bullet() {
        for (raw, body) in [("- [ ] 找原文", "找原文"), ("- [] 找原文", "找原文"), ("-[ ] 找原文", "找原文"), ("[ ] 找原文", "找原文"), ("[] 找原文", "找原文"), ("- [x] 已做", "已做"), ("* [X] 已做", "已做"), ("［ ］ 全角", "全角")] {
            assert_eq!(split_leading_marker(raw), (Some(Marker::Style(Style::Checkbox)), body.into()), "{raw}");
        }
        assert_eq!(split_leading_marker("- [注] 不是复选框"), (Some(Marker::Style(Style::Bullet)), "[注] 不是复选框".into()), "方括号里是别的字：按无序处理");
        assert_eq!(split_leading_marker("[1] 参考文献"), (None, "[1] 参考文献".into()));
    }

    #[test]
    fn leaves_plain_text_alone() {
        assert_eq!(split_leading_marker("口渴了"), (None, "口渴了".into()), "「口」后面直接是字＝正常词");
        assert_eq!(split_leading_marker("-3 度"), (None, "-3 度".into()), "负数不是无序");
        assert_eq!(split_leading_marker("2024 年"), (None, "2024 年".into()));
        assert_eq!(split_leading_marker("3.14 是圆周率"), (None, "3.14 是圆周率".into()), "小数不是编号");
        assert_eq!(split_leading_marker("1.5倍速"), (None, "1.5倍速".into()));
        assert_eq!(split_leading_marker("2.背诵"), (Some(Marker::Style(Style::Numbered)), "背诵".into()), "编号后没空格仍认");
        assert_eq!(split_leading_marker("没听懂"), (None, "没听懂".into()));
        assert_eq!(split_leading_marker("# 只是个标题符号，这条线暂不接"), (None, "# 只是个标题符号，这条线暂不接".into()), "单 # 明确不识别，见模块文档");
        assert_eq!(split_leading_marker(""), (None, String::new()));
    }
}
