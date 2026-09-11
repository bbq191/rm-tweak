//! KOReader 高亮/生词回流：拉 koreader-serve 的原始数据（`annot.rs`/`vocab.rs` 的产出）→
//! `notecore::koreader` 的合并规则 → 条目库。跟 xochitl 摄取（`ingest.rs`）平行的第二条摄取入口，
//! 落同一个 `BookDb`（ink-serve 仍是条目库唯一写者，这条线也走同一个进程内互斥锁）。见笔记线白皮书 §03al。
use crate::bookdb::BookDb;
use notecore::hash::{fnv1a, hex};
use notecore::koreader::{merge_highlights, merge_titles, merge_vocab, MergeStats, RawHighlight, RawVocabWord};
use notecore::model::Book;
use rmsvc_core::paths::Paths;
use rmsvc_core::registry::SvcClient;
use serde::Deserialize;

/// koreader-serve `GET /annotations` 里一条 `annotations` 原始项——字段名照抄 KOReader 自己的
/// `annotations` 表（见 `koreader-serve::annot::RawItem`，两边故意同形状，不加 `rename_all`）。
#[derive(Deserialize, Debug, Clone, Default)]
struct WireAnnotation {
    text: Option<String>,
    note: Option<String>,
    chapter: Option<String>,
    datetime: Option<String>,
    color: Option<String>,
    pos0: Option<serde_json::Value>,
    pos1: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub(crate) struct WireBookAnnotations {
    path: String,
    title: String,
    items: Vec<WireAnnotation>,
}

#[derive(Deserialize, Debug, Clone, Default)]
pub(crate) struct WireVocabWord {
    word: String,
    book_title: String,
    prev_context: Option<String>,
    next_context: Option<String>,
    highlight: Option<String>,
}

pub trait KoreaderSource: Send + Sync {
    fn annotations(&self) -> Result<Vec<WireBookAnnotations>, String>;
    fn vocabulary(&self) -> Result<Vec<WireVocabWord>, String>;
}

pub struct KoreaderHttp(SvcClient);

impl KoreaderHttp {
    pub fn new(paths: Paths) -> KoreaderHttp {
        KoreaderHttp(SvcClient::new(paths, "koreader-serve", 30))
    }
}

impl KoreaderSource for KoreaderHttp {
    fn annotations(&self) -> Result<Vec<WireBookAnnotations>, String> {
        let v = self.0.get_json("/annotations")?;
        serde_json::from_value(v.get("items").cloned().unwrap_or_default()).map_err(|e| format!("annotations 形状不对: {e}"))
    }
    fn vocabulary(&self) -> Result<Vec<WireVocabWord>, String> {
        let v = self.0.get_json("/vocabulary")?;
        serde_json::from_value(v.get("items").cloned().unwrap_or_default()).map_err(|e| format!("vocabulary 形状不对: {e}"))
    }
}

/// 高亮认领键：(书, pos0, pos1, datetime) 哈希——annotations 数组本身没有天然 id，`pos0`/`pos1` 对
/// EPUB 是 xpointer 字符串、对 PDF 是坐标对象，统一按 JSON 文本形式喂哈希，不关心具体形状。
fn highlight_id(book_path: &str, a: &WireAnnotation) -> String {
    let pos = format!("{}|{}", a.pos0.as_ref().map(ToString::to_string).unwrap_or_default(), a.pos1.as_ref().map(ToString::to_string).unwrap_or_default());
    hex(fnv1a(format!("{book_path}|{pos}|{}", a.datetime.as_deref().unwrap_or("")).as_bytes()))
}

/// `Book.uuid`：不能直接拿 `books/` 下的相对路径当 uuid——`BookDb::path()` 拿 uuid 原样拼文件名，
/// 路径里的 `/` 会被当成目录分隔符（真实建出嵌套目录，`list()`/`list_active()` 只读一层，直接找不到，
/// 2026-09-16 本地端到端冒烟测试时发现的坑）；uuid 同时也走 URL 路径段（`GET /books/{uuid}`），带 `/`
/// 还会把路由拆成两段。哈希成定长十六进制彻底绕开这两个问题——人类可读的书名已经存在 `Book.title` 里。
fn book_uuid(relpath: &str) -> String {
    format!("koreader:{}", hex(fnv1a(relpath.as_bytes())))
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportStats {
    pub highlight_books: usize,
    pub highlights: MergeStats,
    pub vocab: MergeStats,
}

/// 拉一遍 koreader-serve 的高亮+生词、按规则并入 `BookDb`。
/// 高亮：一本 KOReader 书一个 `Book` 记录（`uuid = "koreader:<books/ 下的相对路径>"`——KOReader 书
/// 本身没有 uuid，用相对路径当稳定标识，见白皮书 §03al）；`Book.chapters` 按 annotations 数组给的
/// 阅读顺序去重出真实章节名，每条高亮的 `chapter` 落到章节表里的下标（`export`/`project` 两处投影
/// 都靠这个下标分组，缺了条目永远导不出去，见下方 `merge_highlights` 调用点注释）。
/// 生词：全局一份合集 `Book`（`uuid = "koreader-vocab"`）——KOReader 自己的生词表按 `word` 全局去重、
/// 不分书存（同一个词换本书查一次只更新复习进度），这边跟着不拆分；`chapters` 借来存来源书名分组。
pub fn import(db: &BookDb, src: &dyn KoreaderSource, now: u64) -> Result<ImportStats, String> {
    let mut st = ImportStats::default();

    for b in src.annotations()? {
        let uuid = book_uuid(&b.path);
        let items: Vec<RawHighlight> = b
            .items
            .iter()
            .enumerate()
            .filter_map(|(order, a)| {
                let text = a.text.as_deref()?;
                if text.trim().is_empty() {
                    return None;
                }
                Some(RawHighlight { id: highlight_id(&b.path, a), text, note: a.note.as_deref(), chapter_title: a.chapter.as_deref(), color: a.color.as_deref().unwrap_or(""), order })
            })
            .collect();
        if items.is_empty() {
            continue;
        }
        // 按 annotations 数组给的阅读顺序去重出章节列表（不排序——"第一章"/"第十章" 字典序会乱掉，
        // 阅读顺序才是自然顺序）；`export`/`project` 两处投影都按 `entry.chapter == Some(idx)` 匹配
        // `Book.chapters[idx]`，`chapter` 一直是 `None` 的条目找不到自己该落的章节，2026-09-16 真机
        // 验证时发现的缺口——能被点到 `Reviewed`，但导出永远 0 文件。
        let mut chapters: Vec<String> = Vec::new();
        for it in &items {
            let ct = it.chapter_title.unwrap_or_default().to_string();
            if !chapters.contains(&ct) {
                chapters.push(ct);
            }
        }
        let stats = db.update(&uuid, || Book { uuid: uuid.clone(), title: b.title.clone(), ..Default::default() }, |book| {
            book.title = b.title.clone();
            // 只追加不重排，已有条目的章下标保持有效（见 `merge_titles`）。
            book.chapters = merge_titles(&book.chapters, chapters.iter().cloned());
            let chapters = &book.chapters;
            merge_highlights(&mut book.entries, &items, |t| chapters.iter().position(|c| c == t).unwrap_or(0), now)
        })?;
        st.highlight_books += 1;
        st.highlights.added += stats.added;
        st.highlights.unchanged += stats.unchanged;
        st.highlights.revoked += stats.revoked;
    }

    let words = src.vocabulary()?;
    if !words.is_empty() {
        let mut titles: Vec<String> = words.iter().map(|w| w.book_title.clone()).collect();
        titles.sort();
        titles.dedup();
        let raw: Vec<RawVocabWord> = words.iter().map(|w| RawVocabWord { word: &w.word, book_title: &w.book_title, prev_context: w.prev_context.as_deref(), next_context: w.next_context.as_deref(), highlight: w.highlight.as_deref() }).collect();
        const UUID: &str = "koreader-vocab";
        st.vocab = db.update(
            UUID,
            || Book { uuid: UUID.into(), title: "KOReader 生词本".into(), ..Default::default() },
            |book| {
                book.chapters = merge_titles(&book.chapters, titles.iter().cloned());
                let chapters = &book.chapters;
                merge_vocab(&mut book.entries, &raw, |t| chapters.iter().position(|x| x == t).unwrap_or(0), now)
            },
        )?;
    }

    Ok(st)
}

#[cfg(test)]
mod tests {
    use super::*;
    use notecore::model::{Source, Status};

    struct Fake {
        annotations: Vec<WireBookAnnotations>,
        vocab: Vec<WireVocabWord>,
    }
    impl KoreaderSource for Fake {
        fn annotations(&self) -> Result<Vec<WireBookAnnotations>, String> {
            Ok(self.annotations.clone())
        }
        fn vocabulary(&self) -> Result<Vec<WireVocabWord>, String> {
            Ok(self.vocab.clone())
        }
    }

    /// 每条测试标注要有各自不同的 pos0/pos1（`highlight_id` 的哈希输入），不然两条不同文字的标注会撞到
    /// 同一个认领 id——真实 KOReader 数据里位置天然不同，这里用 `text` 顺手掺进 pos0 保证测试里也不同。
    fn ann(text: &str) -> WireAnnotation {
        WireAnnotation { text: Some(text.into()), chapter: Some("第一章".into()), color: Some("yellow".into()), datetime: Some("2026-09-16 10:00:00".into()), pos0: Some(serde_json::json!(format!("/p[{text}]"))), pos1: Some(serde_json::json!("/p[2]")), note: None }
    }

    #[test]
    fn import_creates_one_book_per_koreader_book_keyed_by_hashed_relpath() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        let src = Fake { annotations: vec![WireBookAnnotations { path: "小说/人骨拼图.epub".into(), title: "人骨拼图".into(), items: vec![ann("第一句"), ann("第二句")] }], vocab: vec![] };
        let stats = import(&db, &src, 10).unwrap();
        assert_eq!((stats.highlight_books, stats.highlights.added), (1, 2));
        let uuid = book_uuid("小说/人骨拼图.epub");
        assert!(!uuid.contains('/'), "uuid 落文件名+走 URL 路径段，不能带 /（2026-09-16 端到端冒烟测试踩过）");
        let book = db.load(&uuid).unwrap();
        assert_eq!(book.title, "人骨拼图");
        assert_eq!(book.entries.len(), 2);
        assert!(book.entries.iter().all(|e| e.source == Source::KoreaderHighlight && e.status == Status::Mined));
        assert_eq!(book.chapters, vec!["第一章".to_string()], "章节表要真的落进 Book.chapters");
        assert!(book.entries.iter().all(|e| e.chapter == Some(0)), "chapter 必须是 Some——真机验证发现过 chapter 一直是 None 导致导出 0 文件的缺口");

        // 重扫：同样的输入，不重复新建。
        let stats2 = import(&db, &src, 20).unwrap();
        assert_eq!(stats2.highlights, MergeStats { unchanged: 2, ..Default::default() });
    }

    /// 回归（2026-09-25）：新高亮落在更靠前的章时，已有条目的章下标不能移位（note-serve 按下标记着每章的笔记本）。
    #[test]
    fn chapter_indices_stay_stable_when_an_earlier_chapter_appears_later() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        let mut a2 = ann("第二章的话");
        a2.chapter = Some("第二章".into());
        let src = Fake { annotations: vec![WireBookAnnotations { path: "b.epub".into(), title: "书".into(), items: vec![a2.clone()] }], vocab: vec![] };
        import(&db, &src, 10).unwrap();
        let uuid = book_uuid("b.epub");
        assert_eq!(db.load(&uuid).unwrap().entries[0].chapter, Some(0));
        let src = Fake { annotations: vec![WireBookAnnotations { path: "b.epub".into(), title: "书".into(), items: vec![ann("第一章的话"), a2] }], vocab: vec![] };
        import(&db, &src, 20).unwrap();
        let book = db.load(&uuid).unwrap();
        assert_eq!(book.chapters, ["第二章", "第一章"]);
        let ch = |text: &str| book.entries.iter().find(|e| e.quote.as_ref().unwrap().text == text).unwrap().chapter;
        assert_eq!((ch("第二章的话"), ch("第一章的话")), (Some(0), Some(1)), "旧条目下标不动");
    }

    #[test]
    fn import_books_with_only_bookmark_style_annotations_are_skipped_entirely() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        let mut empty_text = ann("");
        empty_text.text = Some("   ".into());
        let src = Fake { annotations: vec![WireBookAnnotations { path: "空.epub".into(), title: "空".into(), items: vec![empty_text] }], vocab: vec![] };
        let stats = import(&db, &src, 10).unwrap();
        assert_eq!(stats.highlight_books, 0, "全是空文字书签的书不该建 Book 记录");
        assert!(db.load("koreader:空.epub").is_none());
    }

    #[test]
    fn import_vocab_lands_in_one_global_book_grouped_by_source_title() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        let src = Fake {
            annotations: vec![],
            vocab: vec![
                WireVocabWord { word: "ephemeral".into(), book_title: "书A".into(), prev_context: Some("that was an ".into()), next_context: Some(" moment.".into()), highlight: Some("ephemeral".into()) },
                WireVocabWord { word: "lucid".into(), book_title: "书B".into(), ..Default::default() },
            ],
        };
        let stats = import(&db, &src, 10).unwrap();
        assert_eq!(stats.vocab.added, 2);
        let book = db.load("koreader-vocab").unwrap();
        assert_eq!(book.title, "KOReader 生词本");
        assert_eq!(book.chapters, vec!["书A".to_string(), "书B".to_string()]);
        assert!(book.entries.iter().all(|e| e.source == Source::KoreaderVocab));
        assert_eq!(book.entries[0].quote.as_ref().unwrap().text, "that was an 『ephemeral』 moment.");
    }
}
