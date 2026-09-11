//! 图片并行处理的内存预算与线程数。
//!
//! 设备是双核 A55，单线程处理一本 350 页漫画约 285 秒（2026-09-20 真机实测）。图片逐张独立，可以并行；
//! 但**绝不能 OOM**：同时在处理的图片总像素受 [`PIXEL_BUDGET`] 限制——每张图开工前按头部声明的像素数
//! 申请额度，额度不够就等；单张图超过总预算时独占全部额度（此时不与别的图并行）。这样最坏情况的峰值
//! 不会比原来单线程处理一张 900 万像素图（[`crate::imgopt`] 的 `MAX_DECODE_PIXELS`）更高，
//! 典型 170 万像素的漫画页两张并行只多占几十 MB。
//! 并行不改变任何一张图的处理结果（每张仍是同一个纯函数），输出按原条目顺序写，逐字节可复现。

use std::sync::{Condvar, Mutex};

/// 同时在处理的图片总像素上限。实测单张按 ~12 字节/像素（解码+裁边副本+缩放+画布+编码缓冲）估，
/// 600 万像素 ≈ 70MB 上限；两张典型漫画页（各 ~170 万像素）合计 ~340 万，远低于它，可以并行。
pub const PIXEL_BUDGET: u64 = 6_000_000;

/// 并行工作线程数：取 CPU 核数，封顶 2（设备只有 2 核；再多只会加内存不加速）。
pub fn worker_count() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1).clamp(1, 2)
}

pub struct PixelBudget {
    cap: u64,
    used: Mutex<u64>,
    cv: Condvar,
}

pub struct Permit<'a> {
    budget: &'a PixelBudget,
    n: u64,
}

impl PixelBudget {
    pub fn new(cap: u64) -> PixelBudget {
        PixelBudget { cap, used: Mutex::new(0), cv: Condvar::new() }
    }

    /// 申请 `px` 像素的额度（超过总预算按总预算算，即独占）。阻塞到有额度；返回的 [`Permit`] 析构时归还。
    pub fn acquire(&self, px: u64) -> Permit<'_> {
        let need = px.clamp(1, self.cap);
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        while *used + need > self.cap {
            used = self.cv.wait(used).unwrap_or_else(|e| e.into_inner());
        }
        *used += need;
        Permit { budget: self, n: need }
    }
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        let mut used = self.budget.used.lock().unwrap_or_else(|e| e.into_inner());
        *used -= self.n;
        self.budget.cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Arc;

    #[test]
    fn budget_never_exceeds_cap_under_contention() {
        let b = Arc::new(PixelBudget::new(1000));
        let (cur, peak) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let hs: Vec<_> = (0..8)
            .map(|_| {
                let (b, cur, peak) = (b.clone(), cur.clone(), peak.clone());
                std::thread::spawn(move || {
                    for _ in 0..50 {
                        let _p = b.acquire(400);
                        let now = cur.fetch_add(400, Ordering::SeqCst) + 400;
                        peak.fetch_max(now, Ordering::SeqCst);
                        std::thread::yield_now();
                        cur.fetch_sub(400, Ordering::SeqCst);
                    }
                })
            })
            .collect();
        hs.into_iter().for_each(|h| h.join().unwrap());
        assert!(peak.load(Ordering::SeqCst) <= 1000, "同时占用的像素超过了预算: {}", peak.load(Ordering::SeqCst));
    }

    #[test]
    fn oversized_image_takes_whole_budget_and_still_completes() {
        let b = PixelBudget::new(100);
        let p = b.acquire(10_000); // 超过总预算：独占，不死锁
        drop(p);
        let _q = b.acquire(100);
    }

    #[test]
    fn worker_count_is_between_1_and_2() {
        assert!((1..=2).contains(&worker_count()));
    }
}
