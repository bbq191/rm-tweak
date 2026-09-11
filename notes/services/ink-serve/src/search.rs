//! 笔记全文搜索（`GET /search?q=`）：跨所有书，搜勾画原文、定稿文本、最新转写草稿、提问与 AI 回答、书名。
//! 书少、条目少（个人笔记量级），每次全扫条目库（`BookDb::list`，文件没变就用解析缓存），不建索引。已撤销（`Revoked`）的条目不搜。
use notecore::model::{Book, Entry, Status};
use std::borrow::Borrow;
use serde::Serialize;

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Hit {
    pub uuid: String,
    pub title: String,
    pub id: String,
    pub page_index: usize,
    pub chapter_title: String,
    pub status: Status,
    /// 命中的是哪一栏：quote / text / draft / question / answer / title。
    pub field: &'static str,
    /// 命中处前后各约 [`CONTEXT`] 个字的片段（前端负责高亮）。
    pub snippet: String,
}

/// 片段里命中词前后各保留多少个字。
const CONTEXT: usize = 30;

/// 不区分大小写的子串查找，返回字符下标。
/// 整段小写一次、`str::find` 找字节位置再折回字符下标（此前逐字段建两份 `Vec<char>` 再滑窗比较，
/// 3000 条目全扫一次在 host 上要 7ms，2026-09-24 改）。查询词同样走 `str::to_lowercase`，两边规则一致。
fn find_ci(hay: &str, needle_lower: &str) -> Option<usize> {
    let hay_lower = hay.to_lowercase();
    let byte = hay_lower.find(needle_lower)?;
    // to_lowercase 可能一变多（极少见的字符）；那种情况下字符下标会错位，退回整段开头当片段，不影响"命中"本身。
    let aligned = hay.is_ascii() || hay_lower.chars().count() == hay.chars().count();
    Some(if aligned { hay_lower[..byte].chars().count() } else { 0 })
}

fn snippet(text: &str, at: usize, len: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let from = at.saturating_sub(CONTEXT);
    let to = (at + len + CONTEXT).min(chars.len());
    let mut s = String::new();
    if from > 0 {
        s.push('…');
    }
    s.extend(chars[from..to].iter().map(|&c| if c == '\n' { ' ' } else { c }));
    if to < chars.len() {
        s.push('…');
    }
    s
}

fn fields(e: &Entry) -> [(&'static str, Option<&str>); 5] {
    [
        ("quote", e.quote.as_ref().map(|q| q.text.as_str())),
        ("text", e.text.as_deref()),
        // 草稿是新的在前（转写与手动写回都 insert(0, …)），搜最新那份，与 Entry::display_text 一致。
        ("draft", if e.text.is_none() { e.drafts.first().map(|d| d.text.as_str()) } else { None }),
        ("question", e.question.as_deref()),
        ("answer", e.answer.as_ref().map(|a| a.text.as_str())),
    ]
}

/// 在 `books` 里搜 `q`（去首尾空白；空串不搜），每条条目只报第一处命中，最多 `limit` 条。
/// 书名命中时报该书第一条活条目（标 `title`），让用户能跳到这本书。
pub fn search<B: Borrow<Book>>(books: &[B], q: &str, limit: usize) -> Vec<Hit> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return vec![];
    }
    let qlen = q.chars().count();
    let mut out = Vec::new();
    for b in books {
        let b: &Book = b.borrow();
        let live: Vec<&Entry> = b.entries.iter().filter(|e| e.status != Status::Revoked).collect();
        let hit = |e: &Entry, field, snippet| Hit { uuid: b.uuid.clone(), title: b.title.clone(), id: e.id.clone(), page_index: e.page_index, chapter_title: e.chapter_title.clone(), status: e.status, field, snippet };
        let mut any = false;
        for e in &live {
            if let Some((field, text, at)) = fields(e).into_iter().find_map(|(f, t)| t.and_then(|t| find_ci(t, &q).map(|at| (f, t, at)))) {
                out.push(hit(e, field, snippet(text, at, qlen)));
                any = true;
                if out.len() >= limit {
                    return out;
                }
            }
        }
        if !any {
            if let (Some(at), Some(e)) = (find_ci(&b.title, &q), live.first()) {
                out.push(hit(e, "title", snippet(&b.title, at, qlen)));
                if out.len() >= limit {
                    return out;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use notecore::model::{Answer, Quote};

    fn entry(id: &str, status: Status) -> Entry {
        Entry { id: id.into(), page: "p".into(), page_index: 3, chapter: None, chapter_title: "第一章".into(), subhead: None, quote: None, ink: None, drafts: vec![], text: None, style: Default::default(), ask_ai: false, question: None, answer: None, status, destination: Default::default(), source: Default::default(), created: 0, updated: 0 }
    }

    fn book() -> Book {
        let mut a = entry("a", Status::Reviewed);
        a.quote = Some(Quote { id: String::new(), text: "The Quick brown fox jumps over the lazy dog".into(), color: "yellow".into(), rects: vec![] });
        let mut b = entry("b", Status::Draft);
        b.answer = Some(Answer { text: "甲午战争爆发于1894年".into(), backend: "x".into(), at: 0, brief: String::new() });
        let mut gone = entry("gone", Status::Revoked);
        gone.text = Some("甲午 已撤销".into());
        Book { uuid: "u1".into(), title: "战争史".into(), entries: vec![a, b, gone], ..Default::default() }
    }

    #[test]
    fn finds_across_fields_case_insensitive_and_skips_revoked() {
        let books = [book()];
        let h = search(&books, "quick", 10);
        assert_eq!((h.len(), h[0].id.as_str(), h[0].field), (1, "a", "quote"));
        assert!(h[0].snippet.starts_with("The Quick"), "{}", h[0].snippet);
        let h = search(&books, " 甲午 ", 10);
        assert_eq!(h.iter().map(|x| (x.id.as_str(), x.field)).collect::<Vec<_>>(), [("b", "answer")], "撤销的条目不搜");
        assert!(search(&books, "  ", 10).is_empty());
        assert!(search(&books, "不存在的词", 10).is_empty());
    }

    #[test]
    fn title_hit_points_to_first_live_entry_and_limit_applies() {
        let books = [book()];
        let h = search(&books, "战争史", 10);
        assert_eq!((h.len(), h[0].field, h[0].id.as_str()), (1, "title", "a"));
        let h = search(&books, "o", 1);
        assert_eq!(h.len(), 1);
    }

    /// 回归：草稿新的在前，只搜最新那份（此前误用 last() 搜到最早的草稿）。
    #[test]
    fn searches_newest_draft_only() {
        use notecore::model::Draft;
        let mut e = entry("d", Status::Draft);
        e.drafts = vec![
            Draft { text: "新草稿 甲".into(), backend: "x".into(), at: 2, hash: "h2".into() },
            Draft { text: "旧草稿 乙".into(), backend: "x".into(), at: 1, hash: "h1".into() },
        ];
        let books = [Book { uuid: "u".into(), title: "书".into(), entries: vec![e], ..Default::default() }];
        assert_eq!(search(&books, "甲", 10).len(), 1);
        assert!(search(&books, "乙", 10).is_empty(), "过时的旧草稿不该被搜到");
    }

    #[test]
    fn snippet_trims_long_text_with_ellipses() {
        let long = format!("{}关键词{}", "前".repeat(50), "后".repeat(50));
        let s = snippet(&long, 50, 3);
        assert!(s.starts_with('…') && s.ends_with('…') && s.contains("关键词"), "{s}");
        assert_eq!(s.chars().count(), 2 + CONTEXT * 2 + 3);
    }
}
