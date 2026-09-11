//! 落盘：baseline.tsv（上次每 (pid,starttime) 的累计 ticks）、按天滚动的 samples-YYYYMMDD.tsv、旧样本清理，
//! 以及「本次 vs baseline → 增量 → 按 comm/unit 双聚合」的纯计算。
use crate::procs::Proc;
use crate::util::{san, ymd};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::time::UNIX_EPOCH;

pub const RETAIN_DAYS: u64 = 40;
/// 真机 getconf CLK_TCK = 100(aarch64 Linux 常量)
pub const CLK_TCK: u64 = 100;

pub type Baseline = HashMap<(u64, u64), u64>;

/// 载入 baseline；返回 (是否有历史, 表)。
pub fn load_baseline(dir: &Path) -> (bool, Baseline) {
    match fs::read_to_string(dir.join("baseline.tsv")) {
        Ok(c) => (true, parse_baseline(&c)),
        Err(_) => (false, HashMap::new()),
    }
}

fn parse_baseline(content: &str) -> Baseline {
    let mut m = HashMap::new();
    for line in content.lines() {
        if line.starts_with('_') {
            continue; // meta
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() < 3 {
            continue;
        }
        if let (Ok(pid), Ok(st), Ok(t)) = (f[0].parse(), f[1].parse(), f[2].parse()) {
            m.insert((pid, st), t);
        }
    }
    m
}

pub fn write_baseline(dir: &Path, now: u64, cap: i64, procs: &[Proc]) -> std::io::Result<()> {
    let tmp = dir.join("baseline.tsv.tmp");
    let mut s = format!("_ts\t{now}\t_cap\t{cap}\n");
    for p in procs {
        // pid \t starttime \t ticks \t comm \t unit
        s.push_str(&format!("{}\t{}\t{}\t{}\t{}\n", p.pid, p.starttime, p.ticks, san(&p.comm), san(&p.unit)));
    }
    fs::write(&tmp, s)?;
    fs::rename(&tmp, dir.join("baseline.tsv"))
}

/// 距上次的增量按 comm(进程)与 unit(应用)双聚合（ticks）。新进程(不在 baseline)= 距上次全部是它攒的
/// → 增量即当前累计。
pub fn aggregate(procs: &[Proc], base: &Baseline) -> (HashMap<String, u64>, HashMap<String, u64>) {
    let mut by_comm: HashMap<String, u64> = HashMap::new();
    let mut by_unit: HashMap<String, u64> = HashMap::new();
    for p in procs {
        let delta = match base.get(&(p.pid, p.starttime)) {
            Some(pv) => p.ticks.saturating_sub(*pv),
            None => p.ticks,
        };
        if delta == 0 {
            continue;
        }
        *by_comm.entry(p.comm.clone()).or_insert(0) += delta;
        *by_unit.entry(p.unit.clone()).or_insert(0) += delta;
    }
    (by_comm, by_unit)
}

/// 追加样本行:epoch \t kind \t key \t cpu_ms \t disc
pub fn sample_rows(now: u64, disc: i32, by_comm: &HashMap<String, u64>, by_unit: &HashMap<String, u64>) -> String {
    let mut out = String::new();
    let ms = |ticks: u64| ticks * 1000 / CLK_TCK;
    for (k, t) in by_comm {
        out.push_str(&format!("{now}\tproc\t{}\t{}\t{disc}\n", san(k), ms(*t)));
    }
    for (k, t) in by_unit {
        out.push_str(&format!("{now}\tapp\t{}\t{}\t{disc}\n", san(k), ms(*t)));
    }
    out
}

pub fn sample_file(dir: &Path, now: u64) -> std::path::PathBuf {
    dir.join(format!("samples-{}.tsv", ymd(now)))
}

pub fn append(path: &Path, data: &str) -> std::io::Result<()> {
    let mut f = fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(data.as_bytes())
}

/// 是否为样本文件名 `samples-*.tsv`。
pub fn is_sample_file(name: &str) -> bool {
    name.starts_with("samples-") && name.ends_with(".tsv")
}

/// 清理 RETAIN_DAYS 天前的旧样本文件（按 mtime）。
pub fn prune(dir: &Path, now: u64) {
    let cutoff = now.saturating_sub(RETAIN_DAYS * 86400);
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        if !is_sample_file(&e.file_name().to_string_lossy()) {
            continue;
        }
        let old = e
            .metadata()
            .and_then(|md| md.modified())
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .is_some_and(|d| d.as_secs() < cutoff);
        if old {
            let _ = fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u64, st: u64, ticks: u64, comm: &str, unit: &str) -> Proc {
        Proc { pid, starttime: st, ticks, comm: comm.into(), unit: unit.into() }
    }

    #[test]
    fn baseline_roundtrip_and_skip_meta() {
        let t = crate::util::testutil::tmp();
        assert!(!load_baseline(t.path()).0, "无文件 = 无历史");
        write_baseline(t.path(), 100, 50, &[p(1, 10, 500, "a\tb", "u")]).unwrap();
        let (have, b) = load_baseline(t.path());
        assert!(have);
        assert_eq!(b.get(&(1, 10)), Some(&500));
        assert_eq!(b.len(), 1, "_ts 元行不算");
    }

    #[test]
    fn aggregate_deltas_new_and_reused_pid() {
        let mut base = Baseline::new();
        base.insert((1, 10), 100);
        base.insert((2, 20), 50);
        let procs = vec![
            p(1, 10, 130, "xochitl", "xochitl"), // +30
            p(2, 21, 8, "x", "u"),               // pid 复用(starttime 变了)=新进程，全部计入
            p(3, 30, 4, "xochitl", "xochitl"),   // 新进程 +4
            p(4, 40, 0, "idle", "u"),            // 0 增量不进表
            p(1, 10, 130, "dup", "z"),           // 同键重复出现按同 baseline 算 +30（不会发生，仅锁语义）
        ];
        let (c, u) = aggregate(&procs, &base);
        assert_eq!(c["xochitl"], 34);
        assert_eq!(u["xochitl"], 34);
        assert_eq!(c["x"], 8);
        assert!(!c.contains_key("idle"));
        // 计数回退（pid 复用同 starttime 不可能，但 ticks 变小）不下溢
        let (c2, _) = aggregate(&[p(1, 10, 5, "r", "r")], &base);
        assert!(!c2.contains_key("r"));
    }

    #[test]
    fn rows_convert_ticks_to_ms() {
        let mut c = HashMap::new();
        c.insert("a\tb".to_string(), 7u64);
        let rows = sample_rows(9, 1, &c, &HashMap::new());
        assert_eq!(rows, "9\tproc\ta b\t70\t1\n");
    }

    #[test]
    fn prune_removes_only_old_sample_files() {
        let t = crate::util::testutil::tmp();
        let old = t.path().join("samples-20200101.tsv");
        let keep = t.path().join("summary.json");
        fs::write(&old, "x").unwrap();
        fs::write(&keep, "x").unwrap();
        // 以「未来 100 天」为 now：刚建的文件 mtime 相对已超 40 天
        let now = crate::util::now_secs() + 100 * 86400;
        prune(t.path(), now);
        assert!(!old.exists());
        assert!(keep.exists(), "非 samples-*.tsv 不动");
        // 新文件不删
        let fresh = t.path().join("samples-20990101.tsv");
        fs::write(&fresh, "x").unwrap();
        prune(t.path(), crate::util::now_secs());
        assert!(fresh.exists());
    }
}
