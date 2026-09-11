//! 原生书库「移进回收站」队列：外部进程不能直改 `.metadata`（运行中 xochitl 会覆写回来），真正的软删只能走
//! xochitl 自己的代码路——由注入 MainView 的 `shelf/xovi/shelf-trash-agent.qmd` 长轮询 `GET /trash/pending?wait=`
//! 拉队列，调 `LibraryController.moveEntriesToTrash(ids)` 执行。
//! **2026-09-25 改**：原先代理注入 Sidebar、只在书库视图有动静时拉、用"当前文件夹的选择集 + selectionMoveToTrash"
//! 执行——网页「设备健康 → 清理」入队后设备上不翻书库就永远不执行，书不在当前文件夹也加不进选择集（真机：
//! 《告白》《白夜行》在「好读精校」里，入队后一直没动）。现改为全局常驻 + 长轮询 + 按 id 调，跟建文件夹代理同构。本模块只管队列：入队时按 visibleName 核对 uuid（防错删），拉取时把已进回收站 /
//! 已不存在的条目清掉（QML 端无需 ack）。队列文件 `$XDG_STATE_HOME/shelf/books/trash-pending.json`。
//! 首个用途：渲染自检探针书（现已无此调用方，能力保留）送进回收站，不在原生书库里累积（2026-09-06）。
//!
//! 持久化+入队去重+剔除这层通用外壳委托 `pending_queue::PendingQueue<T>`（2026-09-09 消重复，跟
//! `mkdir.rs` 是同一份基础设施，见该模块文档）；这里只留领域校验（uuid 形状/名字核对/是否已在回收站）。
use crate::agent_failures::AgentFailures;
use crate::pending_queue::{Handout, PendingQueue, HANDOUT_MAX_ATTEMPTS};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// 同一 uuid 交给代理后这段时间内不再交（`moveEntriesToTrash` 是异步的，见 `pending_queue::Handout`）；过了仍没进
/// 回收站就再交一次。
const HANDOUT_QUIET: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pending {
    pub uuid: String,
    pub name: String,
    pub at: u64,
}

pub struct TrashQueue {
    q: PendingQueue<Pending>,
    lib_dir: PathBuf,
    handout: Handout,
    /// 放弃时记一条给网页看（见 `agent_failures.rs`）；单测不挂。
    failures: Option<Arc<AgentFailures>>,
}

impl TrashQueue {
    pub fn new(state_books_dir: &Path, lib_dir: &Path) -> TrashQueue {
        TrashQueue { q: PendingQueue::new(state_books_dir.join("trash-pending.json")), lib_dir: lib_dir.to_path_buf(), handout: Handout::new(HANDOUT_QUIET), failures: None }
    }

    pub fn with_failures(mut self, f: Arc<AgentFailures>) -> TrashQueue {
        self.failures = Some(f);
        self
    }

    /// 文档 `.metadata` 的 (visibleName, parent)；文件不存在 → None。
    fn meta(&self, uuid: &str) -> Option<(String, String)> {
        let t = std::fs::read_to_string(self.lib_dir.join(format!("{uuid}.metadata"))).ok()?;
        let v: serde_json::Value = serde_json::from_str(&t).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        Some((s("visibleName"), s("parent")))
    }

    /// 入队：uuid 必须真在书库且 visibleName 与 `name` 相符（忽略大小写、首尾空白），否则拒绝——错 uuid 就是错删别的书。
    pub fn add(&self, uuid: &str, name: &str) -> Result<usize, String> {
        if !rmsvc_core::xochitl::is_uuid_shape(uuid) {
            return Err("uuid 形状不对".into());
        }
        let (vis, parent) = self.meta(uuid).ok_or("书库里没有这份文档")?;
        if !vis.trim().eq_ignore_ascii_case(name.trim()) {
            return Err(format!("名字对不上（书库里叫《{vis}》），拒绝入队"));
        }
        if parent == "trash" {
            return Err("已经在回收站".into());
        }
        // `uuid: &str` 是 Copy，两个闭包各自拿一份拷贝就够——不要先转成 String 再共享，
        // 那样第一个闭包借用、第二个闭包要移动，会被借用检查器拦下来。
        let n = self.q.add(|p| p.uuid == uuid, || Pending { uuid: uuid.to_string(), name: vis, at: rmsvc_core::clock::now_secs() })?;
        self.handout.notify();
        Ok(n)
    }

    /// 待办 uuid（QML 代理拉取）：顺手清掉已进回收站 / 已不存在的，以及交满 [`HANDOUT_MAX_ATTEMPTS`] 次仍没进回收站、
    /// 放弃的（xochitl 的 `entryForId` 拿不到条目但 `.metadata` 还在时，代理每次都执行失败）。返回 (待办 uuid 列表, 本次清掉几条)。
    pub fn pending(&self) -> Result<(Vec<String>, usize), String> {
        let (kept, pruned) = self.q.prune(|p| matches!(self.meta(&p.uuid), Some((_, parent)) if parent != "trash"))?;
        let taken = self.handout.take(kept.iter().map(|p| p.uuid.clone()).collect());
        let mut dropped = pruned;
        if !taken.give_up.is_empty() {
            let (_, n) = self.q.prune(|p| !taken.give_up.contains(&p.uuid))?;
            dropped += n;
            for p in kept.iter().filter(|p| taken.give_up.contains(&p.uuid)) {
                println!("[book-serve] 《{}》（{}）已交给 xochitl {HANDOUT_MAX_ATTEMPTS} 次仍没进回收站，放弃（移出队列）", p.name, p.uuid);
                if let Some(f) = &self.failures {
                    f.record("trash", &p.name, &p.uuid);
                }
            }
        }
        Ok((taken.hand, dropped))
    }

    /// 长轮询版 [`Self::pending`]：有待办立即返回，没有就睡到入队或 `wait` 到期；`wait` 为零＝立即返回。
    pub fn pending_wait(&self, wait: Duration) -> Result<(Vec<String>, usize), String> {
        self.handout.wait(wait, || self.pending())
    }

    #[cfg(test)]
    fn with_handout_quiet(mut self, d: Duration) -> TrashQueue {
        self.handout = Handout::new(d);
        self
    }

    pub fn list(&self) -> Vec<Pending> {
        self.q.list()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(t: &tempfile::TempDir) -> PathBuf {
        let d = t.path().join("xochitl");
        std::fs::create_dir_all(&d).unwrap();
        let w = |n: &str, j: &str| std::fs::write(d.join(n), j).unwrap();
        w("11111111-1111-1111-1111-111111111111.metadata", r#"{"type":"DocumentType","visibleName":"书架自检探针 x","parent":""}"#);
        w("22222222-2222-2222-2222-222222222222.metadata", r#"{"type":"DocumentType","visibleName":"用户的书","parent":""}"#);
        w("33333333-3333-3333-3333-333333333333.metadata", r#"{"type":"DocumentType","visibleName":"已删","parent":"trash"}"#);
        d
    }

    #[test]
    fn add_guards_name_and_shape_then_pending_prunes_trashed() {
        let t = tempfile::tempdir().unwrap();
        let q = TrashQueue::new(&t.path().join("state"), &lib(&t)).with_handout_quiet(Duration::ZERO);
        assert!(q.add("bad", "x").unwrap_err().contains("形状"));
        assert!(q.add("44444444-4444-4444-4444-444444444444", "x").unwrap_err().contains("没有"));
        assert!(q.add("22222222-2222-2222-2222-222222222222", "书架自检探针 x").unwrap_err().contains("名字对不上"), "错 uuid 不许入队");
        assert!(q.add("33333333-3333-3333-3333-333333333333", "已删").unwrap_err().contains("回收站"));
        assert_eq!(q.add("11111111-1111-1111-1111-111111111111", " 书架自检探针 X ").unwrap(), 1);
        assert_eq!(q.add("11111111-1111-1111-1111-111111111111", "书架自检探针 x").unwrap(), 1, "重复入队不翻倍");
        assert_eq!(q.add("22222222-2222-2222-2222-222222222222", "用户的书").unwrap(), 2);
        let (ids, pruned) = q.pending().unwrap();
        assert_eq!((ids.len(), pruned), (2, 0));
        // xochitl 把探针移进回收站 → 下次拉取自动出队
        std::fs::write(t.path().join("xochitl/11111111-1111-1111-1111-111111111111.metadata"), r#"{"type":"DocumentType","visibleName":"书架自检探针 x","parent":"trash"}"#).unwrap();
        let (ids, pruned) = q.pending().unwrap();
        assert_eq!((ids, pruned), (vec!["22222222-2222-2222-2222-222222222222".to_string()], 1));
        assert_eq!(q.list().len(), 1);
    }

    /// xochitl 一直执行不成（.metadata 在、parent 不是 trash）：交满次数后放弃、移出队列，不再每 30 秒重交一次。
    #[test]
    fn gives_up_on_uuid_xochitl_never_trashes() {
        let t = tempfile::tempdir().unwrap();
        let fails = Arc::new(AgentFailures::new(&t.path().join("state"), None));
        let q = TrashQueue::new(&t.path().join("state"), &lib(&t)).with_handout_quiet(Duration::ZERO).with_failures(fails.clone());
        q.add("22222222-2222-2222-2222-222222222222", "用户的书").unwrap();
        for _ in 0..HANDOUT_MAX_ATTEMPTS {
            assert_eq!(q.pending().unwrap().0.len(), 1);
        }
        assert_eq!(q.pending().unwrap(), (vec![], 1), "交满放弃，算一条清掉");
        assert!(q.list().is_empty());
        let f = fails.list();
        assert_eq!((f.len(), f[0].kind.as_str(), f[0].name.as_str(), f[0].uuid.as_str()), (1, "trash", "用户的书", "22222222-2222-2222-2222-222222222222"), "放弃记下来给网页看");
        // 重新入队（用户再点一次）从零计数
        q.add("22222222-2222-2222-2222-222222222222", "用户的书").unwrap();
        assert_eq!(q.pending().unwrap().0.len(), 1);
    }

    #[test]
    fn handed_out_uuid_is_quiet_then_long_poll_wakes_on_add() {
        let t = tempfile::tempdir().unwrap();
        let q = std::sync::Arc::new(TrashQueue::new(&t.path().join("state"), &lib(&t)));
        q.add("11111111-1111-1111-1111-111111111111", "书架自检探针 x").unwrap();
        assert_eq!(q.pending().unwrap().0.len(), 1);
        assert!(q.pending().unwrap().0.is_empty(), "静默期内不重复交出（moveEntriesToTrash 异步，免得重复执行）");
        // 长轮询：空闲等满返回空；入队即刻唤醒
        let t0 = std::time::Instant::now();
        assert!(q.pending_wait(Duration::from_millis(120)).unwrap().0.is_empty());
        assert!(t0.elapsed() >= Duration::from_millis(110));
        let q2 = q.clone();
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            q2.add("22222222-2222-2222-2222-222222222222", "用户的书").unwrap();
        });
        let t0 = std::time::Instant::now();
        let (ids, _) = q.pending_wait(Duration::from_secs(10)).unwrap();
        assert_eq!(ids, vec!["22222222-2222-2222-2222-222222222222".to_string()]);
        assert!(t0.elapsed() < Duration::from_secs(3));
        h.join().unwrap();
    }
}
