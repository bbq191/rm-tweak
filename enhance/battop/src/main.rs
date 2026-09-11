// battop 采集器(常驻服务,进程内每 ~10 分钟采一次;旧模型是 timer 反复拉起 oneshot,
// 反复 service-start 的 cgroup 迁移撞内核 RCU stall 冻死整机 → 改常驻,cgroup 只迁一次)。
// 2026-09-23 真机复现过一次类似冻机(常驻模型下,冻结时间与 wake 模块当时仅存的子进程创建——fork
// journalctl——精确重合到秒;不是同一条 cgroup_procs_write 路径,但进程创建仍是这条循环里唯一残留的
// "非纯内存操作",故 wake 模块改直读 /dev/kmsg,彻底消灭这个循环里的最后一次 fork,见 wake.rs 头注/FINDINGS)。
// 间隔可用 BATTOP_INTERVAL_SECS 覆盖(缺省 600)。
//
// 每轮采样:载入上次 baseline → 采样 /proc 各进程 CPU 累计 + 电量 →
// 算「距上次的增量」→ 按 comm(进程)与 systemd unit(应用)双聚合 →
// 追加到当日样本文件 → 写新 baseline → 清理 40 天前旧样本文件。
//
// 非实时、不持 wakelock、不 WakeSystem —— 本身几乎不耗电。
// 数据目录:$BATTOP_DIR,缺省 /home/root/battop/data。
// 设计见 history/APP-DESIGN.md。纯 std 零依赖。
//
// 模块分工：procs（/proc + sysfs 原始输入）· store（baseline/样本落盘与增量聚合）·
// summary（面板用 summary.json 流式聚合）· wake（/dev/kmsg 唤醒源缓存，纯读不 fork 子进程）· util。
mod procs;
mod store;
mod summary;
mod util;
mod wake;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use util::{local_offset, now_secs, san};

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
    // summary 的按文件聚合缓存跨轮保留（见 summary.rs 头注「按文件缓存」）。
    let mut cache = summary::SummaryCache::default();
    loop {
        sample_once(&dir, &mut cache);
        std::thread::sleep(Duration::from_secs(interval));
    }
}

/// 一轮采样：读电量+进程 → 算增量 → 追加样本 → 写 baseline/summary → 清理旧样本。
fn sample_once(dir: &Path, cache: &mut summary::SummaryCache) {
    let now = now_secs();
    let bat = procs::read_battery();
    let disc = if bat.status == "Discharging" { 1 } else { 0 };
    let awake = procs::read_uptime();

    // 采样当前所有进程
    let snapshot = procs::sample_procs();

    // 载入 baseline(上次每 (pid,starttime) 的累计 ticks + 是否有历史)，算增量并双聚合
    let (have_base, base) = store::load_baseline(dir);
    let (by_comm, by_unit) = if have_base { store::aggregate(&snapshot, &base) } else { Default::default() };

    // 追加样本行 + _sys 行:epoch _sys status awake_s cap disc charge_uah current_ua(后两列 v3 新增,旧行无)
    let mut out = store::sample_rows(now, disc, &by_comm, &by_unit);
    out.push_str(&format!(
        "{now}\t_sys\t{}\t{}\t{}\t{disc}\t{}\t{}\n",
        san(&bat.status),
        awake,
        bat.cap,
        bat.charge,
        bat.current
    ));
    if let Err(e) = store::append(&store::sample_file(dir, now), &out) {
        eprintln!("battop: 写样本失败: {e}");
    }

    // 写新 baseline
    if let Err(e) = store::write_baseline(dir, now, bat.cap, &snapshot) {
        eprintln!("battop: 写 baseline 失败: {e}");
    }

    // 唤醒源事件(设备/子系统级)：小时级缓存 + 增量读 journal；summary 每轮只读这个小缓存。
    wake::refresh_if_stale(dir, now);
    let wakes = wake::load_cache(dir);

    // 生成面板用预聚合(4 窗口 × 应用/进程 CPU + 唤醒源计数 + 放电%)
    if let Err(e) = summary::write_summary(dir, now, local_offset(now), &wakes, cache) {
        eprintln!("battop: 写 summary 失败: {e}");
    }

    // 清理 40 天前旧样本
    store::prune(dir, now);

    // 简报(observe:被 systemctl 收进 journal)
    eprintln!(
        "battop: t={now} cap={} {} procs={} 进程键={} 应用键={} {}",
        bat.cap,
        bat.status,
        snapshot.len(),
        by_comm.len(),
        by_unit.len(),
        if have_base { "" } else { "(首次,仅建 baseline)" }
    );
}
