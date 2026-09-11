//! 母版库条目的"正在跑的异步操作"登记簿：忙锁 + 取消协作，合在一把锁里。
//!
//! 进程内存态，**不落盘**：进程重启＝没有任何操作还在跑，"忙"天然清零（sidecar 里的 status 只管"上次结果展示"，
//! 不参与忙判断；重启后遗留的 pending 由 `Staging::recover_interrupted` 修正）。同一条目的「优化」跟「落库」互斥。
//!
//! 取消协作（2026-09-20 用户反馈"不能停止某个执行中的优化/投入"）：支持中途取消的步骤（EPUB 优化每处理完一个
//! 条目、按卷拆分投递每份之间）先 `mark_cancellable`，并在检查点问 `is_cancelled`；其它步骤（单文件上传、PDF 优化）
//! 没有安全的中断点，不登记，取消请求会如实回"这一步无法中途停止"。
//!
//! 三个状态原来是三把互相独立的 `Mutex<HashSet>`，`try_start` 要先后锁两把、`request_cancel` 锁三把，非原子；
//! 合成一张 `HashMap<条目, OpState>` 后每个操作都是一次持锁，且"结束＝整条移除"不会漏清某个标记。

use rmsvc_core::sync::lock;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct OpState {
    /// 当前这步会检查取消标记。
    cancellable: bool,
    /// 用户已请求取消。
    cancel: bool,
}

#[derive(Clone, Default)]
pub struct OpRegistry(Arc<Mutex<HashMap<String, OpState>>>);

impl OpRegistry {
    pub fn is_busy(&self, name: &str) -> bool {
        lock(&self.0).contains_key(name)
    }

    /// 尝试给条目加忙锁；已经忙着 → false（调用方据此拒绝这次操作，不排队不覆盖）。新操作总从干净状态开始，
    /// 上一轮遗留的取消标记不会带进来。
    pub fn try_start(&self, name: &str) -> bool {
        let mut g = lock(&self.0);
        if g.contains_key(name) {
            return false;
        }
        g.insert(name.to_string(), OpState::default());
        true
    }

    /// 结束操作：忙锁、取消标记、可取消声明一并清掉。
    pub fn end(&self, name: &str) {
        lock(&self.0).remove(name);
    }

    /// 当前这步声明"我会检查取消标记"。只对正在跑异步操作的条目生效（同步调用方不登记，免得残留）。
    pub fn mark_cancellable(&self, name: &str) {
        if let Some(s) = lock(&self.0).get_mut(name) {
            s.cancellable = true;
        }
    }

    pub fn is_cancelled(&self, name: &str) -> bool {
        lock(&self.0).get(name).is_some_and(|s| s.cancel)
    }

    /// 请求取消。`Ok(true)`＝已登记，会在下一个检查点停下；`Ok(false)`＝这一步无法中途停止；`Err`＝当前没有在处理。
    pub fn request_cancel(&self, name: &str) -> Result<bool, String> {
        let mut g = lock(&self.0);
        let Some(s) = g.get_mut(name) else { return Err("这本书当前没有在处理".into()) };
        if !s.cancellable {
            return Ok(false);
        }
        s.cancel = true;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_is_exclusive_and_end_clears_everything() {
        let r = OpRegistry::default();
        assert!(r.try_start("a.epub"));
        assert!(!r.try_start("a.epub"), "同一条目不能同时跑两个操作");
        assert!(r.is_busy("a.epub") && !r.is_busy("b.epub"));
        r.mark_cancellable("a.epub");
        r.request_cancel("a.epub").unwrap();
        assert!(r.is_cancelled("a.epub"));
        r.end("a.epub");
        assert!(!r.is_busy("a.epub") && !r.is_cancelled("a.epub"));
        assert!(r.try_start("a.epub"));
        assert!(!r.is_cancelled("a.epub"), "上一轮的取消标记不带进新操作");
    }

    #[test]
    fn cancel_is_three_state() {
        let r = OpRegistry::default();
        assert!(r.request_cancel("x").is_err(), "没在处理");
        r.try_start("x");
        assert_eq!(r.request_cancel("x"), Ok(false), "这一步没登记可取消");
        r.mark_cancellable("x");
        assert_eq!(r.request_cancel("x"), Ok(true));
        r.mark_cancellable("ghost"); // 没在跑的条目不留残留
        assert!(!r.is_busy("ghost"));
    }

    #[test]
    fn poisoned_lock_does_not_cascade() {
        let r = OpRegistry::default();
        let r2 = r.clone();
        let _ = std::thread::spawn(move || {
            let _g = lock(&r2.0);
            panic!("boom");
        })
        .join();
        assert!(r.try_start("a"), "poison 后仍可用");
    }
}
