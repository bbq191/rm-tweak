//! 设备端代理执行不成、已放弃的记录（2026-09-25 用户要求）：回收站 / 建文件夹两个队列交满
//! [`crate::pending_queue::HANDOUT_MAX_ATTEMPTS`] 次仍没真实发生就放弃（见 `trash.rs` / `mkdir.rs`），此前只在
//! book-serve 日志里留一行，网页上看不到——用户以为书已进回收站、文件夹已建好。现在放弃时记一条到这里、发 `books`
//! 事件（kind `agent-failed`），网页页头横幅列出来，用户点「知道了」清空。
//!
//! 文件 `$XDG_STATE_HOME/shelf/books/agent-failures.json`，只留最近 [`KEEP`] 条（放弃本来就罕见，留多了没意义）。
use crate::pending_queue::PendingQueue;
use rmsvc_core::events::EventBus;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// 最多留几条（最旧的先丢）。
const KEEP: usize = 20;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Failure {
    /// `trash`（移进 xochitl 回收站）/ `mkdir`（在 xochitl 书库建文件夹）。
    pub kind: String,
    /// 书名 / 文件夹名（给人看的）。
    pub name: String,
    /// 书的 uuid（`trash` 才有）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub uuid: String,
    pub at: u64,
}

pub struct AgentFailures {
    q: PendingQueue<Failure>,
    bus: Option<Arc<EventBus>>,
}

impl AgentFailures {
    pub fn new(state_books_dir: &Path, bus: Option<Arc<EventBus>>) -> AgentFailures {
        AgentFailures { q: PendingQueue::new(state_books_dir.join("agent-failures.json")), bus }
    }

    /// 记一条放弃，并通知网页。写盘失败只打日志——放弃本身已经发生，不因记不下来而改变队列行为。
    pub fn record(&self, kind: &str, name: &str, uuid: &str) {
        let f = Failure { kind: kind.into(), name: name.into(), uuid: uuid.into(), at: rmsvc_core::clock::now_secs() };
        if let Err(e) = self.q.push_capped(f, KEEP) {
            eprintln!("[book-serve] 记录代理放弃失败: {e}");
        }
        if let Some(bus) = &self.bus {
            bus.publish("books", "agent-failed");
        }
    }

    pub fn list(&self) -> Vec<Failure> {
        self.q.list()
    }

    /// 用户点了「知道了」：清空。返回清掉几条。
    pub fn clear(&self) -> Result<usize, String> {
        let (_, n) = self.q.prune(|_| false)?;
        if n > 0 {
            if let Some(bus) = &self.bus {
                bus.publish("books", "agent-failed");
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_latest_and_clears() {
        let t = tempfile::tempdir().unwrap();
        let log = AgentFailures::new(t.path(), None);
        assert!(log.list().is_empty());
        for i in 0..KEEP + 3 {
            log.record("mkdir", &format!("夹{i}"), "");
        }
        let items = log.list();
        assert_eq!(items.len(), KEEP, "只留最近 {KEEP} 条");
        assert_eq!((items[0].name.as_str(), items[KEEP - 1].name.as_str()), ("夹3", "夹22"), "最旧的先丢，顺序不乱");
        log.record("trash", "书", "11111111-1111-1111-1111-111111111111");
        assert_eq!(log.list().last().unwrap().uuid, "11111111-1111-1111-1111-111111111111");
        assert_eq!(log.clear().unwrap(), KEEP);
        assert!(log.list().is_empty());
        assert_eq!(log.clear().unwrap(), 0);
    }
}
