//! 条目库（Repository）：一书一文件 `$XDG_STATE_HOME/notes/books/<uuid>.json`，原子写。ink-serve 是**唯一写者**——
//! 转写/脑/本三服务都通过它的 HTTP 改条目字段，避免多进程同时改一份 JSON。
use notecore::model::{Book, Status};
use rmsvc_core::fs::write_atomic;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub struct BookDb {
    dir: PathBuf,
    lock: Mutex<()>,
}

impl BookDb {
    pub fn new(dir: PathBuf) -> BookDb {
        BookDb { dir, lock: Mutex::new(()) }
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }
    fn path(&self, uuid: &str) -> PathBuf {
        self.dir.join(format!("{uuid}.json"))
    }

    pub fn load(&self, uuid: &str) -> Option<Book> {
        serde_json::from_slice(&std::fs::read(self.path(uuid)).ok()?).ok()
    }

    pub fn save(&self, book: &Book) -> Result<(), String> {
        let bytes = serde_json::to_vec_pretty(book).map_err(|e| e.to_string())?;
        write_atomic(&self.path(&book.uuid), &bytes).map_err(|e| format!("写条目库失败: {e}"))
    }

    /// 读—改—写（进程内串行化）。书不存在时以 `seed()` 起。
    pub fn update<T>(&self, uuid: &str, seed: impl FnOnce() -> Book, f: impl FnOnce(&mut Book) -> T) -> Result<T, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut book = self.load(uuid).unwrap_or_else(seed);
        let out = f(&mut book);
        self.save(&book)?;
        Ok(out)
    }

    /// 所有书（按标题排序）——含条目已全部撤销的书（内部/调试用；网页列表用 `list_active`）。
    pub fn list(&self) -> Vec<Book> {
        let mut out: Vec<Book> = std::fs::read_dir(&self.dir)
            .map(|rd| rd.flatten().filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json")).filter_map(|e| serde_json::from_slice(&std::fs::read(e.path()).ok()?).ok()).collect())
            .unwrap_or_default();
        out.sort_by(|a, b| a.title.cmp(&b.title));
        out
    }

    /// 只列"现在还有活条目"的书（按标题排序）：摄取门槛看的是"页 .rm 文件存不存在"，笔画擦光了文件不会被删，
    /// 一本书清空所有勾画/手写后摄取仍会跑（正确把条目标成 Revoked），但如果不过滤，它会因为"曾经有过
    /// .rm 文件"永远赖在网页的书选择列表里——即使当下一条活条目都没有（2026-09-07 真机验证时发现）。
    pub fn list_active(&self) -> Vec<Book> {
        self.list().into_iter().filter(|b| b.entries.iter().any(|e| e.status != Status::Revoked)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notecore::model::Entry;

    fn entry(id: &str, status: Status) -> Entry {
        Entry { id: id.into(), page: "p".into(), page_index: 0, chapter: None, chapter_title: String::new(), subhead: None, quote: None, ink: None, drafts: vec![], text: None, style: Default::default(), ask_ai: false, question: None, answer: None, status, destination: Default::default(), created: 0, updated: 0 }
    }

    #[test]
    fn list_active_hides_books_whose_entries_are_all_revoked() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        db.update("live", || Book { uuid: "live".into(), title: "还有活条目".into(), ..Default::default() }, |b| b.entries.push(entry("e1", Status::Reviewed))).unwrap();
        db.update("cleared", || Book { uuid: "cleared".into(), title: "已清空".into(), ..Default::default() }, |b| b.entries.push(entry("e2", Status::Revoked))).unwrap();
        db.update("empty", || Book { uuid: "empty".into(), title: "从没标注过".into(), ..Default::default() }, |_| ()).unwrap();

        assert_eq!(db.list().len(), 3, "list() 不过滤，三本都在");
        let active = db.list_active();
        assert_eq!(active.iter().map(|b| b.uuid.as_str()).collect::<Vec<_>>(), ["live"], "已清空/从没标注过的书不该出现在 list_active");
    }

    #[test]
    fn update_seeds_saves_and_lists() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        assert!(db.load("u1").is_none());
        let n = db.update("u1", || Book { uuid: "u1".into(), title: "乙".into(), ..Default::default() }, |b| { b.chapters.push("一".into()); b.chapters.len() }).unwrap();
        assert_eq!(n, 1);
        db.update("u2", || Book { uuid: "u2".into(), title: "甲".into(), ..Default::default() }, |_| ()).unwrap();
        assert_eq!(db.load("u1").unwrap().chapters, vec!["一"]);
        assert_eq!(db.list().iter().map(|b| b.title.as_str()).collect::<Vec<_>>(), ["乙", "甲"], "按标题码位排序（乙 U+4E59 < 甲 U+7532）");
    }
}
