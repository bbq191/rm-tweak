//! 时钟：unix 时间戳的唯一出处。此前 8 处各写一遍 `SystemTime::now().duration_since(UNIX_EPOCH)…unwrap_or(0)`
//! （事件时间、证书签发、会话盐、备份戳、边车落库秒、渲染自检毫秒、随机壁纸种子），收编于此（2026-09-06 体检）。
//! 取不到系统时间（理论上只有时钟早于 1970）一律回 0，与旧行为一致。
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn since_epoch() -> Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO)
}

/// unix 秒。
pub fn now_secs() -> u64 {
    since_epoch().as_secs()
}

/// unix 毫秒。
pub fn now_ms() -> u64 {
    since_epoch().as_millis() as u64
}

/// unix 纳秒（随机种子 / 会话盐用；不作时间戳）。
pub fn now_nanos() -> u128 {
    since_epoch().as_nanos()
}

/// 某个 `SystemTime`（如文件 mtime）的 unix 秒；早于 1970 回 0。
pub fn secs_of(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotone_and_units_consistent() {
        let s = now_secs();
        let ms = now_ms();
        assert!(s > 1_700_000_000, "2023 年以后");
        assert!(ms / 1000 >= s && ms / 1000 <= s + 1);
        assert!(now_nanos() / 1_000_000_000 >= s as u128);
        assert_eq!(secs_of(UNIX_EPOCH + Duration::from_secs(1234)), 1234);
        assert_eq!(secs_of(UNIX_EPOCH - Duration::from_secs(1)), 0);
    }
}
