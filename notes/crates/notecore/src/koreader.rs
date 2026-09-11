//! KOReader 高亮/生词 → 条目：跟 xochitl 摄取（`ingest.rs`）平行的另一条入口，产出同一个 [`Entry`] 形状。
//! 两条摄取线都没有笔画坐标——`ink` 永远 `None`，天然走 `Entry::set_triage` 已有的"纯勾画直接定稿"快
//! 路径（浏览态点「转入笔记」直接落定，不经过 `Pending`/`Draft`），不需要为它们单独加状态机分支或改
//! 浏览态前端。零 I/O：调用方（ink-serve）负责读 koreader-serve 吐的原始 JSON、算好稳定 id，这里只管
//! "已经拆好的字段 → Entry / 增量合并规则"。见笔记线白皮书 §03al。
use crate::model::{Destination, Entry, Quote, Source, Status, Style};

/// 一条高亮（调用方已经从 koreader-serve 的 annotations JSON 里拆好字段）。`id` 是调用方按
/// (书, pos0, pos1, datetime) 算好的稳定认领键——annotations 数组本身没有天然 id，认领只能靠内容。
pub struct RawHighlight<'a> {
    /// 拥有所有权：调用方每次都是现算（哈希），不是借某个已有字符串，跟 `text`/`note`/`chapter_title`
    /// 这些"确实是从反序列化出来的 JSON 借用"的字段不是一回事。
    pub id: String,
    pub text: &'a str,
    pub note: Option<&'a str>,
    pub chapter_title: Option<&'a str>,
    pub color: &'a str,
    /// 摄取时在 annotations 数组里的位置：数组本身就是 KOReader 给的阅读顺序，直接拿来当
    /// `page_index` 用于章内排序，不需要另外解析 xpointer/pageno。
    pub order: usize,
}

/// 一个生词（调用方已经从 vocabulary_builder.sqlite3 的行拆好字段）。`word` 在 KOReader 自己的表里
/// 就是主键（全局唯一，不分书——同一个词换本书再查一次只是更新复习进度，见白皮书 §03al 的踩坑记录），
/// 直接拿来当认领 id 用。
pub struct RawVocabWord<'a> {
    pub word: &'a str,
    pub book_title: &'a str,
    pub prev_context: Option<&'a str>,
    pub next_context: Option<&'a str>,
    /// 原文里这个词的实际写法（大小写/变位可能跟 `word` 不同）。
    pub highlight: Option<&'a str>,
}

fn quote_text(rh: &RawHighlight) -> String {
    match rh.note {
        Some(n) if !n.trim().is_empty() => format!("{}\n\n> {}", rh.text, n.trim()),
        _ => rh.text.to_string(),
    }
}

fn vocab_quote_text(v: &RawVocabWord) -> String {
    let word = v.highlight.filter(|s| !s.is_empty()).unwrap_or(v.word);
    match (v.prev_context, v.next_context) {
        (Some(p), Some(n)) if !p.is_empty() || !n.is_empty() => format!("{p}『{word}』{n}"),
        _ => word.to_string(),
    }
}

fn new_entry(id: String, page_index: usize, chapter_title: String, quote: Quote, source: Source, now: u64) -> Entry {
    Entry {
        id,
        page: String::new(),
        page_index,
        chapter: None,
        chapter_title,
        subhead: None,
        quote: Some(quote),
        ink: None,
        drafts: vec![],
        text: None,
        style: Style::Body,
        ask_ai: false,
        question: None,
        answer: None,
        status: Status::Mined,
        destination: Destination::default(),
        source,
        created: now,
        updated: now,
    }
}

/// 合并结果统计（复用 xochitl 摄取那份的字段命名习惯，日志/事件用）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MergeStats {
    pub added: usize,
    pub unchanged: usize,
    pub revoked: usize,
}

/// 把这本书当前抓到的全部高亮并入 `entries`：按 `RawHighlight.id` 认领（新的加、已有的按内容刷新，
/// KOReader 那边被删掉的——当前列表里认领不到的、且还没被用户手动处理过的——标 `Revoked`，不物理删）。
/// 只动 `source == KoreaderHighlight` 的条目，不碰同一本书里可能存在的其它来源条目（这条书 uuid 命名空间
/// 本身就只装 KOReader 高亮，见 ink-serve::koreader 的 `koreader:<relpath>` 前缀，但多一道判据更安全）。
///
/// `chapter_index_of` 必须给：`export::live_entries`/`project::live_entries` 两处投影都按
/// `entry.chapter == Some(idx)` 匹配 `Book.chapters[idx]` 分组，`chapter` 一直是 `None` 的条目
/// 在任何一处投影里都找不到自己该落的章节、永远不会被导出/生成笔记本——2026-09-16 真机验证时
/// 发现的真缺口：条目能被"转入笔记"点到 `Reviewed`，但 `POST .../export` 吐 0 文件，因为压根没有
/// 章节容得下它。调用方（ink-serve）按 `chapter_title` 去重排出 `Book.chapters` 再传这个闭包。
pub fn merge_highlights(entries: &mut Vec<Entry>, items: &[RawHighlight], chapter_index_of: impl Fn(&str) -> usize, now: u64) -> MergeStats {
    let mut st = MergeStats::default();
    let seen: std::collections::BTreeSet<&str> = items.iter().map(|h| h.id.as_str()).collect();
    for h in items {
        let chapter_title = h.chapter_title.unwrap_or_default();
        let chapter = Some(chapter_index_of(chapter_title));
        match entries.iter_mut().find(|e| e.source == Source::KoreaderHighlight && e.id == h.id) {
            Some(e) => {
                let text = quote_text(h);
                let changed = e.quote.as_ref().map(|q| q.text != text || q.color != h.color).unwrap_or(true) || e.page_index != h.order || e.chapter != chapter;
                if changed {
                    e.quote = Some(Quote { id: h.id.clone(), text, color: h.color.to_string(), rects: vec![] });
                    e.page_index = h.order;
                    e.chapter_title = chapter_title.to_string();
                    e.chapter = chapter;
                    e.updated = now;
                }
                st.unchanged += 1;
            }
            None => {
                let mut e = new_entry(h.id.clone(), h.order, chapter_title.to_string(), Quote { id: h.id.clone(), text: quote_text(h), color: h.color.to_string(), rects: vec![] }, Source::KoreaderHighlight, now);
                e.chapter = chapter;
                entries.push(e);
                st.added += 1;
            }
        }
    }
    for e in entries.iter_mut() {
        if e.source == Source::KoreaderHighlight && !seen.contains(e.id.as_str()) && !e.is_terminal() {
            e.status = Status::Revoked;
            e.updated = now;
            st.revoked += 1;
        }
    }
    st
}

/// 生词表比高亮简单：没有"位置"概念，`chapters`/`chapter_title` 借来存来源书名做分组（见 ink-serve::koreader
/// 的调用方，`chapters` 由调用方按书名去重排好序传 `chapter_index_of`）。
pub fn merge_vocab(entries: &mut Vec<Entry>, items: &[RawVocabWord], chapter_index_of: impl Fn(&str) -> usize, now: u64) -> MergeStats {
    let mut st = MergeStats::default();
    for (order, v) in items.iter().enumerate() {
        let id = crate::hash::hex(crate::hash::fnv1a(format!("koreader-vocab/{}", v.word).as_bytes()));
        match entries.iter_mut().find(|e| e.source == Source::KoreaderVocab && e.id == id) {
            Some(e) => {
                let text = vocab_quote_text(v);
                if e.quote.as_ref().map(|q| q.text != text).unwrap_or(true) {
                    e.quote = Some(Quote { id: id.clone(), text, color: String::new(), rects: vec![] });
                    e.updated = now;
                }
                st.unchanged += 1;
            }
            None => {
                let mut e = new_entry(id, order, v.book_title.to_string(), Quote { id: String::new(), text: vocab_quote_text(v), color: String::new(), rects: vec![] }, Source::KoreaderVocab, now);
                e.chapter = Some(chapter_index_of(v.book_title));
                e.quote.as_mut().unwrap().id = e.id.clone();
                entries.push(e);
                st.added += 1;
            }
        }
    }
    // 生词表按 id（词的哈希）本身判"这次还在不在"，不是原文顺序位置。
    let ids: std::collections::BTreeSet<String> = items.iter().map(|v| crate::hash::hex(crate::hash::fnv1a(format!("koreader-vocab/{}", v.word).as_bytes()))).collect();
    for e in entries.iter_mut() {
        if e.source == Source::KoreaderVocab && !ids.contains(&e.id) && !e.is_terminal() {
            e.status = Status::Revoked;
            e.updated = now;
            st.revoked += 1;
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hl<'a>(id: &str, text: &'a str, order: usize) -> RawHighlight<'a> {
        RawHighlight { id: id.to_string(), text, note: None, chapter_title: Some("第一章"), color: "yellow", order }
    }

    fn hl2<'a>(id: &str, text: &'a str, order: usize, chapter_title: &'a str) -> RawHighlight<'a> {
        RawHighlight { id: id.to_string(), text, note: None, chapter_title: Some(chapter_title), color: "yellow", order }
    }

    #[test]
    fn merge_highlights_adds_new_and_is_idempotent_on_rescan() {
        let mut entries = vec![];
        let items = vec![hl("h1", "第一句", 0), hl("h2", "第二句", 1)];
        let st = merge_highlights(&mut entries, &items, |_| 0, 10);
        assert_eq!(st, MergeStats { added: 2, ..Default::default() });
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.ink.is_none() && e.source == Source::KoreaderHighlight && e.status == Status::Mined && e.chapter == Some(0)), "chapter 必须落到 Some，不然 export/project 两处投影按 chapter 分组永远找不到这条（2026-09-16 真机验证发现的缺口）");

        // 同样的列表再合并一次：不新增，内容不变（unchanged）。
        let st2 = merge_highlights(&mut entries, &items, |_| 0, 20);
        assert_eq!(st2, MergeStats { unchanged: 2, ..Default::default() });
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn merge_highlights_finalizes_straight_to_reviewed_via_existing_set_triage() {
        let mut entries = vec![];
        merge_highlights(&mut entries, &[hl("h1", "原文", 0)], |_| 0, 10);
        entries[0].set_triage(Status::Pending, 20).unwrap();
        assert_eq!((entries[0].status, entries[0].text.as_deref()), (Status::Reviewed, Some("原文")), "纯 quote 条目直接定稿，KOReader 高亮复用这条既有快路径");
    }

    #[test]
    fn merge_highlights_revokes_ones_missing_from_a_rescan_but_leaves_terminal_alone() {
        let mut entries = vec![];
        merge_highlights(&mut entries, &[hl("h1", "还在", 0), hl("h2", "被删了", 1), hl("h3", "跳过的", 2)], |_| 0, 10);
        entries.iter_mut().find(|e| e.id == "h3").unwrap().status = Status::Skipped;
        // 重扫：h2 从 KOReader 里消失了（用户删了这条高亮）。
        let st = merge_highlights(&mut entries, &[hl("h1", "还在", 0), hl("h3", "跳过的", 2)], |_| 0, 30);
        assert_eq!(st, MergeStats { unchanged: 2, revoked: 1, ..Default::default() });
        assert_eq!(entries.iter().find(|e| e.id == "h2").unwrap().status, Status::Revoked);
        assert_eq!(entries.iter().find(|e| e.id == "h3").unwrap().status, Status::Skipped, "终态不该被重扫时的消失判定悄悄改判");
    }

    #[test]
    fn merge_highlights_groups_by_chapter_title_into_distinct_indices() {
        let mut entries = vec![];
        let items = vec![hl("h1", "第一章的话", 0), hl2("h2", "第二章的话", 0, "第二章")];
        let chapters = vec!["第一章".to_string(), "第二章".to_string()];
        let idx_of = |t: &str| chapters.iter().position(|c| c == t).unwrap_or(0);
        merge_highlights(&mut entries, &items, idx_of, 10);
        assert_eq!(entries.iter().find(|e| e.id == "h1").unwrap().chapter, Some(0));
        assert_eq!(entries.iter().find(|e| e.id == "h2").unwrap().chapter, Some(1));
    }

    #[test]
    fn merge_vocab_dedups_by_word_across_books_and_formats_context_quote() {
        let mut entries = vec![];
        let items = vec![
            RawVocabWord { word: "ephemeral", book_title: "书A", prev_context: Some("that was an "), next_context: Some(" moment."), highlight: Some("ephemeral") },
            RawVocabWord { word: "lucid", book_title: "书B", prev_context: None, next_context: None, highlight: None },
        ];
        let titles = ["书A".to_string(), "书B".to_string()];
        let st = merge_vocab(&mut entries, &items, |t| titles.iter().position(|x| x == t).unwrap_or(0), 10);
        assert_eq!(st, MergeStats { added: 2, ..Default::default() });
        assert_eq!(entries[0].quote.as_ref().unwrap().text, "that was an 『ephemeral』 moment.");
        assert_eq!(entries[1].quote.as_ref().unwrap().text, "lucid", "没有上下文时退回裸词");
        assert!(entries.iter().all(|e| e.source == Source::KoreaderVocab));

        // 重扫，词表不变：不新增。
        let st2 = merge_vocab(&mut entries, &items, |t| titles.iter().position(|x| x == t).unwrap_or(0), 20);
        assert_eq!(st2, MergeStats { unchanged: 2, ..Default::default() });
    }
}
