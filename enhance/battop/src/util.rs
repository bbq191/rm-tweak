//! 小工具：TSV/JSON 转义、epoch→日期、本地时区偏移。全部无外部依赖。

/// 当前 epoch 秒（时钟早于 1970 时按 0，不 panic——常驻服务里 panic=abort 会被 systemd 拉起循环重启）。
pub fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// TSV 安全:去掉 tab/换行。
pub fn san(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\n' { ' ' } else { c }).collect()
}

/// JSON 字符串转义（不含两侧引号）。控制字符：\n \t \r 折成空格（面板不需要），其余 <0x20 走 \u00XX，
/// 否则 comm 里混进控制字符会让整份 summary.json 变成非法 JSON、面板整页读不出来。
pub fn json_esc(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' | '\t' | '\r' => o.push(' '),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            _ => o.push(c),
        }
    }
    o
}

/// epoch(秒,UTC)→ YYYYMMDD(仅用于文件按天滚动;窗口计算靠行内 epoch)。
pub fn ymd(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = y + if m <= 2 { 1 } else { 0 };
    format!("{:04}{:02}{:02}", y, m, d)
}

/// glibc/musl 在 64 位 Linux 上一致的 `struct tm`（含 BSD 扩展 tm_gmtoff/tm_zone）。
#[repr(C)]
struct Tm {
    fields: [i32; 9], // sec min hour mday mon year wday yday isdst
    gmtoff: std::os::raw::c_long,
    zone: *const std::os::raw::c_char,
}

extern "C" {
    fn localtime_r(t: *const std::os::raw::c_long, out: *mut Tm) -> *mut Tm;
}

/// 本地时区相对 UTC 的偏移（秒）。直接问 libc `localtime_r`（读 /etc/localtime，std 本来就链着 libc），
/// 取代旧的每轮 fork 一次 `date +%z`——采样进程每 ~10min 一次的子进程创建（常驻服务里 fork 会短暂
/// 复制地址空间、走一遍 cgroup 归属）没有存在的理由。失败回落 CST +8h（旧行为）。
pub fn local_offset(now: u64) -> i64 {
    let t = now as std::os::raw::c_long;
    let mut tm = Tm { fields: [0; 9], gmtoff: 0, zone: std::ptr::null() };
    // SAFETY: t/tm 均为本栈上有效对象；localtime_r 只写 tm、线程安全（无静态缓冲）。tm 布局见上。
    let r = unsafe { localtime_r(&t, &mut tm) };
    if r.is_null() {
        8 * 3600
    } else {
        tm.gmtoff as i64
    }
}

#[repr(C)]
struct Timespec {
    tv_sec: std::os::raw::c_long,
    tv_nsec: std::os::raw::c_long,
}

extern "C" {
    fn clock_gettime(clk: std::os::raw::c_int, tp: *mut Timespec) -> std::os::raw::c_int;
}

/// CLOCK_MONOTONIC 的秒数（开机以来、**不含休眠**；与内核 kmsg 时间戳同样不含休眠，wake.rs 用它换算墙钟）。
/// `std::time::Instant` 不暴露绝对值，只好直接问 libc（同 [`local_offset`]）。失败回 0。
pub fn monotonic_secs() -> u64 {
    const CLOCK_MONOTONIC: std::os::raw::c_int = 1;
    let mut ts = Timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: ts 为本栈上有效对象，clock_gettime 只写它；glibc/musl 64 位 Linux 上 timespec 均为两个 long。
    if unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) } != 0 || ts.tv_sec < 0 {
        return 0;
    }
    ts.tv_sec as u64
}

/// 测试用临时目录（纯 std，battop 保持零依赖；Drop 时清理）。
#[cfg(test)]
pub mod testutil {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    pub struct Tmp(PathBuf);
    impl Tmp {
        pub fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    pub fn tmp() -> Tmp {
        static N: AtomicU32 = AtomicU32::new(0);
        let p = std::env::temp_dir().join(format!("battop-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&p).unwrap();
        Tmp(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ymd_known_dates() {
        assert_eq!(ymd(0), "19700101");
        assert_eq!(ymd(1_760_000_000), "20251009");
        assert_eq!(ymd(951_782_400), "20000229"); // 闰日
        assert_eq!(ymd(1_709_164_799), "20240228");
        assert_eq!(ymd(1_709_164_800), "20240229");
    }

    #[test]
    fn json_esc_control_chars_stay_valid() {
        assert_eq!(json_esc("a\"b\\c\nd\te"), "a\\\"b\\\\c d e");
        assert_eq!(json_esc("x\u{1}y"), "x\\u0001y");
        assert_eq!(json_esc("霞鹜"), "霞鹜");
    }

    #[test]
    fn san_strips_tab_newline() {
        assert_eq!(san("a\tb\nc"), "a b c");
    }

    #[test]
    fn monotonic_not_ahead_of_boottime_uptime() {
        let m = monotonic_secs();
        let up = crate::procs::read_uptime();
        assert!(m > 0, "单调时钟应已走过 0");
        assert!(m <= up + 1, "单调时钟不含休眠，不会超过含休眠的 uptime: mono={m} up={up}");
    }

    /// 与 `date +%z` 对拍（host 有 date；进程未改 TZ 时两者同源于 /etc/localtime 或 TZ）。
    #[test]
    fn local_offset_matches_date_z() {
        let off = local_offset(1_760_000_000);
        assert!((-12 * 3600..=14 * 3600).contains(&off), "偏移应在合法时区范围: {off}");
        if let Ok(o) = std::process::Command::new("date").args(["-d", "@1760000000", "+%z"]).output() {
            let s = String::from_utf8_lossy(&o.stdout);
            let s = s.trim();
            if s.len() == 5 && s.is_char_boundary(3) {
                let sec = s[1..3].parse::<i64>().unwrap() * 3600 + s[3..5].parse::<i64>().unwrap() * 60;
                assert_eq!(off, if s.starts_with('-') { -sec } else { sec });
            }
        }
    }
}
