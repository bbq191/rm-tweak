//! 单值 TTL 缓存：给"每次网页 refresh 都会打一遍、但算一次很重"的状态接口用（如 `/status` 里发 HTTP 探活、
//! 读全部 `.metadata`、遍历 `/proc`）。按时间失效；状态会被本服务自己的操作改变的字段，操作完成路径里
//! 调 [`TtlCache::invalidate`] 主动失效，保证"操作完马上刷新能看到变化"。
//!
//! 计算期间持锁：并发的请求排队等同一份结果，而不是各自重算一遍（重活恰恰是这个缓存要省的）。
//! `invalidate` 也要拿同一把锁，所以"计算进行中被操作打断"的情形下，失效发生在这次计算结束之后，
//! 不会被这次（可能已过时的）计算结果盖掉。
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct TtlCache<T> {
    ttl: Duration,
    slot: Mutex<Option<(Instant, T)>>,
}

impl<T: Clone> TtlCache<T> {
    pub fn new(ttl: Duration) -> TtlCache<T> {
        TtlCache { ttl, slot: Mutex::new(None) }
    }

    /// 缓存没过期就直接给，否则调 `compute` 重算并存下。
    pub fn get_or(&self, compute: impl FnOnce() -> T) -> T {
        self.get_or_at(Instant::now(), compute)
    }

    /// 同 [`Self::get_or`]，"现在"由调用方给（单测用，不用真睡觉）。
    pub fn get_or_at(&self, now: Instant, compute: impl FnOnce() -> T) -> T {
        let mut g = crate::sync::lock(&self.slot);
        if let Some((at, v)) = g.as_ref() {
            if now.saturating_duration_since(*at) < self.ttl {
                return v.clone();
            }
        }
        let v = compute();
        *g = Some((now, v.clone()));
        v
    }

    /// 让下一次 `get_or` 必定重算（操作改变了缓存里的内容之后调）。
    pub fn invalidate(&self) {
        *crate::sync::lock(&self.slot) = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn hits_within_ttl_and_recomputes_after_expiry() {
        let c = TtlCache::new(Duration::from_secs(3));
        let calls = Cell::new(0);
        let t0 = Instant::now();
        let get = |at: Instant| c.get_or_at(at, || { calls.set(calls.get() + 1); calls.get() });
        assert_eq!(get(t0), 1);
        assert_eq!(get(t0 + Duration::from_secs(2)), 1, "TTL 内命中，不重算");
        assert_eq!(get(t0 + Duration::from_secs(3)), 2, "到期重算");
        assert_eq!(get(t0 + Duration::from_secs(4)), 2, "新值从重算时刻起重新计时");
    }

    #[test]
    fn invalidate_forces_recompute_even_within_ttl() {
        let c = TtlCache::new(Duration::from_secs(60));
        let n = Cell::new(0);
        assert_eq!(c.get_or(|| { n.set(n.get() + 1); n.get() }), 1);
        assert_eq!(c.get_or(|| { n.set(n.get() + 1); n.get() }), 1);
        c.invalidate();
        assert_eq!(c.get_or(|| { n.set(n.get() + 1); n.get() }), 2, "操作后失效，马上看到新值");
    }
}
