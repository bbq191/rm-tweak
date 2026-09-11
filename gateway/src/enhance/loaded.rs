//! 扩展**实际**有没有加载进 xochitl（2026-09-24 审查补）：网页开关只反映 `reading-qol.json` 里的配置，看不出
//! `.so` 到底进没进进程——历史上两次"开关看着开了、其实没生效"（09-09 langhook 整个从设备上消失；GLIBC
//! 版本不符让 hw-stroke 静默加载失败）。这里直接看 xochitl 主进程的 `/proc/<pid>/maps`：映射了哪个
//! `extensions.d/*.so` 就是真加载了。
//!
//! qmd 补丁（如漫画页边距的 `shelf-comic-margins.qmd`）不是 `.so`，由 qt-resource-rebuilder 在 xochitl **启动时**读
//! 一次：qt-resource-rebuilder.so 在主进程里、qmd 文件在它的 exthome 目录、且文件修改时间早于 xochitl 启动时间，
//! 才算已载入；文件比进程新 = 改过但还没重启 xochitl，报"待重启"。这是按加载机制推断，看不到 qmd 里的
//! LOCATE 是否全部命中。
//!
//! 主进程判定：`comm == "xochitl"` 且父进程是 1（systemd）。xochitl 渲染 PDF 时会 fork 出同名的
//! worker 子进程，父进程不是 1，排除。
use serde::Serialize;
use std::path::Path;

#[derive(Serialize, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Loaded {
    /// 找到了 xochitl 主进程。
    pub xochitl: bool,
    /// 主进程里映射了 xovi.so（LD_PRELOAD 生效）。
    pub xovi: bool,
    /// 已映射的 `extensions.d/` 下的扩展文件名（去重、排序），如 `hl-snap.so`。
    pub extensions: Vec<String>,
    /// 启动时已载入的 qmd 补丁文件名（排序）。
    pub qmds: Vec<String>,
    /// 存在但比 xochitl 进程新的 qmd（改过、还没重启 xochitl 生效）。
    pub qmds_pending: Vec<String>,
}

/// `/proc/<pid>/stat` 里本模块要的三列：comm、ppid、starttime（开机后的时钟滴答）。
/// 格式 "pid (comm) state ppid …"——comm 可能含空格，从最后一个 ')' 之后切。
fn parse_stat(stat: &str) -> Option<(&str, &str, u64)> {
    let (head, rest) = stat.rsplit_once(')')?;
    let comm = head.split_once('(')?.1;
    let mut f = rest.split_whitespace();
    let ppid = f.nth(1)?;
    let ticks = f.nth(17)?.parse().ok()?; // 第 22 列：ppid 是第 4 列，再往后 18 列
    Some((comm, ppid, ticks))
}

/// 读 `pid` 的 stat，是 xochitl 主进程（comm 为 xochitl 且父进程是 1）就返回它的 starttime。
fn main_xochitl_ticks(proc_root: &Path, pid: &str) -> Option<u64> {
    let stat = std::fs::read_to_string(proc_root.join(pid).join("stat")).ok()?;
    let (comm, ppid, ticks) = parse_stat(&stat)?;
    (comm == "xochitl" && ppid == "1").then_some(ticks)
}

/// 全量找 xochitl 主进程：遍历 `/proc` 下的数字目录，每个只读一次 `stat`（comm 就在里面）。
fn find_main_xochitl(proc_root: &Path) -> Option<(String, u64)> {
    std::fs::read_dir(proc_root).ok()?.flatten().find_map(|e| {
        let pid = e.file_name().to_str()?.to_string();
        if !pid.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        main_xochitl_ticks(proc_root, &pid).map(|t| (pid, t))
    })
}

/// 进程启动时刻（unix 秒）：starttime 滴答（arm64/x86 的 USER_HZ 都是 100）+ `/proc/stat` 的 `btime`。
fn start_secs(proc_root: &Path, ticks: u64) -> Option<u64> {
    let btime: u64 = std::fs::read_to_string(proc_root.join("stat")).ok()?.lines().find_map(|l| l.strip_prefix("btime ")?.trim().parse().ok())?;
    Some(btime + ticks / 100)
}

/// `qrr_dir`（qt-resource-rebuilder 的 exthome）里的 `*.qmd`：按"修改时间是否早于 `start`"分成已载入 / 待重启。
fn qmd_states(qrr_dir: &Path, start: u64) -> (Vec<String>, Vec<String>) {
    let (mut done, mut pending) = (Vec::new(), Vec::new());
    for e in std::fs::read_dir(qrr_dir).into_iter().flatten().flatten() {
        let Some(name) = e.file_name().to_str().map(str::to_string).filter(|n| n.ends_with(".qmd")) else { continue };
        let mtime = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        if mtime <= start { done.push(name) } else { pending.push(name) }
    }
    done.sort();
    pending.sort();
    (done, pending)
}

/// 一个 xochitl 主进程的映射结果（按 pid + starttime 认同一个进程）。
struct Mapped {
    pid: String,
    ticks: u64,
    start: Option<u64>,
    xovi: bool,
    extensions: Vec<String>,
}

/// 进程启动后多久才把映射结果缓存下来：xovi 在 xochitl 启动时逐个加载扩展，刚起的进程映射可能还不全。
const SETTLE_SECS: u64 = 30;

/// 扫描结果缓存。`/api/enhance/status`（管理页每次刷新、笔记页每次刷新都会调）原来每次都遍历整个 `/proc`、
/// 逐个读 comm，再把 xochitl 上百 KB 的 `maps` 整个读一遍；而扩展只在 xochitl 启动时加载，同一个进程的映射结果
/// 不会变。缓存命中时只读一次 `/proc/<pid>/stat` 核对还是同一个进程（pid + starttime），qmd 状态照旧现算
/// （看的是文件修改时间，随时会变）。
pub struct Scanner(std::sync::Mutex<Option<Mapped>>);

impl Scanner {
    pub const fn new() -> Scanner {
        Scanner(std::sync::Mutex::new(None))
    }

    pub fn scan(&self, proc_root: &Path, qrr_dir: &Path) -> Loaded {
        let mut cache = rmsvc_core::sync::lock(&self.0);
        let hit = cache.as_ref().is_some_and(|m| main_xochitl_ticks(proc_root, &m.pid) == Some(m.ticks));
        if !hit {
            *cache = None;
            let Some((pid, ticks)) = find_main_xochitl(proc_root) else { return Loaded::default() };
            let maps = std::fs::read_to_string(proc_root.join(&pid).join("maps")).unwrap_or_default();
            let mut extensions: Vec<String> = maps
                .lines()
                .filter_map(|l| l.split_whitespace().nth(5))
                .filter_map(|p| p.split_once("/extensions.d/").map(|(_, f)| f.to_string()))
                .collect();
            extensions.sort();
            extensions.dedup();
            let m = Mapped { start: start_secs(proc_root, ticks), xovi: maps.lines().any(|l| l.ends_with("/xovi.so")), pid, ticks, extensions };
            let settled = m.start.is_some_and(|s| rmsvc_core::clock::now_secs().saturating_sub(s) >= SETTLE_SECS);
            let loaded = m.loaded(qrr_dir);
            if settled {
                *cache = Some(m);
            }
            return loaded;
        }
        cache.as_ref().map(|m| m.loaded(qrr_dir)).unwrap_or_default()
    }
}

impl Mapped {
    fn loaded(&self, qrr_dir: &Path) -> Loaded {
        let (qmds, qmds_pending) = match (self.extensions.iter().any(|e| e == "qt-resource-rebuilder.so"), self.start) {
            (true, Some(start)) => qmd_states(qrr_dir, start),
            _ => (vec![], vec![]),
        };
        Loaded { xochitl: true, xovi: self.xovi, extensions: self.extensions.clone(), qmds, qmds_pending }
    }
}

/// 不带缓存的一次扫描（测试用）。
#[cfg(test)]
pub fn scan(proc_root: &Path, qrr_dir: &Path) -> Loaded {
    Scanner::new().scan(proc_root, qrr_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc_entry(root: &Path, pid: &str, comm: &str, ppid: &str, maps: &str) {
        let d = root.join(pid);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("comm"), format!("{comm}\n")).unwrap();
        // 第 22 列 starttime = 1000 滴答（10 秒）
        std::fs::write(d.join("stat"), format!("{pid} ({comm}) S {ppid} 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 1000 0 0")).unwrap();
        std::fs::write(d.join("maps"), maps).unwrap();
    }

    #[test]
    fn finds_main_process_and_its_mapped_extensions() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        std::fs::create_dir_all(r.join("self")).unwrap();
        proc_entry(r, "839", "xochitl", "613", "7f00-7f01 r-xp 0 00:00 1 /home/root/xovi/extensions.d/bogus.so\n");
        proc_entry(r, "613", "xochitl", "1", "7f00-7f01 r-xp 00000000 b3:04 11 /home/root/xovi/xovi.so\n\
7f02-7f03 r-xp 00000000 b3:04 12 /home/root/xovi/extensions.d/hw-stroke.so\n\
7f04-7f05 r--p 00001000 b3:04 12 /home/root/xovi/extensions.d/hw-stroke.so\n\
7f06-7f07 r-xp 00000000 b3:04 13 /home/root/xovi/extensions.d/hl-snap.so\n\
7f08-7f09 rw-p 00000000 00:00 0 \n");
        proc_entry(r, "700", "sh", "1", "");
        let l = scan(r, &r.join("none"));
        assert!(l.xochitl && l.xovi);
        assert_eq!(l.extensions, ["hl-snap.so", "hw-stroke.so"], "去重排序、不含 worker 子进程的映射");
        assert!(l.qmds.is_empty() && l.qmds_pending.is_empty(), "没有 qt-resource-rebuilder 就不报 qmd");
    }

    #[test]
    fn qmds_split_by_xochitl_start_time() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path().join("proc");
        let qrr = t.path().join("qrr");
        std::fs::create_dir_all(&qrr).unwrap();
        proc_entry(&r, "613", "xochitl", "1", "7f00-7f01 r-xp 0 00:00 1 /home/root/xovi/extensions.d/qt-resource-rebuilder.so\n");
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        // 进程启动于 now-60（btime = now-70，starttime = 10 秒）
        std::fs::write(r.join("stat"), format!("cpu 0\nbtime {}\n", now - 70)).unwrap();
        for (n, age) in [("old.qmd", 3600u64), ("new.qmd", 0), ("x.qmd.bak", 3600)] {
            let f = qrr.join(n);
            std::fs::write(&f, "").unwrap();
            let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(now - age);
            std::fs::File::options().write(true).open(&f).unwrap().set_modified(when).unwrap();
        }
        let l = scan(&r, &qrr);
        assert_eq!(l.qmds, ["old.qmd"]);
        assert_eq!(l.qmds_pending, ["new.qmd"], "比进程新的 = 待重启；.bak 不算");
    }

    #[test]
    fn no_xochitl_or_no_xovi() {
        let t = tempfile::tempdir().unwrap();
        assert_eq!(scan(t.path(), t.path()), Loaded::default());
        proc_entry(t.path(), "5", "xochitl", "1", "7f00-7f01 r-xp 0 00:00 1 /usr/bin/xochitl\n");
        let l = scan(t.path(), t.path());
        assert!(l.xochitl && !l.xovi && l.extensions.is_empty());
    }

    /// 缓存：同一个进程（pid + starttime）第二次不再读 maps；进程换了（starttime 变）就重扫；刚起的进程不缓存。
    #[test]
    fn caches_mapping_per_process_and_rescans_on_restart() {
        let t = tempfile::tempdir().unwrap();
        let r = t.path();
        let now = rmsvc_core::clock::now_secs();
        std::fs::write(r.join("stat"), format!("cpu 0\nbtime {}\n", now - 3600)).unwrap(); // 进程已跑了约 1 小时
        proc_entry(r, "613", "xochitl", "1", "7f00-7f01 r-xp 0 00:00 1 /home/root/xovi/extensions.d/hl-snap.so\n");
        let sc = Scanner::new();
        assert_eq!(sc.scan(r, r).extensions, ["hl-snap.so"]);
        std::fs::write(r.join("613/maps"), "").unwrap(); // 缓存命中时不会再读 maps
        assert_eq!(sc.scan(r, r).extensions, ["hl-snap.so"], "同一进程命中缓存");
        // 同 pid 但 starttime 变了（xochitl 重启后恰好复用 pid）：重扫
        std::fs::write(r.join("613/stat"), "613 (xochitl) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 2000 0 0").unwrap();
        assert!(sc.scan(r, r).extensions.is_empty(), "进程变了要重扫");
        // 进程退出：不再报 xochitl
        std::fs::remove_dir_all(r.join("613")).unwrap();
        assert!(!sc.scan(r, r).xochitl);
        // 刚启动的进程（不到 SETTLE_SECS）不缓存：之后映射变了能看到
        std::fs::write(r.join("stat"), format!("cpu 0\nbtime {}\n", now - 12)).unwrap();
        proc_entry(r, "700", "xochitl", "1", "");
        assert!(sc.scan(r, r).extensions.is_empty());
        std::fs::write(r.join("700/maps"), "7f00-7f01 r-xp 0 00:00 1 /home/root/xovi/extensions.d/hw-stroke.so\n").unwrap();
        assert_eq!(sc.scan(r, r).extensions, ["hw-stroke.so"], "启动初期的结果不该被缓存住");
    }

    #[test]
    fn parse_stat_handles_spaces_in_comm() {
        assert_eq!(parse_stat("5 (a b) c) S 1 5 5 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 777 0"), Some(("a b) c", "1", 777)));
        assert_eq!(parse_stat("garbage"), None);
    }
}
