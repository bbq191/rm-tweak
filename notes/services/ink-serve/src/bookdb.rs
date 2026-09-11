//! 条目库（Repository）：一书一文件 `$XDG_STATE_HOME/notes/books/<uuid>.json`，原子写。ink-serve 是**唯一写者**——
//! 转写/脑/本三服务都通过它的 HTTP 改条目字段，避免多进程同时改一份 JSON。
//!
//! **解析结果按文件身份缓存**（2026-09-24 第三轮审计）：网页每收到一条 `notes` 事件就刷新一次（`GET /books`
//! 全量解析每本书 + `GET /books/{uuid}`），transcribe-serve 每次被 `entries` 事件踢醒也要 `GET /books`，一次
//! 改字会连带三四遍"读全部 JSON 再解析"。这里按 (inode, 长度, mtime) 记住上次解析出的 `Arc<Book>`：文件没变
//! 就直接给缓存，变了（含外部手改）才重新解析；本进程自己 `save` 后当场换入新值。磁盘始终是事实源——身份
//! 对不上就回退到读盘，不存在"缓存和磁盘各说各话"。
use notecore::model::{Book, Status};
use rmsvc_core::fs::{plain_name, write_atomic};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::SystemTime;

/// 文件身份：`write_atomic` 每次 rename 都换 inode，外部改写至少会动 mtime/长度。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct FileKey {
    ino: u64,
    len: u64,
    mtime: Option<SystemTime>,
}

impl FileKey {
    fn of(m: &std::fs::Metadata) -> FileKey {
        #[cfg(unix)]
        let ino = std::os::unix::fs::MetadataExt::ino(m);
        #[cfg(not(unix))]
        let ino = 0;
        FileKey { ino, len: m.len(), mtime: m.modified().ok() }
    }
}

type Cache = HashMap<String, (FileKey, Arc<Book>)>;

pub struct BookDb {
    dir: PathBuf,
    /// 读—改—写串行化（进程内）。
    lock: Mutex<()>,
    /// uuid → (文件身份, 解析结果)。只放解析成功的；坏文件每次都走读盘路径（`.corrupt` 逻辑照常触发）。
    cache: Mutex<Cache>,
}

impl BookDb {
    pub fn new(dir: PathBuf) -> BookDb {
        BookDb { dir, lock: Mutex::new(()), cache: Mutex::new(HashMap::new()) }
    }
    fn cache(&self) -> MutexGuard<'_, Cache> {
        rmsvc_core::sync::lock(&self.cache)
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }
    /// `<dir>/<uuid>.json`。uuid 来自 URL 路径参数（网关解码后可含 `/`、`..`），过 `plain_name`
    /// 单段校验，防止读写条目库目录之外的 .json。
    fn path(&self, uuid: &str) -> Result<PathBuf, String> {
        Ok(self.dir.join(format!("{}.json", plain_name(uuid)?)))
    }

    /// 读一本书：文件不存在 → `Ok(None)`；读失败或 JSON 解析失败 → `Err`。**绝不能把"坏了"当成"没有"**：
    /// 此前两者都返回 `None`，`update` 随即用 `seed()` 的空书整本覆盖掉——降级部署遇到不认识的枚举值、
    /// 文件被改坏，校对文本/AI 回答就静默全丢（2026-09-24 审查）。解析失败时另存一份 `<uuid>.json.corrupt`
    /// 副本（已有就不重复拷），原文件原样不动，后续写入一律拒绝，直到人工处理。
    /// 非法 uuid（含 `/`、`..`）同样当作不存在。返回共享只读快照（见模块文档"按文件身份缓存"）。
    pub fn read(&self, uuid: &str) -> Result<Option<Arc<Book>>, String> {
        let Ok(p) = self.path(uuid) else { return Ok(None) };
        // 先打开、再对同一个 fd 取身份并读内容：身份与内容出自同一个文件，中途被 rename 替换也不会张冠李戴。
        let mut f = match std::fs::File::open(&p) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                self.cache().remove(uuid);
                return Ok(None);
            }
            Err(e) => return Err(format!("读条目库 {uuid} 失败: {e}")),
        };
        let key = f.metadata().map(|m| FileKey::of(&m)).map_err(|e| format!("读条目库 {uuid} 失败: {e}"))?;
        if let Some((k, b)) = self.cache().get(uuid) {
            if *k == key {
                return Ok(Some(b.clone()));
            }
        }
        let mut bytes = Vec::with_capacity(key.len as usize);
        std::io::Read::read_to_end(&mut f, &mut bytes).map_err(|e| format!("读条目库 {uuid} 失败: {e}"))?;
        match serde_json::from_slice::<Book>(&bytes) {
            Ok(b) => {
                let b = Arc::new(b);
                self.cache().insert(uuid.to_string(), (key, b.clone()));
                Ok(Some(b))
            }
            Err(e) => {
                self.cache().remove(uuid);
                let msg = format!("条目库 {uuid}.json 解析失败，已另存 .corrupt 副本、原文件未动、拒绝覆盖写入: {e}");
                // 只在头一次发现（副本还没有）时打日志：`list()` 每次网页刷新都会路过它，别刷屏。
                let bak = p.with_extension("json.corrupt");
                if !bak.exists() {
                    let _ = std::fs::copy(&p, &bak);
                    eprintln!("[ink-serve] {msg}");
                }
                Err(msg)
            }
        }
    }

    #[cfg(test)]
    pub fn load(&self, uuid: &str) -> Option<Book> {
        self.read(uuid).ok().flatten().map(|b| (*b).clone())
    }

    /// 落盘，并把新值换进缓存（身份取 rename 之后的文件）。文件名用调用方的 `uuid`（读的那一份），不用书里的
    /// `uuid` 字段：两者对不上（外部手改、seed 写错）时，写回去的必须还是原来那个文件，不能另写一份。
    fn save(&self, uuid: &str, book: Book) -> Result<(), String> {
        let p = self.path(uuid)?;
        let bytes = serde_json::to_vec_pretty(&book).map_err(|e| e.to_string())?;
        write_atomic(&p, &bytes).map_err(|e| format!("写条目库失败: {e}"))?;
        let mut cache = self.cache();
        match std::fs::metadata(&p) {
            Ok(m) => cache.insert(uuid.to_string(), (FileKey::of(&m), Arc::new(book))),
            Err(_) => cache.remove(uuid),
        };
        Ok(())
    }

    /// 读—改—写（进程内串行化）。书不存在时以 `seed()` 起。`f` 没改动任何东西就不写盘（见 [`Self::update_existing`]）。
    pub fn update<T>(&self, uuid: &str, seed: impl FnOnce() -> Book, f: impl FnOnce(&mut Book) -> T) -> Result<T, String> {
        self.path(uuid)?; // 先验 key：非法 uuid 不跑 f、不落盘
        let _g = rmsvc_core::sync::lock(&self.lock);
        let prev = self.read(uuid)?;
        let mut book = prev.as_ref().map(|b| (**b).clone()).unwrap_or_else(seed);
        let out = f(&mut book);
        if prev.as_deref() != Some(&book) {
            self.save(uuid, book)?;
        }
        Ok(out)
    }

    /// 读—改—写**已存在**的书（进程内串行化）：书不存在（或 uuid 非法）→ `Ok(None)`，不跑 `f`、不落盘、不建空书。
    /// 取代此前"先 `load` 判存在、再 `update(.., || Default::default(), ..)`"的两步（多解析一遍整本 JSON，
    /// 且两步之间不在同一把锁里）。
    pub fn update_existing<T>(&self, uuid: &str, f: impl FnOnce(&mut Book) -> T) -> Result<Option<T>, String> {
        if self.path(uuid).is_err() {
            return Ok(None);
        }
        let _g = rmsvc_core::sync::lock(&self.lock);
        let Some(prev) = self.read(uuid)? else { return Ok(None) };
        let mut book = (*prev).clone();
        let out = f(&mut book);
        // 没改动就不写盘：回收站里的书每次事件/启动追平都会路过 `revoke_stale`、清空回收站点了 0 条、`rescan`
        // 清空本来就空的页记录——此前都会原样重写整本 JSON（磨闪存、换 inode 让解析缓存失效）。
        if *prev != book {
            self.save(uuid, book)?;
        }
        Ok(Some(out))
    }

    /// 所有书（按标题排序）——含条目已全部撤销的书（内部/调试用；网页列表用 `list_active`）。
    /// 走 `read`（命中缓存就不重新解析）；解析失败的书跳过（`read` 已另存 `.corrupt` 并打日志）。
    pub fn list(&self) -> Vec<Arc<Book>> {
        let mut out: Vec<Arc<Book>> = std::fs::read_dir(&self.dir)
            .map(|rd| rd.flatten().filter_map(|e| self.read(e.file_name().to_str()?.strip_suffix(".json")?).ok().flatten()).collect())
            .unwrap_or_default();
        out.sort_by(|a, b| a.title.cmp(&b.title));
        out
    }

    /// 只列"现在还有活条目"的书（按标题排序）：摄取门槛看的是"页 .rm 文件存不存在"，笔画擦光了文件不会被删，
    /// 一本书清空所有勾画/手写后摄取仍会跑（正确把条目标成 Revoked），但如果不过滤，它会因为"曾经有过
    /// .rm 文件"永远赖在网页的书选择列表里——即使当下一条活条目都没有（2026-09-07 真机验证时发现）。
    pub fn list_active(&self) -> Vec<Arc<Book>> {
        self.list().into_iter().filter(|b| b.entries.iter().any(|e| e.status != Status::Revoked)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notecore::model::Entry;

    fn entry(id: &str, status: Status) -> Entry {
        Entry { id: id.into(), page: "p".into(), page_index: 0, chapter: None, chapter_title: String::new(), subhead: None, quote: None, ink: None, drafts: vec![], text: None, style: Default::default(), ask_ai: false, question: None, answer: None, status, destination: Default::default(), source: Default::default(), created: 0, updated: 0 }
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

    #[test]
    fn update_existing_never_creates_a_book_and_handles_bad_uuid() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        let mut ran = false;
        assert_eq!(db.update_existing("ghost", |_| ran = true).unwrap(), None);
        assert_eq!(db.update_existing("../evil", |_| ran = true).unwrap(), None, "非法 uuid 同样当作不存在");
        assert!(!ran, "不存在的书不跑闭包");
        assert!(db.list().is_empty() && !t.path().join("books/ghost.json").exists(), "不能悄悄建出空书");
        db.update("u1", || Book { uuid: "u1".into(), title: "甲".into(), ..Default::default() }, |_| ()).unwrap();
        assert_eq!(db.update_existing("u1", |b| { b.chapters.push("一".into()); b.chapters.len() }).unwrap(), Some(1));
        assert_eq!(db.load("u1").unwrap().chapters, vec!["一"], "改动已落盘");
    }

    /// 回归：条目库 JSON 解析失败时不能被当成新书、用空书覆盖掉；原文件不动，另存 .corrupt 副本。
    #[test]
    fn corrupt_book_is_never_overwritten() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        let f = t.path().join("books/u1.json");
        let bad = br#"{"uuid":"u1","title":"x","entries":[{"status":"FutureStatus"}]}"#;
        std::fs::write(&f, bad).unwrap();

        assert!(db.read("u1").unwrap_err().contains("解析失败"));
        let mut ran = false;
        assert!(db.update("u1", || Book { uuid: "u1".into(), ..Default::default() }, |_| ran = true).is_err());
        assert!(db.update_existing("u1", |_| ran = true).is_err());
        assert!(!ran, "坏文件不跑闭包");
        assert_eq!(std::fs::read(&f).unwrap(), bad, "原文件原样不动");
        assert_eq!(std::fs::read(t.path().join("books/u1.json.corrupt")).unwrap(), bad, "另存了副本");
        assert!(db.read("nope").unwrap().is_none(), "不存在仍是 Ok(None)");
    }

    /// 解析缓存：自己写入后立即读到新值；文件被外部改写（换内容/改坏/删掉）后不会继续给旧缓存。
    #[test]
    fn cache_follows_own_writes_and_external_changes() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        db.update("u1", || Book { uuid: "u1".into(), title: "甲".into(), ..Default::default() }, |_| ()).unwrap();
        let a = db.read("u1").unwrap().unwrap();
        let b = db.read("u1").unwrap().unwrap();
        assert!(Arc::ptr_eq(&a, &b), "文件没变 → 同一份缓存，不重新解析");
        db.update_existing("u1", |b| b.title = "乙".into()).unwrap();
        assert_eq!(db.read("u1").unwrap().unwrap().title, "乙", "自己写完立刻可见");
        assert_eq!(db.list()[0].title, "乙");

        let f = t.path().join("books/u1.json");
        // 外部改写（换 inode：跟 write_atomic 一样 rename 进来）
        std::fs::write(t.path().join("x.tmp"), r#"{"uuid":"u1","title":"外部改的"}"#).unwrap();
        std::fs::rename(t.path().join("x.tmp"), &f).unwrap();
        assert_eq!(db.read("u1").unwrap().unwrap().title, "外部改的");
        // 原地改坏（同 inode，长度变了）
        std::fs::write(&f, b"{ broken").unwrap();
        assert!(db.read("u1").is_err(), "坏文件不能拿旧缓存顶替");
        assert!(db.list().is_empty(), "list 跳过坏文件");
        std::fs::remove_file(&f).unwrap();
        assert!(db.read("u1").unwrap().is_none(), "删掉了就是没有");
    }

    /// 没改动的读—改—写不落盘（文件身份不变）；书里 `uuid` 字段跟文件名对不上时仍写回原文件。
    #[test]
    fn noop_update_does_not_rewrite_and_save_uses_file_key() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        db.update("u1", || Book { uuid: "u1".into(), title: "甲".into(), ..Default::default() }, |_| ()).unwrap();
        assert!(t.path().join("books/u1.json").exists(), "新书即使没改也要落盘");
        let ino = |p: &str| std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(t.path().join(p)).unwrap());
        let before = ino("books/u1.json");
        db.update_existing("u1", |b| b.page_mtimes.clear()).unwrap();
        db.update("u1", Book::default, |b| b.title = "甲".into()).unwrap();
        assert_eq!(ino("books/u1.json"), before, "没变化 → 不重写");
        db.update_existing("u1", |b| b.title = "乙".into()).unwrap();
        assert_ne!(ino("books/u1.json"), before, "有变化 → 写");

        std::fs::write(t.path().join("books/u2.json"), r#"{"uuid":"别的","title":"x"}"#).unwrap();
        db.update_existing("u2", |b| b.title = "y".into()).unwrap();
        assert_eq!(db.load("u2").unwrap().title, "y");
        assert!(!t.path().join("books/别的.json").exists());
    }

    /// 回归：uuid 带 `/`、`..` 不能读写条目库目录之外的文件。
    #[test]
    fn rejects_path_traversal_uuid() {
        let t = tempfile::tempdir().unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        std::fs::write(t.path().join("outside.json"), r#"{"uuid":"o","title":"外面"}"#).unwrap();
        assert!(db.load("../outside").is_none(), "不能读到目录之外的 json");
        let mut ran = false;
        let r = db.update("../evil", || Book { uuid: "../evil".into(), title: "x".into(), ..Default::default() }, |_| ran = true);
        assert!(r.is_err() && !ran, "非法 uuid 直接拒绝，不跑闭包");
        assert!(!t.path().join("evil.json").exists());
    }
}
