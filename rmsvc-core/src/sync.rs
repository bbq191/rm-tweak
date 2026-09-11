//! 同步原语小工具：容忍 poison 的取锁。
//!
//! 各服务 release 是 `panic=unwind`，处理函数 panic 由 HTTP 层 `catch_unwind` 兜住、进程照常服务；这时那条线程
//! 持有的 `Mutex` 会被标记 poison，若别处还写 `.lock().unwrap()`，之后每个请求都会跟着 panic。全仓库此前有六十来处
//! 手写 `.lock().unwrap_or_else(|e| e.into_inner())`（book-serve 另有一份私有 `ops::lock`），收编于此（2026-09-24 审计）。
//! 受保护的数据都是"缓存 / 队列 / 计数"这类即使上一个持有者半途 panic 也仍然自洽的状态，直接接着用即可。
use std::sync::{Mutex, MutexGuard};

/// 取锁；锁已 poison 时照样拿到里面的数据（见模块文档）。
pub fn lock<T: ?Sized>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poisoned_lock_is_still_usable() {
        let m = std::sync::Arc::new(Mutex::new(1));
        let m2 = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m2.lock().unwrap();
            panic!("boom");
        })
        .join();
        assert!(m.is_poisoned());
        *lock(&m) += 1;
        assert_eq!(*lock(&m), 2);
    }
}
