//! 条目库 → Markdown 导出（纯函数，零 I/O）：`vault/<书名>/第N章 章名.md`（一章一文件，跟设备笔记本
//! 同构，文件名格式照抄 `note-serve::publish` 的 `visibleName` 命名）+ `vault/<书名>/书名.md`（索引页，
//! 列各有内容的章、反链回去）。跟 `project.rs`（条目库 → 设备笔记本投影）用同一批"live 条目"、同一套
//! 排布逻辑（按页序平铺，见 `project.rs` 模块文档"2026-09-08 三期：砍掉分区"），只是产物是纯文本
//! Markdown 不是 `.rm` 段落——给 Obsidian 用：稳定 `Entry.id`（16 位小写 hex，天然合法 Obsidian 块 id）
//! 当块锚 `^id`，改字段按 id 幂等覆盖同一行，不会因为重新导出而产生重复块或丢失反链。
//!
//! ⚠️ 已知限制（跟 `project.rs` 记的是同一类问题，这次同样不解决）：CommonMark 严格实现下，有序列表
//! （`Style::Numbered`）条目之间如果夹着摘录/回答的引用块，可能不被认作连续列表、编号从 1 重来——
//! 真要连续编号，条目间不能有摘录/回答。多数 Markdown 渲染器（含 Obsidian）对这个更宽容，先不处理。
use crate::hash::{fnv1a, hex};
use crate::model::{Book, Entry, Status, Style};

/// YAML 双引号字符串字面量（转义反斜杠与双引号；标题/书名可能含冒号、井号等 YAML 特殊字符，
/// 不加引号会被解析错）。
fn yaml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// 参与导出的条目：本章、真被要求转笔记的（`Pending`/`Draft`/`Reviewed`，跟 `project::live_entries`
/// 同一判据——2026-09-08 真机测试亲眼看见一条 `Skipped`（"不需要"）混进导出文件才发现之前 `!=
/// Revoked` 是错的，见白皮书 §03r）、去处要 Obsidian（`Obsidian`/`Both`——三期新增，见
/// `model::Destination`），按页序排；跟 `project::live_entries` 只有去处过滤方向相反
/// （那边收 `wants_notebook()`，这边收 `wants_obsidian()`），两条投影路径本就该看到不同的条目集合。
fn live_entries(book: &Book, chapter_idx: usize) -> Vec<&Entry> {
    let mut v: Vec<&Entry> = book.entries.iter().filter(|e| e.chapter == Some(chapter_idx) && e.status.is_live_for_projection() && e.destination.wants_obsidian()).collect();
    v.sort_by_key(|e| e.page_index);
    v
}

/// 章文件名（不含扩展名）：`第N章 标题`，N 从 1 起——跟 `note-serve::publish` 给设备笔记本起的
/// `visibleName` 完全一致，方便人对照"这本 md 对应设备上哪本笔记本"。
pub fn chapter_stem(chapter_idx: usize, title: &str) -> String {
    format!("第{}章 {}", chapter_idx + 1, title)
}

/// 这一章 Obsidian 导出的内容指纹——跟 `project::fingerprint_chapter` 同一套算法（标题+各条目
/// id/样式/文本/勾画/回答拼起来算哈希），只是这边的 `live_entries` 收 `wants_obsidian()` 不是
/// `wants_notebook()`：两条投影各看各的活条目集合，指纹自然也该分开算，不能共用 `project.rs`
/// 那份（同一条目改了去处，比如从 `Notebook` 切到 `Obsidian`，两边该不该判"变了"是不一样的）。
/// 没有任何该导出的条目 → `None`（跟 `export_chapter_md` 的"空章不落文件"是同一个判据）。
/// **用途**（整理区第三轮反馈，2026-09-08）：`note-serve::export.rs` 用它判断"跟上次导出比有没有
/// 变化"，没变就不重写文件、也不用刷新"已同步"记录——同一套"指纹没变就跳过"的纪律搬到导出这边。
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

fn push_entry_md(out: &mut String, e: &Entry) {
    let text = e.display_text().unwrap_or("（待转写）");
    let anchor = format!(" ^{}", e.id);
    let indent = match e.style {
        Style::Body => {
            out.push_str(text);
            out.push_str(&anchor);
            out.push('\n');
            ""
        }
        Style::Bullet => {
            out.push_str(&format!("- {text}{anchor}\n"));
            "  "
        }
        Style::Numbered => {
            out.push_str(&format!("1. {text}{anchor}\n"));
            "  "
        }
        Style::Checkbox => {
            out.push_str(&format!("- [ ] {text}{anchor}\n"));
            "  "
        }
    };
    if let Some(q) = &e.quote {
        if !q.text.trim().is_empty() {
            out.push_str(&format!("{indent}> 「{}」\n", q.text));
        }
    }
    if let Some(a) = &e.answer {
        if !a.text.trim().is_empty() {
            out.push_str(&format!("{indent}> **AI**：{}\n", a.text));
        }
    }
    out.push('\n');
}

/// 投影一章的 Markdown 全文。`None` = 没有这一章、或这一章没有可导出的条目（跟 `project_chapter`
/// 同一判据——调用方不该为空章落盘一个空文件）。
pub fn export_chapter_md(book: &Book, chapter_idx: usize) -> Option<String> {
    let title = book.chapters.get(chapter_idx)?;
    let entries = live_entries(book, chapter_idx);
    if entries.is_empty() {
        return None;
    }
    let pages: Vec<usize> = entries.iter().map(|e| e.page_index + 1).collect();
    let (lo, hi) = (*pages.iter().min().unwrap(), *pages.iter().max().unwrap());
    let all_reviewed = entries.iter().all(|e| e.status == Status::Reviewed);

    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("book: {}\n", yaml_str(&book.title)));
    if !book.author.is_empty() {
        out.push_str(&format!("author: {}\n", yaml_str(&book.author)));
    }
    out.push_str(&format!("chapter: {}\n", yaml_str(title)));
    let pages_str = if lo == hi { lo.to_string() } else { format!("{lo}–{hi}") };
    out.push_str(&format!("pages: {}\n", yaml_str(&pages_str)));
    out.push_str(&format!("status: {}\n", if all_reviewed { "reviewed" } else { "draft" }));
    out.push_str("tags: [reading-notes]\n");
    out.push_str("---\n\n");
    out.push_str(&format!("[[{}]]\n\n", book.title));

    for e in entries {
        push_entry_md(&mut out, e);
    }
    Some(out)
}

/// 书索引页：列每一章有内容的（跟 `export_chapter_md` 同一判据），反链各章文件；没有任何一章有
/// 内容时 `None`（调用方不该为空书落盘索引页）。
pub fn export_index_md(book: &Book) -> Option<String> {
    let chapters: Vec<(usize, &String)> = book.chapters.iter().enumerate().filter(|(i, _)| !live_entries(book, *i).is_empty()).collect();
    if chapters.is_empty() {
        return None;
    }
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("book: {}\n", yaml_str(&book.title)));
    if !book.author.is_empty() {
        out.push_str(&format!("author: {}\n", yaml_str(&book.author)));
    }
    out.push_str("tags: [reading-notes]\n---\n\n");
    out.push_str(&format!("# {}\n\n", book.title));
    for (idx, title) in chapters {
        out.push_str(&format!("- [[{}]]\n", chapter_stem(idx, title)));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Answer, Book, Quote};

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
            source: Default::default(),
            created: 0,
            updated: 0,
        }
    }

    fn book() -> Book {
        Book {
            uuid: "u".into(),
            title: "人骨拼图".into(),
            author: "杰佛瑞·迪佛".into(),
            chapters: vec!["第一章".into(), "空章".into()],
            entries: vec![
                entry("e1", 0, 2, Style::Body, "林肯·莱姆"),
                entry("e2", 0, 1, Style::Bullet, "为什么是纽约"),
                entry("e3", 0, 5, Style::Body, "没归类的一条"),
            ],
            page_mtimes: Default::default(),
        }
    }

    #[test]
    fn no_chapter_or_empty_chapter_yields_none() {
        let b = book();
        assert!(export_chapter_md(&b, 9).is_none(), "没有第 9 章");
        assert!(export_chapter_md(&b, 1).is_none(), "空章没条目");
    }

    #[test]
    fn front_matter_has_book_author_chapter_pages_status_tags() {
        let b = book();
        let md = export_chapter_md(&b, 0).unwrap();
        assert!(md.starts_with("---\n"));
        assert!(md.contains("book: \"人骨拼图\"\n"));
        assert!(md.contains("author: \"杰佛瑞·迪佛\"\n"));
        assert!(md.contains("chapter: \"第一章\"\n"));
        assert!(md.contains("pages: \"2–6\"\n"), "page_index 0 起，展示时 +1；三条 1/2/5 → 页 2/3/6，范围 2–6: {md}");
        assert!(md.contains("status: reviewed\n"), "三条全是 Reviewed");
        assert!(md.contains("tags: [reading-notes]\n"));
        assert!(md.contains("[[人骨拼图]]\n"), "反链回书索引页");
    }

    #[test]
    fn status_is_draft_when_any_entry_not_reviewed() {
        let mut b = book();
        b.entries[0].status = Status::Draft;
        let md = export_chapter_md(&b, 0).unwrap();
        assert!(md.contains("status: draft\n"));
    }

    #[test]
    fn entries_are_flat_and_ordered_by_page_no_section_headers() {
        let b = book();
        let md = export_chapter_md(&b, 0).unwrap();
        // 按页序：e2(page 1) 先于 e1(page 2) 先于 e3(page 5)；没有任何 "## " 分区头。
        let e2_at = md.find("为什么是纽约").unwrap();
        let e1_at = md.find("林肯·莱姆").unwrap();
        let e3_at = md.find("没归类的一条").unwrap();
        assert!(e2_at < e1_at && e1_at < e3_at);
        assert!(!md.contains("##"), "三期砍掉分区之后不该再有任何分区头: {md}");
        assert!(md.contains("林肯·莱姆 ^e1\n"), "Body 样式无列表标记，块锚跟在文本后: {md}");
        assert!(md.contains("- 为什么是纽约 ^e2\n"), "Bullet 样式加 - 前缀");
        assert!(md.contains("没归类的一条 ^e3\n"));
    }

    #[test]
    fn quote_and_answer_render_as_indented_blockquotes() {
        let mut b = book();
        b.entries[0].quote = Some(Quote { id: "q1".into(), text: "原文摘录".into(), color: "yellow".into(), rects: vec![] });
        b.entries[0].answer = Some(Answer { text: "AI 的回答".into(), backend: "x".into(), at: 0, brief: "问题".into() });
        let md = export_chapter_md(&b, 0).unwrap();
        assert!(md.contains("> 「原文摘录」\n"));
        assert!(md.contains("> **AI**：AI 的回答\n"));
        // Body 样式没有列表缩进，引用块顶格。
        assert!(!md.contains("  > 「原文摘录」"), "Body 条目的引用不该有列表缩进: {md}");

        let mut b2 = book();
        b2.entries[1].quote = Some(Quote { id: "q2".into(), text: "第二条原文".into(), color: "yellow".into(), rects: vec![] });
        let md2 = export_chapter_md(&b2, 0).unwrap();
        assert!(md2.contains("  > 「第二条原文」\n"), "Bullet 条目的引用要跟随列表缩进两格: {md2}");
    }

    #[test]
    fn no_text_falls_back_to_placeholder() {
        let mut b = book();
        b.entries[0].text = None;
        let md = export_chapter_md(&b, 0).unwrap();
        assert!(md.contains("（待转写） ^e1\n"));
    }

    #[test]
    fn revoked_entries_excluded() {
        let mut b = book();
        b.entries[0].status = Status::Revoked;
        let md = export_chapter_md(&b, 0).unwrap();
        assert!(!md.contains("林肯·莱姆"));
        assert!(md.contains("为什么是纽约"), "别的条目不受影响");
    }

    #[test]
    fn archived_entries_excluded_same_as_revoked() {
        let mut b = book();
        b.entries[0].status = Status::Archived;
        let md = export_chapter_md(&b, 0).unwrap();
        assert!(!md.contains("林肯·莱姆"));
    }

    /// 真机 bug（2026-09-08，见白皮书 §03r）：之前判据是 `!= Revoked`，`Skipped`（用户点了「不需要」）
    /// 会一直混进导出——真机导出这本书时亲眼看见一条"不需要"的条目出现在 md 文件里才揪出来。
    #[test]
    fn skipped_and_mined_entries_are_excluded_not_just_revoked() {
        let mut b = book();
        b.entries[0].status = Status::Skipped;
        assert!(!export_chapter_md(&b, 0).unwrap().contains("林肯·莱姆"), "「不需要」的条目不该出现在导出里");
        b.entries[0].status = Status::Mined;
        assert!(!export_chapter_md(&b, 0).unwrap().contains("林肯·莱姆"), "还没被要求转笔记的条目也不该出现");
    }

    #[test]
    fn destination_notebook_only_excludes_entry_from_obsidian_export() {
        use crate::model::Destination;
        let mut b = book();
        b.entries[0].destination = Destination::Notebook; // e1 改成"只留设备笔记本"
        let md = export_chapter_md(&b, 0).unwrap();
        assert!(!md.contains("林肯·莱姆"), "只想留设备的条目不该出现在 Obsidian 导出里");
        assert!(md.contains("为什么是纽约"), "e2 缺省 Both，不受影响");
        b.entries[0].destination = Destination::Obsidian;
        assert!(export_chapter_md(&b, 0).unwrap().contains("林肯·莱姆"), "改成只导出/缺省 Both 都该重新出现");
    }

    #[test]
    fn chapter_stem_matches_note_serve_visible_name_convention() {
        assert_eq!(chapter_stem(0, "起点"), "第1章 起点");
        assert_eq!(chapter_stem(9, "终点"), "第10章 终点");
    }

    #[test]
    fn index_lists_only_chapters_with_content_and_links_by_stem() {
        let b = book();
        let md = export_index_md(&b).unwrap();
        assert!(md.contains("book: \"人骨拼图\"\n") && md.contains("author: \"杰佛瑞·迪佛\"\n"));
        assert!(md.contains("# 人骨拼图\n"));
        assert!(md.contains("- [[第1章 第一章]]\n"));
        assert!(!md.contains("空章"), "空章没条目，不该出现在索引里");
    }

    #[test]
    fn index_is_none_when_book_has_no_content_at_all() {
        let mut b = book();
        b.entries.clear();
        assert!(export_index_md(&b).is_none());
    }

    #[test]
    fn yaml_escapes_quotes_and_backslashes_in_titles() {
        let mut b = book();
        b.title = "书名带\"引号\"和\\反斜杠".into();
        let md = export_chapter_md(&b, 0).unwrap();
        assert!(md.contains("book: \"书名带\\\"引号\\\"和\\\\反斜杠\"\n"), "{md}");
    }

    #[test]
    fn fingerprint_changes_on_content_change_and_is_scoped_to_obsidian_destined_entries() {
        let b = book();
        let fp1 = fingerprint_chapter(&b, 0).unwrap();
        assert_eq!(fingerprint_chapter(&b, 0), Some(fp1.clone()), "同样的书两次算出同一个指纹");

        let mut edited = b.clone();
        edited.entries[0].text = Some("换了校对文本".into());
        assert_ne!(fingerprint_chapter(&edited, 0).unwrap(), fp1, "校对文本变了指纹要变");

        // 去处切到 Notebook（不再要 Obsidian）→ 这条条目从这份指纹的输入集合里消失，指纹跟着变——
        // 跟 project::fingerprint_chapter 该不该联动是两回事，这条只测 export 自己这份。
        let mut moved = b.clone();
        moved.entries[0].destination = crate::model::Destination::Notebook;
        assert_ne!(fingerprint_chapter(&moved, 0).unwrap(), fp1, "去处改成不要 Obsidian 了，指纹要变");

        assert_eq!(fingerprint_chapter(&b, 1), None, "空章没有指纹");
    }
}
