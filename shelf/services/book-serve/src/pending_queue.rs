//! 共享的"落盘待办队列"骨架：`TrashQueue`（`trash.rs`）与 `MkdirQueue`（`mkdir.rs`）此前是两份
//! 几乎逐行相同的代码（同样的 `{file, lib_dir, lock}` 字段、同样的原子读写 JSON、同样的
//! "入队去重"/"拉取时剔除已经真实发生的项"结构，只有记录类型和校验/剔除谓词不一样）——2026-09-09
//! 审计发现，仿照 `note-serve::chapter_store::ChapterStore<T>` 的做法泛型化，只抽持久化+入队+剔除
//! 这层通用外壳；入队前的领域校验（uuid 形状、名字对不对得上、文件夹是否已存在……）留给各自的
//! `add()` 包装方法，那是两边真正不同、不该合并的部分。
use serde::de::DeserializeOwned;
use serde::Serialize;
use rmsvc_core::fs::write_atomic;
use std::collections::HashMap;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

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
        let _g = rmsvc_core::sync::lock(&self.lock);
        let mut items = self.load();
        if !items.iter().any(exists) {
            items.push(make());
            self.save(&items)?;
        }
        Ok(items.len())
    }

    /// 追加一条，超过 `cap` 条时丢掉最旧的（日志型用法，如 `agent_failures`）。
    pub fn push_capped(&self, item: T, cap: usize) -> Result<(), String> {
        let _g = rmsvc_core::sync::lock(&self.lock);
        let mut items = self.load();
        items.push(item);
        let over = items.len().saturating_sub(cap);
        items.drain(..over);
        self.save(&items)
    }

    /// 剔除不再满足 `keep` 的记录（已经真实发生/消失，不用再等 QML 代理处理的那些），返回
    /// `(保留的记录, 剔除了几条)`。只有真剔除了才落盘，没变化不重写文件。
    pub fn prune(&self, keep: impl Fn(&T) -> bool) -> Result<(Vec<T>, usize), String> {
        let _g = rmsvc_core::sync::lock(&self.lock);
        let items = self.load();
        let kept: Vec<T> = items.iter().filter(|p| keep(p)).cloned().collect();
        let pruned = items.len() - kept.len();
        if pruned > 0 {
            self.save(&kept)?;
        }
        Ok((kept, pruned))
    }
}

/// 两个队列共用的"交给 QML 代理"那一半（2026-09-25 从 `mkdir.rs` 抽出，`trash.rs` 同时改成长轮询）：
/// - **长轮询**：入队计数（代数）+ 条件变量，[`Handout::wait`] 有待办立即返回、没有就睡到入队唤醒或到期——
///   代理空闲时整条链路零唤醒；
/// - **不重复交出**：两件事在 xochitl 里都是异步的（`createCollection` / `moveEntriesToTrash` 调完 `.metadata`
///   稍晚才落盘），长轮询让代理几乎立刻再来拉，这时仍把同一项交出去就会重复执行。同一个键交出后 `quiet` 内
///   不再交；过了还没真实发生才再交一次，等于自带重试。
/// - **有限次重试**：同一个键最多交出 [`HANDOUT_MAX_ATTEMPTS`] 次；交满了、静默期过了还没真实发生（xochitl 那边一直
///   执行不成，比如 `entryForId` 拿不到条目但 `.metadata` 还在），就放弃这一项——调用方把它移出队列并记一行日志。
///   此前没有终止条件，这样的项每个静默期被交出一次、代理每次执行失败写一行日志，永远不停（2026-09-25 第四轮审计）。
pub struct Handout {
    gen: Mutex<u64>,
    wake: Condvar,
    /// 键 → (最近一次交出的时刻, 已交出次数)。只保留仍在待办里的键。
    handed: Mutex<HashMap<String, (Instant, u32)>>,
    quiet: Duration,
    max_attempts: u32,
}

/// 同一项最多交给代理几次（见 [`Handout`]）。静默期 15s/30s 下约 1–2.5 分钟后放弃。
pub const HANDOUT_MAX_ATTEMPTS: u32 = 5;

/// [`Handout::take`] 的结果：这次交出的键 + 交满次数仍没完成、该放弃的键。
#[derive(Debug, Default, PartialEq)]
pub struct Taken {
    pub hand: Vec<String>,
    pub give_up: Vec<String>,
}

impl Handout {
    pub fn new(quiet: Duration) -> Handout {
        Handout { gen: Mutex::new(0), wake: Condvar::new(), handed: Mutex::new(HashMap::new()), quiet, max_attempts: HANDOUT_MAX_ATTEMPTS }
    }

    /// 单测用：改最多交出次数。
    #[cfg(test)]
    pub fn with_max_attempts(mut self, n: u32) -> Handout {
        self.max_attempts = n;
        self
    }

    /// 入队后调用：唤醒正在长轮询的代理。
    pub fn notify(&self) {
        *rmsvc_core::sync::lock(&self.gen) += 1;
        self.wake.notify_all();
    }

    /// 从"仍待办的键"里挑出这次该交出的（去掉静默期内交过的），并把它们记为"已交出"；静默期已过、却已经交满次数的
    /// 放进 `give_up`（不再记录，调用方负责移出队列）。
    pub fn take(&self, keys: Vec<String>) -> Taken {
        let now = Instant::now();
        let mut handed = rmsvc_core::sync::lock(&self.handed);
        handed.retain(|k, _| keys.contains(k));
        let mut out = Taken::default();
        for k in keys {
            let attempts = match handed.get(&k) {
                Some(&(at, _)) if now.duration_since(at) < self.quiet => continue,
                Some(&(_, n)) => n,
                None => 0,
            };
            if attempts >= self.max_attempts {
                handed.remove(&k);
                out.give_up.push(k);
            } else {
                handed.insert(k.clone(), (now, attempts + 1));
                out.hand.push(k);
            }
        }
        out
    }

    /// 最早一个"已交出、静默期到了还在队列里就再交一次"的时刻（没有交出过的待办 → `None`）。
    fn next_retry(&self) -> Option<Instant> {
        rmsvc_core::sync::lock(&self.handed).values().map(|(at, _)| *at + self.quiet).min()
    }

    /// 长轮询：反复调 `fetch`（返回 (本次该交出的键, 剔除条数)），有结果或 `wait` 到期就返回；`wait` 为零＝不等。
    /// 睡眠还会在最早一个静默期到期时醒一次：此前只等入队或 `wait` 到期，代理用 290 秒长轮询时，交出后没执行成功的
    /// 那一项要等满 290 秒才重交，"静默期过了自带重试"名存实亡——建文件夹落库只等 20 秒，重试永远赶不上
    /// （2026-09-25 第四轮审计）。空闲（没有交出过的待办）时仍然只在入队或到期时醒。
    pub fn wait(&self, wait: Duration, mut fetch: impl FnMut() -> Result<(Vec<String>, usize), String>) -> Result<(Vec<String>, usize), String> {
        let deadline = Instant::now() + wait;
        let mut total_pruned = 0;
        loop {
            // 先取代数再查队列：查完到睡下之间若有入队，代数已变，wait_timeout_while 不会睡过头。
            let seen = *rmsvc_core::sync::lock(&self.gen);
            let (keys, pruned) = fetch()?;
            total_pruned += pruned;
            let now = Instant::now();
            if !keys.is_empty() || now >= deadline {
                return Ok((keys, total_pruned));
            }
            let wake_at = self.next_retry().map_or(deadline, |r| r.min(deadline));
            let g = rmsvc_core::sync::lock(&self.gen);
            let _ = self.wake.wait_timeout_while(g, wake_at.saturating_duration_since(now), |cur| *cur == seen).unwrap_or_else(|e| e.into_inner());
        }
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

    /// 有限次重试：交满次数后（静默期已过）放弃，放弃后不再记录；完成（不在待办里）的项清掉计数。
    #[test]
    fn handout_gives_up_after_max_attempts() {
        let h = Handout::new(Duration::ZERO).with_max_attempts(3);
        let keys = || vec!["a".to_string(), "b".to_string()];
        for _ in 0..3 {
            assert_eq!(h.take(keys()), Taken { hand: keys(), give_up: vec![] });
        }
        assert_eq!(h.take(vec!["a".to_string()]), Taken { hand: vec![], give_up: vec!["a".to_string()] }, "b 已完成（不在待办里），a 交满放弃");
        assert!(h.next_retry().is_none(), "放弃/完成的项不再占计时");
        assert_eq!(h.take(vec!["a".to_string()]).hand, vec!["a".to_string()], "放弃后若又被重新入队，从零计");
        // 静默期内不交也不算次数
        let h = Handout::new(Duration::from_secs(60)).with_max_attempts(1);
        assert_eq!(h.take(keys()).hand.len(), 2);
        assert_eq!(h.take(keys()), Taken::default());
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
