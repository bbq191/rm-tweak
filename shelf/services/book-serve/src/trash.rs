//! 原生书库「移进回收站」队列：外部进程不能直改 `.metadata`（运行中 xochitl 会覆写回来），真正的软删只有
//! xochitl 自己的 `selectionMoveToTrash()`——由注入 Sidebar 的 `shelf/xovi/shelf-trash-agent.qmd` 在书库视图有动静时
//! `GET /trash/pending` 拉队列执行。本模块只管队列：入队时按 visibleName 核对 uuid（防错删），拉取时把已进回收站 /
//! 已不存在的条目清掉（QML 端无需 ack）。队列文件 `$XDG_STATE_HOME/shelf/books/trash-pending.json`。
//! 首个用途：`shelf doctor --render` 量完把探针书送进回收站，不在原生书库里累积（2026-09-06）。
//!
//! 持久化+入队去重+剔除这层通用外壳委托 `pending_queue::PendingQueue<T>`（2026-09-09 消重复，跟
//! `mkdir.rs` 是同一份基础设施，见该模块文档）；这里只留领域校验（uuid 形状/名字核对/是否已在回收站）。
use crate::pending_queue::PendingQueue;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pending {
    pub uuid: String,
    pub name: String,
    pub at: u64,
}

pub struct TrashQueue {
    q: PendingQueue<Pending>,
    lib_dir: PathBuf,
}

impl TrashQueue {
    pub fn new(state_books_dir: &Path, lib_dir: &Path) -> TrashQueue {
        TrashQueue { q: PendingQueue::new(state_books_dir.join("trash-pending.json")), lib_dir: lib_dir.to_path_buf() }
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
        if uuid.len() != 36 || !uuid.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
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
        self.q.add(|p| p.uuid == uuid, || Pending { uuid: uuid.to_string(), name: vis, at: rmsvc_core::clock::now_secs() })
    }

    /// 待办 uuid（QML 代理拉取）：顺手清掉已进回收站 / 已不存在的。返回 (待办 uuid 列表, 本次清掉几条)。
    pub fn pending(&self) -> Result<(Vec<String>, usize), String> {
        let (kept, pruned) = self.q.prune(|p| matches!(self.meta(&p.uuid), Some((_, parent)) if parent != "trash"))?;
        Ok((kept.into_iter().map(|p| p.uuid).collect(), pruned))
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
        let q = TrashQueue::new(&t.path().join("state"), &lib(&t));
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
}
