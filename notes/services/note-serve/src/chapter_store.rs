//! 通用"每本书每章记一条 `T` 类型记录"的簿记：一书一文件 `<dir>/<uuid>.json`。
//!
//! `NotebookState`（记生成设备笔记本用的 `ChapterRecord`）和 `ExportState`（记导出 md 用的
//! `ExportRecord`）之前是两份几乎逐行相同的代码，只有记录类型的字段不一样——这里抽成一个泛型，
//! 两边各自的记录类型保持不变（字段、字段名都不动），磁盘上的 JSON 形状因此逐字节不变（`BookRecord`
//! 不带 `rename_all`，字段名就是 Rust 里写的 snake_case，跟真机上已经存在的 `notebooks/*.json`/
//! `exports/*.json` 完全对得上，不用迁移），只是"按书分文件、按章存取、锁写"这部分逻辑不再抄两遍。
//!
//! 这是 note-serve 自己的簿记，不是条目库（条目库的唯一写者仍是 ink-serve）——丢了任意一份文件最坏
//! 后果只是"重新判一次要不要重生成/重写"，不丢数据。
use serde::{de::DeserializeOwned, Serialize};
use rmsvc_core::fs::write_atomic;
use std::collections::BTreeMap;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[serde(bound(serialize = "T: Serialize", deserialize = "T: DeserializeOwned"))]
struct BookRecord<T> {
    chapters: BTreeMap<String, T>,
}

impl<T> Default for BookRecord<T> {
    fn default() -> Self {
        BookRecord { chapters: BTreeMap::new() }
    }
}

pub struct ChapterStore<T> {
    dir: PathBuf,
    lock: Mutex<()>,
    _record: PhantomData<T>,
}

impl<T: Clone + Serialize + DeserializeOwned> ChapterStore<T> {
    pub fn new(dir: PathBuf) -> ChapterStore<T> {
        ChapterStore { dir, lock: Mutex::new(()), _record: PhantomData }
    }
    pub fn ensure(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.dir)
    }
    pub fn dir(&self) -> &Path {
        &self.dir
    }
    fn path(&self, book_uuid: &str) -> PathBuf {
        self.dir.join(format!("{book_uuid}.json"))
    }
    fn load(&self, book_uuid: &str) -> BookRecord<T> {
        std::fs::read(self.path(book_uuid)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }
    fn save(&self, book_uuid: &str, b: &BookRecord<T>) -> Result<(), String> {
        write_atomic(&self.path(book_uuid), &serde_json::to_vec_pretty(b).map_err(|e| e.to_string())?).map_err(|e| format!("写状态失败: {e}"))
    }

    pub fn get(&self, book_uuid: &str, chapter_idx: usize) -> Option<T> {
        self.load(book_uuid).chapters.get(&chapter_idx.to_string()).cloned()
    }

    pub fn set(&self, book_uuid: &str, chapter_idx: usize, rec: T) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut b = self.load(book_uuid);
        b.chapters.insert(chapter_idx.to_string(), rec);
        self.save(book_uuid, &b)
    }

    /// 清掉某章的记录——章内容变成"没有可导出/可生成的条目"时用，不然旧记录会一直显示"已同步"，
    /// 跟当前"这章根本没有对应文件"的事实对不上。原来只有 `ExportState` 有这个方法，`NotebookState`
    /// 没有；泛型后两边都能用（`notebooks.rs` 暂时不调用它，行为不变，只是能力对齐了）。
    pub fn clear(&self, book_uuid: &str, chapter_idx: usize) -> Result<(), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut b = self.load(book_uuid);
        if b.chapters.remove(&chapter_idx.to_string()).is_some() {
            self.save(book_uuid, &b)?;
        }
        Ok(())
    }

    /// 这本书目前记着的全部章节记录（章序号 → 记录），网页状态展示用。
    pub fn list(&self, book_uuid: &str) -> BTreeMap<usize, T> {
        self.load(book_uuid).chapters.into_iter().filter_map(|(k, v)| k.parse().ok().map(|i| (i, v))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
    struct Rec {
        a: String,
        b: u64,
    }

    #[test]
    fn set_get_list_clear_roundtrip_persists_across_instances() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("recs");
        let st: ChapterStore<Rec> = ChapterStore::new(dir.clone());
        st.ensure().unwrap();
        assert!(st.get("b1", 0).is_none());
        let rec = Rec { a: "x".into(), b: 100 };
        st.set("b1", 0, rec.clone()).unwrap();
        assert_eq!(st.get("b1", 0), Some(rec.clone()));
        assert!(st.get("b1", 1).is_none(), "另一章互不影响");

        // 换一个新实例（模拟重启）应该读到同一份
        let st2: ChapterStore<Rec> = ChapterStore::new(dir);
        assert_eq!(st2.get("b1", 0), Some(rec));
        assert!(st2.dir().join("b1.json").exists());

        let rec2 = Rec { a: "y".into(), b: 200 };
        st2.set("b1", 0, rec2.clone()).unwrap();
        assert_eq!(st2.get("b1", 0), Some(rec2.clone()), "覆盖旧记录");

        let rec3 = Rec { a: "z".into(), b: 300 };
        st2.set("b1", 1, rec3.clone()).unwrap();
        let all = st2.list("b1");
        assert_eq!(all.len(), 2);
        assert_eq!(all[&0], rec2);
        assert_eq!(all[&1], rec3);
        assert!(st2.list("no-such-book").is_empty());

        st2.clear("b1", 0).unwrap();
        assert!(st2.get("b1", 0).is_none(), "清掉之后就是没有过记录");
        assert_eq!(st2.list("b1").len(), 1, "另一章不受影响");
        st2.clear("b1", 0).unwrap(); // 再清一次（本来就没有）不报错
    }

    /// 真机上已经存在的 `notebooks/<uuid>.json`（`ChapterRecord`：`doc_uuid`/`visible_name`/
    /// `fingerprint`/`generated_at`，snake_case，无 rename_all）能被泛型化后的 `ChapterStore` 原样
    /// 读回——这是这次重构最关键的一条回归：真机上有用户已经生成过的真实数据，字段名/大小写差一点
    /// 都会读不出来（`serde_json::from_slice` 失败会被 `load()` 的 `.ok()` 悄悄吞掉、退化成"没有记录"，
    /// 不会报错，但会让「整理」页误判所有章节都"从没生成过"）。
    #[test]
    fn reads_real_device_notebook_record_shape_unchanged() {
        #[derive(Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
        struct ChapterRecordShape {
            doc_uuid: String,
            visible_name: String,
            fingerprint: String,
            generated_at: u64,
        }
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("notebooks");
        std::fs::create_dir_all(&dir).unwrap();
        // 真机 2026-09-08 实测采样（uuid/内容脱敏，字段名与形状原样）。
        std::fs::write(
            dir.join("book1.json"),
            r#"{"chapters":{"1":{"doc_uuid":"6b1279be-d70d-46bd-815a-bcff8c8067c2","visible_name":"第2章 起点","fingerprint":"14312532556a536f","generated_at":1788860715740}}}"#,
        )
        .unwrap();
        let st: ChapterStore<ChapterRecordShape> = ChapterStore::new(dir);
        let rec = st.get("book1", 1).expect("真机采样的记录应该能读出来");
        assert_eq!(rec.doc_uuid, "6b1279be-d70d-46bd-815a-bcff8c8067c2");
        assert_eq!(rec.generated_at, 1788860715740);
    }
}
