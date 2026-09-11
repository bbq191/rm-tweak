//! 电池刺客（battop）状态探测 + 开关。battop 是独立于 shelf 安装体系之外的诊断采样器
//! （`enhance/battop/`，固定装在 `/home/root/battop`，不走 shelf 的 XDG/`bin_dir` 那套），
//! 原生「设置」App 只有一个只读展示页（读 `summary.json`），**从没有过开关**——这里是它第一次有开关。
//!
//! 2026-08-29 出过 cgroup/RCU 死锁事故（`enhance/battop/FINDINGS.md`），根因是内核罕见的 RCU
//! stall（没有被修复，是概率事件，不是 battop 逻辑 bug）——当时 `battop.timer` 每 ~10 分钟重启一次
//! oneshot，把"进程启动时 systemd 做 cgroup 迁移"这个动作干到 144 次/天，144 倍放大了撞上那个
//! 罕见窗口的概率。已经修复为常驻 `Type=simple`（进程内 `loop{sample;sleep}`，开机只迁一次
//! cgroup）。**这不是把风险归零，是把触发频率从"每天必然 144 次"降到"用户手点几次"**：每一次
//! `systemctl start` 依然是同一类 cgroup 迁移操作，只是正常使用（偶尔开关）频率低到可以忽略——
//! 短时间内在网页上反复连点启停，理论上就是在人为复现旧 timer 那种高频重复触发的条件，这里没做
//! 任何防连点/限流，2026-09-10 用户问起后把这条边界条件补进注释和网页文案，不再只说"不会重现"。
use std::path::Path;
use std::time::UNIX_EPOCH;

const UNIT_FILE: &str = "/usr/lib/systemd/system/battop.service";
const BIN: &str = "/home/root/battop/battop";
const SUMMARY: &str = "/home/root/battop/data/summary.json";

pub struct Status {
    pub installed: bool,
    pub running: bool,
    pub last_sample_at: Option<u64>,
}

pub fn status() -> Status {
    let installed = Path::new(UNIT_FILE).is_file() && Path::new(BIN).is_file();
    let running = installed && crate::manage::run("systemctl", &["is-active", "battop.service"]).map(|s| s.trim() == "active").unwrap_or(false);
    let last_sample_at = std::fs::metadata(SUMMARY).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());
    Status { installed, running, last_sample_at }
}

/// `summary.json`（battop 自己写的预聚合：4 个时间窗 × 应用/进程/唤醒源 top15，见
/// `enhance/battop/src/main.rs::write_summary` 头注）原样透传——网页这边不重新聚合一遍，battop
/// 自己已经算好了要展示的数据，这里只是把文件内容读出来当 JSON 转发。文件不存在（刚装还没首次
/// 采样，或者从没装过）返回 `None`，调用方决定怎么提示。
pub fn summary() -> Option<serde_json::Value> {
    std::fs::read_to_string(SUMMARY).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// `action`：`start`/`stop`。未装（unit 文件不在）时拒绝——网页这次不做"从零装 battop"。
pub fn toggle(action: &str) -> Result<(), String> {
    if !Path::new(UNIT_FILE).is_file() {
        return Err("设备上没有装 battop（这次网页开关只控制已经手动装好的 battop，不提供从网页安装）".into());
    }
    match action {
        "start" | "stop" => crate::manage::run("systemctl", &[action, "battop.service"]).map(|_| ()),
        _ => Err("action 只能 start|stop".into()),
    }
}
