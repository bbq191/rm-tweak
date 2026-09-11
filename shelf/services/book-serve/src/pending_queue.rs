//! 共享的"落盘待办队列"骨架：`TrashQueue`（`trash.rs`）与 `MkdirQueue`（`mkdir.rs`）此前是两份
//! 几乎逐行相同的代码（同样的 `{file, lib_dir, lock}` 字段、同样的原子读写 JSON、同样的
//! "入队去重"/"拉取时剔除已经真实发生的项"结构，只有记录类型和校验/剔除谓词不一样）——2026-09-09
//! 审计发现，仿照 `note-serve::chapter_store::ChapterStore<T>` 的做法泛型化，只抽持久化+入队+剔除
//! 这层通用外壳；入队前的领域校验（uuid 形状、名字对不对得上、文件夹是否已存在……）留给各自的
//! `add()` 包装方法，那是两边真正不同、不该合并的部分。
use serde::de::DeserializeOwned;
use serde::Serialize;
use rmsvc_core::fs::write_atomic;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::Mutex;

pub struct PendingQueue<T> {
    file: PathBuf,
    lock: Mutex<()>,
    _marker: PhantomData<T>,
}

impl<T: Clone + Serialize + DeserializeOwned> PendingQueue<T> {
    pub fn new(file: PathBuf) -> PendingQueue<T> {
        PendingQueue { file, lock: Mutex::new(()), _marker: PhantomData }
    }

    fn load(&self) -> Vec<T> {
        std::fs::read(&self.file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    fn save(&self, items: &[T]) -> Result<(), String> {
        if let Some(p) = self.file.parent() {
            let _ = std::fs::create_dir_all(p);
        }
        write_atomic(&self.file, &serde_json::to_vec(items).map_err(|e| e.to_string())?).map_err(|e| format!("写队列失败: {e}"))
    }

    pub fn list(&self) -> Vec<T> {
        self.load()
    }

    /// 入队：`exists` 判"这条是不是已经在队列里了"（去重键），`make` 造新记录——**拿到锁之后才调用**，
    /// 保证"查重 + 写入"是一个原子操作，不会有两个并发请求都通过查重各插一条。返回入队后的队列长度
    /// （调用方常用它当"这是第几条"的粗略反馈，不代表新增了一条——已存在时也返回当前长度）。
    pub fn add(&self, exists: impl Fn(&T) -> bool, make: impl FnOnce() -> T) -> Result<usize, String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let mut items = self.load();
        if !items.iter().any(exists) {
            items.push(make());
            self.save(&items)?;
        }
        Ok(items.len())
    }

    /// 剔除不再满足 `keep` 的记录（已经真实发生/消失，不用再等 QML 代理处理的那些），返回
    /// `(保留的记录, 剔除了几条)`。只有真剔除了才落盘，没变化不重写文件。
    pub fn prune(&self, keep: impl Fn(&T) -> bool) -> Result<(Vec<T>, usize), String> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        let items = self.load();
        let kept: Vec<T> = items.iter().filter(|p| keep(p)).cloned().collect();
        let pruned = items.len() - kept.len();
        if pruned > 0 {
            self.save(&kept)?;
        }
        Ok((kept, pruned))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
    struct Item {
        key: String,
    }

    #[test]
    fn add_dedupes_by_key_and_persists_across_instances() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("state").join("q.json");
        let q = PendingQueue::<Item>::new(file.clone());
        assert_eq!(q.add(|i| i.key == "a", || Item { key: "a".into() }).unwrap(), 1);
        assert_eq!(q.add(|i| i.key == "a", || Item { key: "a".into() }).unwrap(), 1, "重复入队不翻倍");
        assert_eq!(q.add(|i| i.key == "b", || Item { key: "b".into() }).unwrap(), 2);
        assert_eq!(q.list(), vec![Item { key: "a".into() }, Item { key: "b".into() }]);

        // 新实例指向同一份文件，读到的是磁盘上已经落的内容——证明真的原子写落了盘，不是只在内存里。
        let q2 = PendingQueue::<Item>::new(file);
        assert_eq!(q2.list().len(), 2);
    }

    #[test]
    fn prune_removes_only_items_failing_keep_and_skips_save_when_nothing_pruned() {
        let t = tempfile::tempdir().unwrap();
        let file = t.path().join("q.json");
        let q = PendingQueue::<Item>::new(file.clone());
        q.add(|i| i.key == "keep", || Item { key: "keep".into() }).unwrap();
        q.add(|i| i.key == "drop", || Item { key: "drop".into() }).unwrap();

        let (kept, pruned) = q.prune(|i| i.key == "keep").unwrap();
        assert_eq!(kept, vec![Item { key: "keep".into() }]);
        assert_eq!(pruned, 1);
        assert_eq!(q.list(), vec![Item { key: "keep".into() }]);

        // 再剪一次，这次没有可剪的——不该重写文件（没法直接断言"没写"，但结果该保持不变，
        // 且 pruned 该是 0，这是行为层面能确认的部分）。
        let (kept2, pruned2) = q.prune(|_| true).unwrap();
        assert_eq!((kept2, pruned2), (vec![Item { key: "keep".into() }], 0));
    }
}
