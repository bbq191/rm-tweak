//! 一轮采样的原始输入：/proc 各进程 CPU 累计 + 归属 unit、电池 sysfs、uptime。只读 procfs/sysfs，不 fork。
use std::fs;

pub const BAT: &str = "/sys/class/power_supply/max77818_battery";

pub struct Proc {
    pub pid: u64,
    pub starttime: u64,
    pub ticks: u64,
    pub comm: String,
    pub unit: String,
}

/// 解析 `/proc/<pid>/stat` → (comm, utime+stime ticks, starttime)。
/// comm 在首个 '(' 与末个 ')' 之间(可含空格/括号)；')' 之后是 field3(state) 起的空格分隔字段。
pub fn parse_stat(stat: &str) -> Option<(String, u64, u64)> {
    let lp = stat.find('(')?;
    let rp = stat.rfind(')')?;
    if rp < lp {
        return None;
    }
    let comm = stat[lp + 1..rp].to_string();
    let rest: Vec<&str> = stat[rp + 1..].split_whitespace().collect();
    // field14 utime=idx11, field15 stime=idx12, field22 starttime=idx19
    if rest.len() < 20 {
        return None;
    }
    let utime: u64 = rest[11].parse().unwrap_or(0);
    let stime: u64 = rest[12].parse().unwrap_or(0);
    let starttime: u64 = rest[19].parse().unwrap_or(0);
    Some((comm, utime + stime, starttime))
}

pub fn sample_procs() -> Vec<Proc> {
    let mut v = Vec::new();
    let entries = match fs::read_dir("/proc") {
        Ok(e) => e,
        Err(_) => return v,
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        let pid: u64 = match name.parse() {
            Ok(p) => p,
            Err(_) => continue,
        };
        let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let Some((comm, ticks, starttime)) = parse_stat(&stat) else { continue };
        v.push(Proc { pid, starttime, ticks, comm, unit: read_unit(pid) });
    }
    v
}

/// 从 cgroup v2 取归属 unit;找不到 .service/.scope 则归 kernel/other。
fn read_unit(pid: u64) -> String {
    match fs::read_to_string(format!("/proc/{pid}/cgroup")) {
        Ok(cg) => unit_from_cgroup(&cg),
        Err(_) => "kernel".into(),
    }
}

/// `/proc/<pid>/cgroup` 内容 → unit 名。形如 "0::/system.slice/xochitl.service"。
pub fn unit_from_cgroup(cg: &str) -> String {
    let path = cg.trim().rsplit("::").next().unwrap_or("").trim();
    if path.is_empty() || path == "/" {
        return "kernel".into();
    }
    for seg in path.split('/').rev() {
        if seg.ends_with(".service") || seg.ends_with(".scope") {
            // 去掉 @实例后缀与 .service,取干净名
            let base = seg.trim_end_matches(".service").trim_end_matches(".scope");
            let base = base.split('@').next().unwrap_or(base);
            if !base.is_empty() {
                return base.to_string();
            }
        }
    }
    // 无 service:取叶子(如 user.slice)或 other
    path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("other").to_string()
}

pub struct Battery {
    /// 容量 %（读不到 -1）
    pub cap: i64,
    pub status: String,
    /// charge_now µAh 库仑计当前电量,下降量=精确放电 mAh(v3)
    pub charge: i64,
    /// current_now µA 瞬时
    pub current: i64,
}

pub fn read_battery() -> Battery {
    let read_i = |k: &str| -> i64 {
        fs::read_to_string(format!("{BAT}/{k}"))
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(-1)
    };
    Battery {
        cap: read_i("capacity"),
        status: fs::read_to_string(format!("{BAT}/status")).map(|s| s.trim().to_string()).unwrap_or_else(|_| "Unknown".into()),
        charge: read_i("charge_now"),
        current: read_i("current_now"),
    }
}

pub fn read_uptime() -> u64 {
    fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split('.').next().and_then(|x| x.trim().parse().ok()))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_with_parens_and_spaces_in_comm() {
        // pid (comm) S ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt utime stime ... starttime(22)
        let stat = "42 (a (b) c) S 1 42 42 0 -1 4194560 100 0 0 0 7 5 0 0 20 0 1 0 999 1000 200 18446744073709551615";
        let (comm, ticks, st) = parse_stat(stat).unwrap();
        assert_eq!(comm, "a (b) c");
        assert_eq!(ticks, 12);
        assert_eq!(st, 999);
    }

    #[test]
    fn stat_malformed_is_none() {
        assert!(parse_stat("").is_none());
        assert!(parse_stat(") (").is_none());
        assert!(parse_stat("1 (x) S 1 2 3").is_none());
    }

    #[test]
    fn cgroup_unit_mapping() {
        assert_eq!(unit_from_cgroup("0::/system.slice/xochitl.service\n"), "xochitl");
        assert_eq!(unit_from_cgroup("0::/system.slice/getty@tty1.service"), "getty");
        assert_eq!(unit_from_cgroup("0::/user.slice/user-0.slice/session-1.scope"), "session-1");
        assert_eq!(unit_from_cgroup("0::/"), "kernel");
        assert_eq!(unit_from_cgroup(""), "kernel");
        assert_eq!(unit_from_cgroup("0::/user.slice"), "user.slice");
    }

    #[test]
    fn sample_procs_sees_self() {
        let me = std::process::id() as u64;
        assert!(sample_procs().iter().any(|p| p.pid == me));
    }
}
