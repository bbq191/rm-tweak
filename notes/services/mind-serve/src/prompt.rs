//! 提示词（纯函数）：把"书名 + 章节 + 勾画原文 + 手写转写文本 + 用户问题"拼成一份给文字模型的提示——
//! 二期设计要点原文（笔记线白皮书 §03n）。`custom` 非空时替换内置的角色设定/答题要求那句，
//! 上下文（书/章/原文/批注/问题）总是照拼，不受 `custom` 影响。
const DEFAULT: &str = "你在帮一个人阅读时回答问题。下面是他在书里勾画/批注的上下文，回答要贴合这段上下文、直接切题，\
不要泛泛而谈；能简短说清楚就别写长篇。";

pub struct Context<'a> {
    pub book: &'a str,
    pub chapter: &'a str,
    /// 勾画原文（没有勾画就是本页批注，传 None）。
    pub quote: Option<&'a str>,
    /// 已校对文本或转写草稿（还没转写出来就传 None——问题本身多半已经说清楚要问什么）。
    pub text: Option<&'a str>,
    pub question: &'a str,
}

/// 截断长字段，别把整本书的原文喂进去；先 trim 再截（跟 `backend.rs` 的 `trunc` 唯一的差别，
/// 2026-09-09 收进 `vendorcfg::truncate_chars` 之后只剩这一行胶水）。
fn take(s: &str, n: usize) -> String {
    vendorcfg::truncate_chars(s.trim(), n)
}

pub fn build(custom: &str, ctx: &Context<'_>) -> String {
    let intro = if custom.trim().is_empty() { DEFAULT } else { custom.trim() };
    let mut p = String::from(intro);
    p.push_str("\n\n");
    if !ctx.book.trim().is_empty() {
        p.push_str(&format!("书名：{}\n", take(ctx.book, 100)));
    }
    if !ctx.chapter.trim().is_empty() {
        p.push_str(&format!("章节：{}\n", take(ctx.chapter, 100)));
    }
    if let Some(q) = ctx.quote.map(str::trim).filter(|q| !q.is_empty()) {
        p.push_str(&format!("勾画原文：「{}」\n", take(q, 500)));
    }
    if let Some(t) = ctx.text.map(str::trim).filter(|t| !t.is_empty()) {
        p.push_str(&format!("旁边的批注：{}\n", take(t, 500)));
    }
    p.push_str(&format!("\n问题：{}", take(ctx.question, 500)));
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_book_chapter_quote_text_and_question() {
        let p = build("", &Context { book: "人骨拼圖", chapter: "第一部 一天的國王", quote: Some("約翰．傑．查普曼"), text: Some("这是谁"), question: "他是做什么的" });
        assert!(p.starts_with("你在帮一个人阅读"));
        assert!(p.contains("书名：人骨拼圖"));
        assert!(p.contains("章节：第一部 一天的國王"));
        assert!(p.contains("勾画原文：「約翰．傑．查普曼」"));
        assert!(p.contains("旁边的批注：这是谁"));
        assert!(p.ends_with("问题：他是做什么的"));
    }

    #[test]
    fn empty_optional_fields_are_omitted_and_custom_replaces_intro() {
        let p = build("只回答一句话", &Context { book: "", chapter: "", quote: None, text: None, question: "问题" });
        assert!(p.starts_with("只回答一句话"));
        assert!(!p.contains("书名：") && !p.contains("章节：") && !p.contains("勾画原文：") && !p.contains("旁边的批注："));
        assert!(p.ends_with("问题：问题"));
    }

    #[test]
    fn long_fields_are_truncated() {
        let long: String = "字".repeat(600);
        let p = build("", &Context { book: "b", chapter: "c", quote: Some(&long), text: None, question: "q" });
        let line = p.lines().find(|l| l.starts_with("勾画原文：")).unwrap();
        assert_eq!(line.chars().filter(|c| *c == '字').count(), 500);
    }
}
