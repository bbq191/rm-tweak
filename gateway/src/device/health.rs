//! 「管理 → 设备健康」卡片的数据（2026-09-25）：只读，**用户打开这一屏 / 点刷新时才采集**，不轮询。
//!
//! 采集手段：读 `/proc`（uptime、进程 maps/status），加**一次** `systemctl show`（所有单元合在同一条命令里），
//! 再加一次 `journalctl -b -1`（上次开机的最后几行，冻结/意外重启的线索）——一个请求共 fork 两个子进程。结果进 [`super::HEALTH`] 的短 TTL 缓存，刷新按钮带 `?fresh=1` 跳过缓存。
//!
//! 为什么要这张卡：历史上几次"看着装了、其实没生效"都要 SSH 上去逐条查——xochitl 有没有带 xovi、换了 `.so`
//! 没重启（maps 里是 `(deleted)`）、待换入区里压着新版、某个服务反复重启、内存涨到哪、上次开机最后留下了什么。
//! 这里把这些"只读就能看出来"的东西摆在一处，不代替真机排查，只省掉第一轮 SSH。
use crate::manage::MODULES;
use rmsvc_core::paths::Paths;
use serde::Serialize;
use std::collections::HashMap;
use std::path::Path;

/// `systemctl show` 要的属性。时间戳都取 `…Monotonic`（开机后的微秒数），直接就是"开机后第几秒"。
const PROPS: &str = "Id,LoadState,ActiveState,SubState,NRestarts,MainPID,InactiveExitTimestampMonotonic,ActiveEnterTimestampMonotonic";

/// 上次开机 journal 取最后这么多行。
const PREV_BOOT_LINES: &str = "20";
/// `journalctl` 超时：只读持久化 journal 的尾部，正常一两秒内；卡住也不拖住请求线程太久。
const JOURNAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 要看的单元：xochitl 本体、开机让 xovi 生效的 `xovi-reenable`、网关自己，再加 [`MODULES`] 里的全部常驻服务
/// （单一事实源，不另写一份清单）。没装的单元 `LoadState=not-found`，前端照样列出来、标"未装"。
pub fn units() -> Vec<String> {
    let mut v: Vec<String> = ["xochitl.service", "xovi-reenable.service", "gateway.service"].iter().map(|s| s.to_string()).collect();
    v.extend(MODULES.iter().map(|m| format!("{}.service", m.service)));
    v
}

/// 一次 `systemctl show -p … <全部单元>`。失败（host 上没有 systemd、超时）返回空表，前端显示"读不到"。
pub fn systemctl_show(units: &[String]) -> Result<String, String> {
    let mut args: Vec<&str> = vec!["show", "-p", PROPS];
    args.extend(units.iter().map(|s| s.as_str()));
    crate::manage::run("systemctl", &args)
}

/// 解析多单元 `systemctl show` 输出：单元之间空行分隔，每行 `键=值`。按 `Id` 建表（`not-found` 的单元 `Id` 就是传入名）。
pub fn parse_show(text: &str) -> HashMap<String, HashMap<String, String>> {
    let mut out = HashMap::new();
    for block in text.split("\n\n") {
        let kv: HashMap<String, String> = block.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
        if let Some(id) = kv.get("Id").filter(|s| !s.is_empty()).cloned() {
            out.insert(id, kv);
        }
    }
    out
}

#[derive(Serialize, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct UnitHealth {
    pub unit: String,
    /// `loaded` / `not-found` / 空（systemctl 读不到）。
    pub load: String,
    pub active: String,
    pub sub: String,
    pub n_restarts: Option<u64>,
    pub pid: Option<u32>,
    pub rss_kb: Option<u64>,
    pub hwm_kb: Option<u64>,
    /// 最近一次进入 active 的时刻（开机后毫秒）。服务在开机后被重启过，这里就是重启那次，不是开机那次。
    pub started_at_ms: Option<u64>,
    /// 最近一次启动耗时（毫秒）= 进入 active − 离开 inactive（含 `ExecStartPre`；oneshot 含整个 `ExecStart`）。
    pub start_ms: Option<u64>,
}

fn num<T: std::str::FromStr>(kv: &HashMap<String, String>, k: &str) -> Option<T> {
    kv.get(k).and_then(|v| v.parse().ok())
}

/// 进程 `status` 里的 `VmRSS`/`VmHWM`（kB）。
fn vm_kb(proc_root: &Path, pid: u32) -> (Option<u64>, Option<u64>) {
    let Ok(s) = std::fs::read_to_string(proc_root.join(pid.to_string()).join("status")) else { return (None, None) };
    let field = |name: &str| s.lines().find_map(|l| l.strip_prefix(name)?.trim().strip_suffix("kB")?.trim().parse().ok());
    (field("VmRSS:"), field("VmHWM:"))
}

pub fn unit_health(proc_root: &Path, unit: &str, kv: Option<&HashMap<String, String>>) -> UnitHealth {
    let Some(kv) = kv else { return UnitHealth { unit: unit.to_string(), ..Default::default() } };
    let s = |k: &str| kv.get(k).cloned().unwrap_or_default();
    let pid = num::<u32>(kv, "MainPID").filter(|p| *p > 0);
    let (rss_kb, hwm_kb) = pid.map(|p| vm_kb(proc_root, p)).unwrap_or((None, None));
    let enter = num::<u64>(kv, "ActiveEnterTimestampMonotonic").filter(|v| *v > 0);
    let exit_inactive = num::<u64>(kv, "InactiveExitTimestampMonotonic").filter(|v| *v > 0);
    UnitHealth {
        unit: unit.to_string(),
        load: s("LoadState"),
        active: s("ActiveState"),
        sub: s("SubState"),
        n_restarts: num(kv, "NRestarts"),
        pid,
        rss_kb,
        hwm_kb,
        started_at_ms: enter.map(|v| v / 1000),
        start_ms: match (enter, exit_inactive) {
            (Some(a), Some(b)) if a >= b => Some((a - b) / 1000),
            _ => None,
        },
    }
}

/// xochitl 主进程 maps 里看到的东西。
#[derive(Serialize, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct XochitlMaps {
    /// 读到了 maps（进程在、有权限）。
    pub readable: bool,
    pub xovi: bool,
    /// 映射着的 `extensions.d/*.so`（去重排序）。
    pub extensions: Vec<String>,
    /// 其中文件已被删除/替换、进程还在用旧的那份（maps 行尾 ` (deleted)`）：换了文件但 xochitl 还没重启。
    pub deleted: Vec<String>,
}

pub fn xochitl_maps(proc_root: &Path, pid: Option<u32>) -> XochitlMaps {
    let Some(pid) = pid else { return XochitlMaps::default() };
    let Ok(maps) = std::fs::read_to_string(proc_root.join(pid.to_string()).join("maps")) else { return XochitlMaps::default() };
    parse_maps(&maps)
}

pub fn parse_maps(maps: &str) -> XochitlMaps {
    let mut m = XochitlMaps { readable: true, ..Default::default() };
    for line in maps.lines() {
        // maps 第 6 列起是路径，路径本身可能含空格，所以取第 5 个空白之后的全部。
        let Some(path) = line.split_whitespace().nth(5).map(|_| line.splitn(6, char::is_whitespace).last().unwrap_or("").trim()) else { continue };
        let (path, deleted) = match path.strip_suffix(" (deleted)") {
            Some(p) => (p, true),
            None => (path, false),
        };
        if path.ends_with("/xovi.so") {
            m.xovi = true;
        }
        if let Some((_, f)) = path.split_once("/extensions.d/") {
            m.extensions.push(f.to_string());
            if deleted {
                m.deleted.push(f.to_string());
            }
        }
    }
    for v in [&mut m.extensions, &mut m.deleted] {
        v.sort();
        v.dedup();
    }
    m
}

/// 目录下的普通文件名（排序）；目录不在 → 空。不递归。
pub fn plain_files(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_type().is_ok_and(|t| t.is_file()))
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .collect();
    v.sort();
    v
}

/// 上次开机的最后几行 journal（`journalctl -b -1 -n 20 --no-pager`）：设备冻死/意外重启后，这是设备自己能拿到的
/// 最直接线索。journal 没有持久化（只在 `/run`）时 journalctl 报错或给 `-- No entries --`，一律返回 `None`，前端不显示。
/// （宿主机上的"飞行记录仪"日志不在设备上，网关读不到，所以不看它。）
pub fn prev_boot_journal() -> Option<Vec<String>> {
    parse_journal(&crate::manage::run_timeout("journalctl", &["-b", "-1", "-n", PREV_BOOT_LINES, "--no-pager"], JOURNAL_TIMEOUT).ok()?)
}

pub fn parse_journal(out: &str) -> Option<Vec<String>> {
    let lines: Vec<String> = out.lines().filter(|l| !l.trim().is_empty() && !l.starts_with("-- ")).map(str::to_string).collect();
    (!lines.is_empty()).then_some(lines)
}

/// `path` 所在文件系统的 (可用字节, 总字节)。与 book-serve `free_bytes_of` 同一写法（statvfs，不 fork `df`）。
pub fn fs_space(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut st = std::mem::MaybeUninit::<libc::statvfs>::zeroed();
    // SAFETY: `c` 是合法的 NUL 结尾 C 字符串；`st` 是足够大的零初始化 statvfs，成功返回后由内核填好。
    if unsafe { libc::statvfs(c.as_ptr(), st.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: statvfs 返回 0，结构体已被内核写入。
    let st = unsafe { st.assume_init() };
    #[allow(clippy::unnecessary_cast)] // 各架构 libc 的字段类型不同（有的 u32 有的 u64）
    let (bavail, blocks, frsize) = (st.f_bavail as u64, st.f_blocks as u64, st.f_frsize as u64);
    Some((bavail.checked_mul(frsize)?, blocks.checked_mul(frsize)?))
}

/// `/proc/uptime` 第一列（秒）。
pub fn uptime_secs(proc_root: &Path) -> Option<u64> {
    std::fs::read_to_string(proc_root.join("uptime")).ok()?.split_whitespace().next()?.split('.').next()?.parse().ok()
}

/// 待换入区（与 `packaging/devlib.sh` 的 `CJ_SO_PENDING_DIR` 缺省值同一路径）。
pub fn so_pending_dir(paths: &Paths) -> std::path::PathBuf {
    paths.home().join(".cangjie-stage/so-pending")
}

/// 采集一次。`show` 是 `systemctl show` 的原始输出（失败时传 `Err`，其余照常采集）；`prev_boot` 是上次开机的 journal 尾部。
pub fn collect(paths: &Paths, proc_root: &Path, show: Result<String, String>, prev_boot: Option<Vec<String>>) -> serde_json::Value {
    let units = units();
    let (table, show_err) = match show {
        Ok(t) => (parse_show(&t), None),
        Err(e) => (HashMap::new(), Some(e)),
    };
    let list: Vec<UnitHealth> = units.iter().map(|u| unit_health(proc_root, u, table.get(u))).collect();
    let xochitl_pid = list.first().and_then(|u| u.pid);
    let maps = xochitl_maps(proc_root, xochitl_pid);
    let space = fs_space(paths.home());
    serde_json::json!({
        "uptimeSecs": uptime_secs(proc_root),
        "systemctlError": show_err,
        "units": list,
        "xochitl": maps,
        "soPending": plain_files(&so_pending_dir(paths)),
        "prevBoot": prev_boot,
        "home": {"freeBytes": space.map(|s| s.0), "totalBytes": space.map(|s| s.1)},
        "firmware": super::ota::firmware_json(),
        "at": rmsvc_core::clock::now_secs(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOW: &str = "Id=xochitl.service\nLoadState=loaded\nActiveState=active\nSubState=running\nNRestarts=2\nMainPID=613\nInactiveExitTimestampMonotonic=7400000\nActiveEnterTimestampMonotonic=7401500\n\n\
Id=xovi-reenable.service\nLoadState=loaded\nActiveState=active\nSubState=exited\nNRestarts=0\nMainPID=0\nInactiveExitTimestampMonotonic=5460000\nActiveEnterTimestampMonotonic=7490000\n\n\
Id=book-serve.service\nLoadState=not-found\nActiveState=inactive\nSubState=dead\nNRestarts=0\nMainPID=0\nInactiveExitTimestampMonotonic=0\nActiveEnterTimestampMonotonic=0\n";

    #[test]
    fn parses_multi_unit_show_and_computes_start_times() {
        let t = parse_show(SHOW);
        assert_eq!(t.len(), 3);
        let r = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(r.path().join("613")).unwrap();
        std::fs::write(r.path().join("613/status"), "Name:\txochitl\nVmHWM:\t  250000 kB\nVmRSS:\t  180000 kB\n").unwrap();
        let x = unit_health(r.path(), "xochitl.service", t.get("xochitl.service"));
        assert_eq!((x.pid, x.n_restarts, x.rss_kb, x.hwm_kb), (Some(613), Some(2), Some(180000), Some(250000)));
        assert_eq!((x.started_at_ms, x.start_ms), (Some(7401), Some(1)));
        let o = unit_health(r.path(), "xovi-reenable.service", t.get("xovi-reenable.service"));
        assert_eq!((o.pid, o.start_ms, o.sub.as_str()), (None, Some(2030), "exited"), "oneshot：MainPID=0 不读内存，耗时=整个 ExecStart");
        let b = unit_health(r.path(), "book-serve.service", t.get("book-serve.service"));
        assert_eq!((b.load.as_str(), b.started_at_ms, b.start_ms), ("not-found", None, None));
        // systemctl 整个失败：每个单元只有名字
        assert_eq!(unit_health(r.path(), "gateway.service", None), UnitHealth { unit: "gateway.service".into(), ..Default::default() });
    }

    #[test]
    fn maps_detects_xovi_extensions_and_deleted() {
        let m = parse_maps(
            "7f00-7f01 r-xp 00000000 b3:04 11 /home/root/xovi/xovi.so\n\
7f02-7f03 r-xp 00000000 b3:04 12 /home/root/xovi/extensions.d/hl-snap.so (deleted)\n\
7f04-7f05 r--p 00001000 b3:04 12 /home/root/xovi/extensions.d/hl-snap.so (deleted)\n\
7f06-7f07 r-xp 00000000 b3:04 13 /home/root/xovi/extensions.d/hw-stroke.so\n\
7f08-7f09 rw-p 00000000 00:00 0 \n\
7f0a-7f0b r-xp 00000000 b3:04 14 /usr/lib/lib with space.so\n",
        );
        assert!(m.readable && m.xovi);
        assert_eq!(m.extensions, ["hl-snap.so", "hw-stroke.so"]);
        assert_eq!(m.deleted, ["hl-snap.so"]);
        assert_eq!(parse_maps("7f00-7f01 r-xp 0 00:00 1 /usr/bin/xochitl\n"), XochitlMaps { readable: true, ..Default::default() });
        assert!(!xochitl_maps(Path::new("/nonexistent-proc"), Some(1)).readable);
    }

    #[test]
    fn journal_output_without_entries_is_none() {
        assert_eq!(parse_journal("-- No entries --\n"), None);
        assert_eq!(parse_journal(""), None);
        assert_eq!(parse_journal("Sep 24 03:01:00 imx93 kernel: a\n\nSep 24 03:01:01 imx93 xochitl[613]: b\n"), Some(vec!["Sep 24 03:01:00 imx93 kernel: a".to_string(), "Sep 24 03:01:01 imx93 xochitl[613]: b".to_string()]));
    }

    #[test]
    fn collect_tolerates_missing_everything() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| match k {
            "HOME" => Some(h.clone()),
            "XDG_STATE_HOME" | "XDG_DATA_HOME" | "XDG_CONFIG_HOME" | "XDG_RUNTIME_DIR" => Some(format!("{h}/{k}")),
            _ => None,
        });
        std::fs::create_dir_all(so_pending_dir(&paths)).unwrap();
        std::fs::write(so_pending_dir(&paths).join("hl-snap.so"), b"x").unwrap();
        let v = collect(&paths, &t.path().join("proc"), Err("no systemd".into()), Some(vec!["a".into(), "b".into()]));
        assert_eq!(v["systemctlError"], "no systemd");
        assert_eq!(v["units"].as_array().unwrap().len(), units().len());
        assert_eq!(v["soPending"], serde_json::json!(["hl-snap.so"]));
        assert_eq!(v["prevBoot"], serde_json::json!(["a", "b"]));
        assert!(collect(&paths, &t.path().join("proc"), Err(String::new()), None)["prevBoot"].is_null(), "journal 没持久化：不显示");
        assert!(v["home"]["freeBytes"].as_u64().is_some(), "临时目录所在分区应可查");
        assert!(v["uptimeSecs"].is_null() && v["xochitl"]["readable"] == false);
    }
}
