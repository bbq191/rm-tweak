//! KOReader 生词本读取：`settings/vocabulary_builder.sqlite3`（内置生词本插件的数据库，2026-09-16 真机
//! 核对过路径在 `settings/` 不在 `data/`，schema 抄自
//! `plugins/vocabbuilder.koplugin/db.lua` 的 `CREATE TABLE`，2026-09-16 核实）。解析走 `sqlite_min`
//! （手写纯 Rust 只读解析器，见该模块顶部注释——`rusqlite` 在本项目交叉编译环境下链接失败，生产
//! 二进制不能依赖它）。库不存在＝用户没用过这个插件，返回空列表，不是错误。
use crate::sqlite_min::{Db, Value};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

/// `word` 在 KOReader 自己的表里是全局主键（不分书——同一个词换本书再查一次只更新复习进度，不会
/// 出现两行），`bookTitle` 是它第一次被记录时所在的那本书。
#[derive(Serialize, Debug, Clone, PartialEq, Default)]
pub struct VocabWord {
    pub word: String,
    pub book_title: String,
    pub prev_context: Option<String>,
    pub next_context: Option<String>,
    pub highlight: Option<String>,
    pub create_time: i64,
}

fn text(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_text).map(str::to_string)
}
fn int(v: Option<&Value>) -> Option<i64> {
    v.and_then(Value::as_int)
}

pub fn read(db_path: &Path) -> Result<Vec<VocabWord>, String> {
    if !db_path.is_file() {
        return Ok(vec![]);
    }
    let db = Db::open(db_path)?;
    // title: (id, name, filter) —— id 是 INTEGER PRIMARY KEY（rowid 别名，见 sqlite_min::table_rows 的说明）。
    let titles = db.table_rows("title", Some(0))?;
    let title_name: BTreeMap<i64, String> = titles.iter().filter_map(|r| Some((int(r.first())?, text(r.get(1))?))).collect();
    // vocabulary: (word, title_id, create_time, review_time, due_time, review_count, prev_context, next_context, streak_count, highlight)
    let vocab = db.table_rows("vocabulary", None)?;
    let mut out: Vec<VocabWord> = vocab
        .into_iter()
        .filter_map(|r| {
            let word = text(r.first())?;
            let book_title = int(r.get(1)).and_then(|id| title_name.get(&id).cloned()).unwrap_or_default();
            let create_time = int(r.get(2)).unwrap_or(0);
            Some(VocabWord { word, book_title, create_time, prev_context: text(r.get(6)), next_context: text(r.get(7)), highlight: text(r.get(9)) })
        })
        .collect();
    out.sort_by_key(|w| w.create_time);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn seed(db_path: &Path) {
        let conn = Connection::open(db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE vocabulary (word TEXT NOT NULL UNIQUE, title_id INTEGER, create_time INTEGER NOT NULL, review_time INTEGER, due_time INTEGER NOT NULL, review_count INTEGER NOT NULL DEFAULT 0, prev_context TEXT, next_context TEXT, streak_count INTEGER NOT NULL DEFAULT 0, highlight TEXT, PRIMARY KEY(word));
             CREATE TABLE title (id INTEGER NOT NULL UNIQUE, name TEXT UNIQUE, filter INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(id));
             INSERT INTO title (id, name) VALUES (1, '人骨拼图');
             INSERT INTO vocabulary (word, title_id, create_time, due_time, prev_context, next_context, highlight) VALUES ('ephemeral', 1, 100, 200, 'that was an ', ' moment.', 'ephemeral');
             INSERT INTO vocabulary (word, title_id, create_time, due_time) VALUES ('lucid', 1, 50, 150);",
        )
        .unwrap();
    }

    #[test]
    fn read_joins_title_and_orders_by_create_time() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("vocabulary_builder.sqlite3");
        seed(&db);
        let words = read(&db).unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!((words[0].word.as_str(), words[0].book_title.as_str()), ("lucid", "人骨拼图"), "按 create_time 升序，50 在 100 前面");
        assert_eq!(words[1].word, "ephemeral");
        assert_eq!(words[1].prev_context.as_deref(), Some("that was an "));
        assert!(words[0].prev_context.is_none());
    }

    #[test]
    fn missing_db_is_empty_not_error() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(read(&t.path().join("nope.sqlite3")).unwrap(), vec![]);
    }
}
