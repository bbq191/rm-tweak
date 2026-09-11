// battop 采集器(常驻服务,进程内每 ~10 分钟采一次;旧模型是 timer 反复拉起 oneshot,
// 反复 service-start 的 cgroup 迁移撞内核 RCU stall 冻死整机 → 改常驻,cgroup 只迁一次)。
// 间隔可用 BATTOP_INTERVAL_SECS 覆盖(缺省 600)。
//
// 每轮采样:载入上次 baseline → 采样 /proc 各进程 CPU 累计 + 电量 →
// 算「距上次的增量」→ 按 comm(进程)与 systemd unit(应用)双聚合 →
// 追加到当日样本文件 → 写新 baseline → 清理 40 天前旧样本文件。
//
// 非实时、不常驻、不持 wakelock、不 WakeSystem —— 本身几乎不耗电。
// 数据目录:$BATTOP_DIR,缺省 /home/root/battop/data。
// 设计见 ../APP-DESIGN.md。纯 std 零依赖。

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CLK_TCK: u64 = 100; // 真机 getconf CLK_TCK = 100(aarch64 Linux 常量)
const RETAIN_DAYS: u64 = 40;
const BAT: &str = "/sys/class/power_supply/max77818_battery";

fn main() {
    let dir = std::env::var("BATTOP_DIR").unwrap_or_else(|_| "/home/root/battop/data".into());
    let dir = PathBuf::from(dir);
    if let Err(e) = fs::create_dir_all(&dir) {
        eprintln!("battop: 建目录失败 {}: {e}", dir.display());
        std::process::exit(1);
    }

    // 常驻采样：内核只在开机把本进程迁进 cgroup 一次，不再每 ~10min 由 timer 反复拉起 oneshot——
    // 反复 service-start 的 cgroup 迁移（systemd 把子进程 PID 写进 service cgroup.procs）曾撞上内核
    // RCU stall（synchronize_rcu 卡住、向另一 CPU 发 NMI），把握着 cgroup_threadgroup_rwsem 的启动
    // 进程连同 systemd(PID1) 一起拖死 → 整机冻死（2026-08-29 真机事故，见 FINDINGS）。改常驻后 cgroup
    // 迁移一次性，暴露窗口从 144次/天降到开机 1 次。
    // 间隔用 thread::sleep（CLOCK_MONOTONIC）：设备休眠时不推进 → 天然“醒时每 N 分钟”，与旧 timer 的
    // awake-only 语义一致；休眠中进程随之冻结、不持 wakelock、不阻止休眠。
    let interval = std::env::var("BATTOP_INTERVAL_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(600);
    loop {
        sample_once(&dir);
        std::thread::sleep(Duration::from_secs(interval));
    }
}

/// 一轮采样（原 main 主体）：读电量+进程 → 算增量 → 追加样本 → 写 baseline/summary → 清理旧样本。
fn sample_once(dir: &Path) {
    let dir = dir.to_path_buf(); // 影子绑定：下方 `&dir` / `dir.join` 沿用原逻辑、无需逐处改
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let (cap, status, charge, current) = read_battery();
    let disc = if status == "Discharging" { 1 } else { 0 };
    let awake = read_uptime();

    // 采样当前所有进程
    let procs = sample_procs();

    // 载入 baseline(上次每 (pid,starttime) 的累计 ticks + 是否有历史)
    let (have_base, base) = load_baseline(&dir);

    // 算增量并双聚合
    let mut by_comm: HashMap<String, u64> = HashMap::new();
    let mut by_unit: HashMap<String, u64> = HashMap::new();
    if have_base {
        for p in &procs {
            let prev = base.get(&(p.pid, p.starttime)).copied();
            // 新进程(不在 baseline)= 距上次全部是它攒的 → 增量即当前累计
            let delta = match prev {
                Some(pv) => p.ticks.saturating_sub(pv),
                None => p.ticks,
            };
            if delta == 0 {
                continue;
            }
            *by_comm.entry(p.comm.clone()).or_insert(0) += delta;
            *by_unit.entry(p.unit.clone()).or_insert(0) += delta;
        }
    }

    // 追加样本行:epoch \t kind \t key \t cpu_ms \t disc
    let file = dir.join(format!("samples-{}.tsv", ymd(now)));
    let mut out = String::new();
    let ms = |ticks: u64| ticks * 1000 / CLK_TCK;
    if have_base {
        for (k, t) in &by_comm {
            out.push_str(&format!("{now}\tproc\t{}\t{}\t{disc}\n", san(k), ms(*t)));
        }
        for (k, t) in &by_unit {
            out.push_str(&format!("{now}\tapp\t{}\t{}\t{disc}\n", san(k), ms(*t)));
        }
    }
    // _sys 行:epoch _sys status awake_s cap disc charge_uah current_ua(后两列 v3 新增,旧行无)
    out.push_str(&format!(
        "{now}\t_sys\t{}\t{}\t{}\t{disc}\t{charge}\t{current}\n",
        san(&status), awake, cap
    ));
    if let Err(e) = append(&file, &out) {
        eprintln!("battop: 写样本失败: {e}");
    }

    // 写新 baseline
    if let Err(e) = write_baseline(&dir, now, cap, &procs) {
        eprintln!("battop: 写 baseline 失败: {e}");
    }

    // v2:唤醒源事件(设备/子系统级)。journalctl 读 31 天内核日志较贵(~1s),故走小时级缓存:
    // wakes.tsv 超 50 分钟才重刷,summary 每轮只读这个小缓存 → battop 大多数运行仍 ~30ms。
    let cache = dir.join("wakes.tsv");
    let stale = fs::metadata(&cache)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| now.saturating_sub(d.as_secs()) > 3000)
        .unwrap_or(true);
    if stale {
        refresh_wake_cache(&dir, now.saturating_sub(31 * 86400));
    }
    let wakes = load_wake_cache(&dir);

    // 生成面板用预聚合(4 窗口 × 应用/进程 CPU + 唤醒源计数 + 放电%)
    if let Err(e) = write_summary(&dir, now, &wakes) {
        eprintln!("battop: 写 summary 失败: {e}");
    }

    // 清理 40 天前旧样本
    prune(&dir, now);

    // 简报(observe:被 systemctl 收进 journal)
    eprintln!(
        "battop: t={now} cap={cap} {status} procs={} 进程键={} 应用键={} {}",
        procs.len(),
        by_comm.len(),
        by_unit.len(),
        if have_base { "" } else { "(首次,仅建 baseline)" }
    );
}

struct Proc {
    pid: u64,
    starttime: u64,
    ticks: u64,
    comm: String,
    unit: String,
}

fn sample_procs() -> Vec<Proc> {
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
        // comm 在首个 '(' 与末个 ')' 之间(可含空格/括号)
        let lp = match stat.find('(') {
            Some(i) => i,
            None => continue,
        };
        let rp = match stat.rfind(')') {
            Some(i) => i,
            None => continue,
        };
        if rp < lp {
            continue;
        }
        let comm = stat[lp + 1..rp].to_string();
        // ')' 之后是 field3(state) 起的空格分隔字段
        let rest: Vec<&str> = stat[rp + 1..].split_whitespace().collect();
        // field14 utime=idx11, field15 stime=idx12, field22 starttime=idx19
        if rest.len() < 20 {
            continue;
        }
        let utime: u64 = rest[11].parse().unwrap_or(0);
        let stime: u64 = rest[12].parse().unwrap_or(0);
        let starttime: u64 = rest[19].parse().unwrap_or(0);
        let unit = read_unit(pid);
        v.push(Proc { pid, starttime, ticks: utime + stime, comm, unit });
    }
    v
}

// 从 cgroup v2 取归属 unit;找不到 .service/.scope 则归 kernel/other
fn read_unit(pid: u64) -> String {
    let cg = match fs::read_to_string(format!("/proc/{pid}/cgroup")) {
        Ok(s) => s,
        Err(_) => return "kernel".into(),
    };
    // 形如 "0::/system.slice/xochitl.service"
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

// 返回 (容量%, 状态, charge_now µAh 库仑计当前电量, current_now µA 瞬时电流)
fn read_battery() -> (i64, String, i64, i64) {
    let read_i = |k: &str| -> i64 {
        fs::read_to_string(format!("{BAT}/{k}"))
            .ok()
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(-1)
    };
    let cap = read_i("capacity");
    let charge = read_i("charge_now"); // µAh,下降量=精确放电 mAh(v3)
    let current = read_i("current_now"); // µA 瞬时
    let status = fs::read_to_string(format!("{BAT}/status"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "Unknown".into());
    (cap, status, charge, current)
}

fn read_uptime() -> u64 {
    fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split('.').next().and_then(|x| x.trim().parse().ok()))
        .unwrap_or(0)
}

fn load_baseline(dir: &Path) -> (bool, HashMap<(u64, u64), u64>) {
    let mut m = HashMap::new();
    let content = match fs::read_to_string(dir.join("baseline.tsv")) {
        Ok(c) => c,
        Err(_) => return (false, m),
    };
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
    (true, m)
}

fn write_baseline(dir: &Path, now: u64, cap: i64, procs: &[Proc]) -> std::io::Result<()> {
    let tmp = dir.join("baseline.tsv.tmp");
    let mut s = format!("_ts\t{now}\t_cap\t{cap}\n");
    for p in procs {
        // pid \t starttime \t ticks \t comm \t unit
        s.push_str(&format!(
            "{}\t{}\t{}\t{}\t{}\n",
            p.pid, p.starttime, p.ticks, san(&p.comm), san(&p.unit)
        ));
    }
    fs::write(&tmp, s)?;
    fs::rename(&tmp, dir.join("baseline.tsv"))
}

fn append(path: &Path, data: &str) -> std::io::Result<()> {
    let mut f = fs::OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(data.as_bytes())
}

fn prune(dir: &Path, now: u64) {
    let cutoff = now.saturating_sub(RETAIN_DAYS * 86400);
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for e in entries.flatten() {
        let n = e.file_name();
        let n = n.to_string_lossy();
        if !n.starts_with("samples-") || !n.ends_with(".tsv") {
            continue;
        }
        if let Ok(md) = e.metadata() {
            if let Ok(m) = md.modified() {
                if let Ok(d) = m.duration_since(UNIX_EPOCH) {
                    if d.as_secs() < cutoff {
                        let _ = fs::remove_file(e.path());
                    }
                }
            }
        }
    }
}

// ── summary.json:面板用预聚合 ────────────────────────────────
// 读全部样本 → 4 窗口(当日/7天/30天/全部)× 应用(友好名)/进程(comm)双分组
// + 每窗口放电% → 写 summary.json。窗口计算靠行内 epoch。
fn write_summary(dir: &Path, now: u64, wakes: &[(u64, String)]) -> std::io::Result<()> {
    // usage:(epoch, is_app, key, ms)   sys:(epoch, cap, disc)
    let mut usage: Vec<(u64, bool, String, u64)> = Vec::new();
    let mut sys: Vec<(u64, i64, i64)> = Vec::new(); // (epoch, cap%, charge_uah)
    for e in fs::read_dir(dir)?.flatten() {
        let n = e.file_name();
        let n = n.to_string_lossy();
        if !n.starts_with("samples-") || !n.ends_with(".tsv") {
            continue;
        }
        let content = fs::read_to_string(e.path()).unwrap_or_default();
        for line in content.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() < 3 {
                continue;
            }
            let epoch: u64 = match f[0].parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            match f[1] {
                "proc" | "app" if f.len() >= 4 => {
                    let ms: u64 = f[3].parse().unwrap_or(0);
                    usage.push((epoch, f[1] == "app", f[2].to_string(), ms));
                }
                "_sys" if f.len() >= 5 => {
                    let cap: i64 = f[4].parse().unwrap_or(-1);
                    let charge: i64 = f.get(6).and_then(|x| x.parse().ok()).unwrap_or(-1);
                    sys.push((epoch, cap, charge));
                }
                _ => {}
            }
        }
    }

    let off = local_offset();
    let local = now as i64 + off;
    let midnight = (local - local.rem_euclid(86400) - off) as u64; // 本地今日 0 点(UTC epoch)
    let windows: [(&str, u64); 4] = [
        ("today", midnight),
        ("7d", now.saturating_sub(7 * 86400)),
        ("30d", now.saturating_sub(30 * 86400)),
        ("all", 0),
    ];

    let mut json = format!("{{\"generated\":{now},\"windows\":{{");
    for (wi, (wname, start)) in windows.iter().enumerate() {
        // 应用(按友好名聚合)/ 进程(按 comm)
        let mut app: HashMap<String, u64> = HashMap::new();
        let mut proc: HashMap<String, u64> = HashMap::new();
        for (ep, is_app, key, ms) in &usage {
            if ep < start {
                continue;
            }
            if *is_app {
                *app.entry(friendly(key)).or_insert(0) += ms;
            } else {
                *proc.entry(key.clone()).or_insert(0) += ms;
            }
        }
        // 放电:窗口内 _sys 相邻样本,容量% 下降之和(disc)+ 库仑计 µAh 下降之和(精确 mAh,v3)
        let mut s: Vec<(u64, i64, i64)> = sys.iter().filter(|(e, _, _)| e >= start).cloned().collect();
        s.sort_by_key(|t| t.0);
        let mut disc = 0i64;
        let mut drained_uah = 0i64;
        let mut disc_secs = 0u64;
        for w in s.windows(2) {
            let (ca, cb) = (w[0].1, w[1].1);
            if ca >= 0 && cb >= 0 && ca > cb {
                disc += ca - cb;
            }
            let (qa, qb) = (w[0].2, w[1].2);
            if qa >= 0 && qb >= 0 && qa > qb {
                drained_uah += qa - qb;
                disc_secs += w[1].0.saturating_sub(w[0].0);
            }
        }
        let samples = s.len();
        let mah = drained_uah / 1000; // 精确放电 mAh
        let ma = if disc_secs > 0 { drained_uah * 3600 / disc_secs as i64 / 1000 } else { 0 }; // 均放电 mA

        // 唤醒源计数(窗口内 journal 事件按友好名聚合)
        let mut wake: HashMap<String, u64> = HashMap::new();
        for (ep, name) in wakes {
            if ep < start {
                continue;
            }
            *wake.entry(friendly_wake(name)).or_insert(0) += 1;
        }

        if wi > 0 {
            json.push(',');
        }
        json.push_str(&format!(
            "\"{wname}\":{{\"discharge\":{disc},\"mah\":{mah},\"ma\":{ma},\"samples\":{samples},"
        ));
        json.push_str(&format!("\"app\":{},", top_json(&app)));
        json.push_str(&format!("\"proc\":{},", top_json(&proc)));
        json.push_str(&format!("\"wake\":{}}}", top_json(&wake)));
    }
    json.push_str("}}\n");

    let tmp = dir.join("summary.json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, dir.join("summary.json"))
}

// 取前 15 名(按 ms 降序)→ JSON 数组;pct=占窗口总量百分比
fn top_json(m: &HashMap<String, u64>) -> String {
    let total: u64 = m.values().sum();
    let mut v: Vec<(&String, &u64)> = m.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1));
    v.truncate(15);
    let mut s = String::from("[");
    for (i, (k, ms)) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let pct = if total > 0 { **ms * 100 / total } else { 0 };
        s.push_str(&format!(
            "{{\"name\":\"{}\",\"ms\":{},\"pct\":{}}}",
            json_esc(k),
            ms,
            pct
        ));
    }
    s.push(']');
    s
}

// systemd unit / comm → 友好应用名(应用视图用;设计 §5)
fn friendly(unit: &str) -> String {
    let n = match unit {
        "xochitl" => "reMarkable 核心",
        "memfaultd" | "crashuploader" => "Memfault 遥测",
        "remarkable-counter-metrics" | "slumber-metrics" | "nm-metrics"
        | "battery-status-metrics" => "reMarkable 度量",
        "mdm-agent" => "reMarkable MDM",
        "rm-sync" | "update-engine" | "swupdate" => "reMarkable 同步/更新",
        "NetworkManager" | "wpa_supplicant" | "systemd-networkd" | "systemd-resolved" => "网络栈",
        "marker-manager" | "tee-supplicant" => "硬件",
        "wr-serve" | "cj-stars" | "wr-renew" | "battop" | "cangjie-wallpaper" => "cang-jie",
        "kernel" => "内核",
        _ if unit.ends_with("-metrics") => "reMarkable 度量",
        _ => unit,
    };
    n.to_string()
}

// v2:读 journal 内核唤醒源事件(自 since 起)。格式:"<epoch>.<us> host kernel: PM: active wakeup source: <NAME>"
/// 有界执行子进程：读满 stdout 或超 `secs` 秒即 SIGKILL，返回 stdout 字节；spawn 失败/超时返回 None。
/// 纯 std：读线程排空管道（防子进程写满 pipe 阻塞成 D 态），主线程 recv_timeout 计时。常驻模型下
/// 一个卡住的 journalctl 会拖死整个采样循环，故所有外部子进程必须有界。
fn run_bounded(mut cmd: Command, secs: u64) -> Option<Vec<u8>> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut stdout = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut buf);
        let _ = tx.send(buf); // rx 可能已 drop（超时路径）→ 忽略
    });
    match rx.recv_timeout(Duration::from_secs(secs)) {
        Ok(buf) => {
            let _ = child.wait();
            Some(buf)
        }
        Err(_) => {
            let _ = child.kill(); // 超时 → 杀子进程，绝不让循环无限阻塞
            let _ = child.wait();
            None
        }
    }
}

fn read_wake_events(since: u64) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    // _TRANSPORT=kernel 跨 boot 取内核消息(不用 -k,那只当前 boot);--since 界定,输出有界(~1MB/31天)
    let mut cmd = Command::new("journalctl");
    cmd.args(["-o", "short-unix", "--no-pager", "--since"])
        .arg(format!("@{since}"))
        .arg("_TRANSPORT=kernel");
    // 有界执行：journald 卡住最多等 20s 就 kill，本轮跳过刷缓存（用旧 wakes.tsv），绝不卡死循环。
    let stdout = match run_bounded(cmd, 20) {
        Some(b) => b,
        None => return out,
    };
    let text = String::from_utf8_lossy(&stdout);
    for line in text.lines() {
        let idx = match line.find("active wakeup source: ") {
            Some(i) => i,
            None => continue,
        };
        let name = line[idx + "active wakeup source: ".len()..].trim();
        if name.is_empty() {
            continue;
        }
        // 行首是 epoch(可能带 .微秒)
        let epoch: u64 = line
            .split_whitespace()
            .next()
            .and_then(|t| t.split('.').next())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        if epoch == 0 {
            continue;
        }
        out.push((epoch, name.to_string()));
    }
    out
}

// 刷新唤醒缓存:journalctl 读一次 → 写 wakes.tsv(epoch \t 原始源名)
fn refresh_wake_cache(dir: &Path, since: u64) {
    let ev = read_wake_events(since);
    let mut s = String::new();
    for (ep, name) in &ev {
        s.push_str(&format!("{ep}\t{}\n", san(name)));
    }
    let tmp = dir.join("wakes.tsv.tmp");
    if fs::write(&tmp, s).is_ok() {
        let _ = fs::rename(&tmp, dir.join("wakes.tsv"));
    }
}

// 读唤醒缓存
fn load_wake_cache(dir: &Path) -> Vec<(u64, String)> {
    let mut out = Vec::new();
    let content = match fs::read_to_string(dir.join("wakes.tsv")) {
        Ok(c) => c,
        Err(_) => return out,
    };
    for line in content.lines() {
        let mut it = line.splitn(2, '\t');
        if let (Some(e), Some(n)) = (it.next(), it.next()) {
            if let Ok(ep) = e.parse::<u64>() {
                out.push((ep, n.to_string()));
            }
        }
    }
    out
}

// 唤醒源名 → 友好名
fn friendly_wake(name: &str) -> String {
    let n = match name {
        "mwlan" => "WiFi",
        "xochitl.batterymanager" => "电池管理",
        "sleep.resume" => "唤醒锁",
        "udev.charger" => "充电器",
        "gpio-hall-sensors" => "合盖磁吸",
        "rtc0" | "rtc" => "定时器(RTC)",
        _ if name.starts_with("spi") => "触控笔(SPI)",
        _ if name.ends_with("pwrkey") => "电源键",
        // I2C 设备名形如 "0-0048" / "1-0021"
        _ if name.len() >= 3
            && name.as_bytes()[0].is_ascii_digit()
            && name[1..].starts_with('-') =>
        {
            "传感器(I2C)"
        }
        _ => name,
    };
    n.to_string()
}

fn local_offset() -> i64 {
    // 用 date +%z 取本地时区偏移(设备 busybox date);失败回落 CST +8h
    if let Ok(o) = std::process::Command::new("date").arg("+%z").output() {
        let s = String::from_utf8_lossy(&o.stdout);
        let s = s.trim();
        if s.len() == 5 {
            if let (Ok(h), Ok(m)) = (s[1..3].parse::<i64>(), s[3..5].parse::<i64>()) {
                let sec = h * 3600 + m * 60;
                return if s.starts_with('-') { -sec } else { sec };
            }
        }
    }
    8 * 3600
}

fn json_esc(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' | '\t' | '\r' => o.push(' '),
            _ => o.push(c),
        }
    }
    o
}

// TSV 安全:去掉 tab/换行
fn san(s: &str) -> String {
    s.chars().map(|c| if c == '\t' || c == '\n' { ' ' } else { c }).collect()
}

// epoch(秒,UTC)→ YYYYMMDD(仅用于文件按天滚动;窗口计算靠行内 epoch)
fn ymd(secs: u64) -> String {
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
