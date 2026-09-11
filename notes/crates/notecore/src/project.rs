//! 条目库 → 设备笔记本投影（纯函数，零 I/O）：把一章的条目按页序铺成 `rmv6::write::Paragraph` 列表，
//! note-serve 拿它去打包 `.rmdoc`、走 `/upload`。这里只管"排成什么样"，不管"怎么传"（那是
//! `note-serve::publish` 的事，出于"笔记本只读、由条目库投影生成"的原则，本模块不产生任何副作用）。
//!
//! 布局：Title=章名 → 条目按 `page_index` 平铺（样式=`Entry.style`，文本=`display_text()`，没转写
//! 占位"（待转写）"）→ 条目下各跟一段 Body 摘录勾画原文（有的话）与 AI 回答（有的话）。
//! 已撤销（`Status::Revoked`）条目不投影；一章零条目 → `None`（调用方不该为空章生成文档）。
//!
//! **2026-09-08 三期：砍掉"分区"**（`Section`/`Entry.section`/`Book.sections` 整个概念都删了）——
//! 分区身兼两职：给 AI 提示词分组（二期就已经改成按条目单发的"问AI"，这职责早就跟分区脱钩了）、
//! 给笔记本排版分组（`## 文字`手写标记 → Subheading 1 分区头 → 分区内条目）。用户直接拍板这第二职
//! 也不要了：条目就按页序平铺，靠各自的 `Style`（正文/无序/有序/待办）做唯一的格式区分，不再有
//! "分区头"这层结构。旧版本按分区分组+"未分区"兜底的逻辑整段删除，见 git 历史。
//!
//! ⚠️ 已知限制（先记录、不在本模块解决）：xochitl 按"连续几个 NUMBERED 段落"自动编号，摘录/回答的
//! Body 段插在两条 NUMBERED 条目之间会打断连续、导致编号从 1 重来——真要连续编号的清单，条目之间
//! 目前不能有摘录/回答。留给以后有真机样本再决定要不要为此改变编排。
use crate::hash::{fnv1a, hex};
use crate::model::{Book, Entry};
use rmv6::v6::scene_item::text::ParagraphStyle;
use rmv6::write::Paragraph;

fn style_to_wire(s: crate::model::Style) -> ParagraphStyle {
    match s {
        crate::model::Style::Body => ParagraphStyle::PLAIN,
        crate::model::Style::Bullet => ParagraphStyle::BULLET,
        crate::model::Style::Numbered => ParagraphStyle::NUMBERED,
        crate::model::Style::Checkbox => ParagraphStyle::CHECKBOX,
    }
}

/// 参与投影的条目：本章、真被要求转笔记的（`Pending`/`Draft`/`Reviewed`——**不是**"非撤销"这种
/// 反着写的判据；`Mined`/`Skipped` 都不该出现，之前拿 `!= Revoked` 当判据是个真 bug：`Skipped`
/// （用户点了"不需要"）之前会一直混进投影，2026-09-08 真机导出 md 测试时亲眼看见一条"不需要"的
/// 条目出现在导出文件里才揪出来，见笔记线白皮书 §03r；跟「整理」网页视图的过滤条件对齐，那边
/// 一直是对的）、去处要设备笔记本（`Notebook`/`Both`——三期新增，见 `model::Destination`；缺省
/// `Both`，不设置这个字段的老条目库行为不变），按页序排。
fn live_entries(book: &Book, chapter_idx: usize) -> Vec<&Entry> {
    let mut v: Vec<&Entry> = book.entries.iter().filter(|e| e.chapter == Some(chapter_idx) && e.status.is_live_for_projection() && e.destination.wants_notebook()).collect();
    v.sort_by_key(|e| e.page_index);
    v
}

fn push_entry(out: &mut Vec<Paragraph>, e: &Entry) {
    let text = e.display_text().map(str::to_string).unwrap_or_else(|| "（待转写）".to_string());
    out.push(Paragraph::new(style_to_wire(e.style), text));
    if let Some(q) = &e.quote {
        if !q.text.trim().is_empty() {
            out.push(Paragraph::new(ParagraphStyle::PLAIN, format!("〔原文〕{}", q.text)));
        }
    }
    if let Some(a) = &e.answer {
        if !a.text.trim().is_empty() {
            out.push(Paragraph::new(ParagraphStyle::PLAIN, format!("〔AI〕{}", a.text)));
        }
    }
}

/// 投影一章。`None` = 没有这一章、或这一章没有可投影的条目（全撤销/没条目）。
pub fn project_chapter(book: &Book, chapter_idx: usize) -> Option<Vec<Paragraph>> {
    let title = book.chapters.get(chapter_idx)?;
    let entries = live_entries(book, chapter_idx);
    if entries.is_empty() {
        return None;
    }
    let mut out = vec![Paragraph::new(ParagraphStyle::HEADING, title.clone())];
    for e in entries {
        push_entry(&mut out, e);
    }
    Some(out)
}

/// 一章的投影指纹：只要它不变，笔记本内容不变，note-serve 不必重新生成/重新上传。覆盖投影用到的
/// 全部输入（章名、条目的样式/文本/原文/回答），任何一项变了指纹就变。
pub fn fingerprint_chapter(book: &Book, chapter_idx: usize) -> Option<String> {
    let title = book.chapters.get(chapter_idx)?;
    let entries = live_entries(book, chapter_idx);
    if entries.is_empty() {
        return None;
    }
    let mut buf = String::new();
    buf.push_str(title);
    buf.push('\u{2}');
    for e in &entries {
        let text = e.display_text().unwrap_or("");
        let quote = e.quote.as_ref().map(|q| q.text.as_str()).unwrap_or("");
        let answer = e.answer.as_ref().map(|a| a.text.as_str()).unwrap_or("");
        buf.push_str(&format!("{}|{:?}|{}|{}|{}", e.id, e.style, text, quote, answer));
        buf.push('\u{1}');
    }
    Some(hex(fnv1a(buf.as_bytes())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Answer, Draft, Quote, Status, Style};

    fn entry(id: &str, chapter: usize, page_index: usize, style: Style, text: &str) -> Entry {
        Entry {
            id: id.into(),
            page: "p".into(),
            page_index,
            chapter: Some(chapter),
            chapter_title: String::new(),
            subhead: None,
            quote: None,
            ink: None,
            drafts: vec![],
            text: (!text.is_empty()).then(|| text.to_string()),
            style,
            ask_ai: false,
            question: None,
            answer: None,
            status: Status::Reviewed,
            destination: Default::default(),
            created: 0,
            updated: 0,
        }
    }

    fn book() -> Book {
        Book {
            uuid: "u".into(),
            title: "人骨拼图".into(),
            author: String::new(),
            chapters: vec!["第一章".into(), "第二章".into()],
            entries: vec![
                entry("e1", 0, 2, Style::Body, "林肯·莱姆"),
                entry("e2", 0, 1, Style::Bullet, "为什么是纽约"),
                entry("e3", 0, 5, Style::Body, "没归类的一条"),
                entry("e4", 1, 0, Style::Numbered, "第二章的条目"),
            ],
            page_mtimes: Default::default(),
        }
    }

    #[test]
    fn no_chapter_or_empty_chapter_yields_none() {
        let b = book();
        assert!(project_chapter(&b, 9).is_none(), "没有第 9 章");
        assert!(project_chapter(&b, 2).is_none(), "第 2 章（index）不存在，只有 0/1");
        assert!(fingerprint_chapter(&b, 9).is_none());
    }

    #[test]
    fn entries_are_flat_and_ordered_by_page_no_section_grouping() {
        let b = book();
        let ps = project_chapter(&b, 0).expect("第一章有条目");
        let texts: Vec<&str> = ps.iter().map(|p| p.text.as_str()).collect();
        assert_eq!(texts, ["第一章", "为什么是纽约", "林肯·莱姆", "没归类的一条"], "按页序平铺，e2(page 1) < e1(page 2) < e3(page 5)，没有分区头");
        assert_eq!(ps[0].style, ParagraphStyle::HEADING);
        assert_eq!(ps[1].style, ParagraphStyle::BULLET, "e2 是 Style::Bullet");
        assert_eq!(ps[2].style, ParagraphStyle::PLAIN, "e1 是 Style::Body");
        assert_eq!(ps[3].style, ParagraphStyle::PLAIN, "e3 也是 Style::Body");

        let ps2 = project_chapter(&b, 1).expect("第二章有条目");
        assert_eq!(ps2.iter().map(|p| p.text.as_str()).collect::<Vec<_>>(), ["第二章", "第二章的条目"]);
        assert_eq!(ps2[1].style, ParagraphStyle::NUMBERED);
    }

    #[test]
    fn revoked_entries_are_excluded_from_projection_and_fingerprint() {
        let mut b = book();
        let fp_before = fingerprint_chapter(&b, 1).unwrap();
        b.entries[3].status = Status::Revoked;
        assert!(project_chapter(&b, 1).is_none(), "唯一条目撤销后第二章没内容可投影");
        assert!(fingerprint_chapter(&b, 1).is_none());
        // 撤销前后指纹一定不同（这里只需确认"有值"变"无值"，上面两条已覆盖）
        assert!(!fp_before.is_empty());
    }

    /// 真机 bug（2026-09-08，见白皮书 §03r）：之前判据是 `!= Revoked`，`Skipped`（用户点了「不需要」）
    /// 会一直混进投影——真机导出 md 时亲眼看见一条"不需要"的条目出现在导出文件里才揪出来。
    #[test]
    fn skipped_and_mined_entries_are_excluded_not_just_revoked() {
        let mut b = book();
        b.entries[3].status = Status::Skipped;
        assert!(project_chapter(&b, 1).is_none(), "「不需要」的条目不该出现在设备笔记本投影里");
        b.entries[3].status = Status::Mined;
        assert!(project_chapter(&b, 1).is_none(), "还没被要求转笔记的条目也不该出现");
    }

    #[test]
    fn archived_entries_are_excluded_same_as_revoked() {
        let mut b = book();
        b.entries[3].status = Status::Archived;
        assert!(project_chapter(&b, 1).is_none(), "归档跟撤销一样从投影里消失");
    }

    #[test]
    fn destination_obsidian_only_excludes_entry_from_device_notebook_projection() {
        use crate::model::Destination;
        let mut b = book();
        b.entries[3].destination = Destination::Obsidian; // 第二章唯一条目改成"只导出 Obsidian"
        assert!(project_chapter(&b, 1).is_none(), "只想去 Obsidian 的条目不该出现在设备笔记本投影里");
        b.entries[3].destination = Destination::Notebook;
        assert!(project_chapter(&b, 1).is_some(), "改回只留设备/缺省 Both 都该重新出现");
    }

    #[test]
    fn fingerprint_changes_when_text_or_style_changes_but_not_on_no_op_rebuild() {
        let b = book();
        let fp1 = fingerprint_chapter(&b, 0).unwrap();
        let fp2 = fingerprint_chapter(&b, 0).unwrap();
        assert_eq!(fp1, fp2, "同样的书两次算出同一个指纹");

        let mut edited = b.clone();
        edited.entries[0].text = Some("换了校对文本".into());
        assert_ne!(fingerprint_chapter(&edited, 0).unwrap(), fp1, "校对文本变了指纹要变");

        let mut restyled = b.clone();
        restyled.entries[0].style = Style::Checkbox;
        assert_ne!(fingerprint_chapter(&restyled, 0).unwrap(), fp1, "样式变了指纹要变");

        let mut with_answer = b.clone();
        with_answer.entries[0].answer = Some(Answer { text: "AI 说的话".into(), backend: "x".into(), at: 0, brief: String::new() });
        assert_ne!(fingerprint_chapter(&with_answer, 0).unwrap(), fp1, "AI 回答变了也算数");

        let mut with_draft_only = b.clone();
        with_draft_only.entries[0].text = None;
        with_draft_only.entries[0].drafts.push(Draft { text: "草稿文本".into(), backend: "x".into(), at: 0, hash: "h".into() });
        assert_ne!(fingerprint_chapter(&with_draft_only, 0).unwrap(), fp1, "校对文本换成草稿，display_text 变了指纹也要变");

        // quote 是 Option<Quote>，构造函数里没给，补一个验证也参与指纹
        let mut with_quote = b.clone();
        with_quote.entries[0].quote = Some(Quote { id: "q".into(), text: "原文摘录".into(), color: "yellow".into(), rects: vec![] });
        assert_ne!(fingerprint_chapter(&with_quote, 0).unwrap(), fp1, "勾画原文变了指纹也要变");
    }
}
