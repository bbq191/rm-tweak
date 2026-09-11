//! 轮换触发器：xochitl 休眠时读完 `current.png`，就轮换到下一张（下次休眠显示它）。
//!
//! **怎么知道"休眠了"**：xochitl 每次进 DeepSleep 都按 `SleepScreenPath` 重读一遍 `current.png`——2026-09-24 真机用
//! inotify 观察坐实：`OPEN`→`CLOSE_NOWRITE current.png` 恰在它写 `Changing display state from Normal to DeepSleep`
//! 前约 120 ms，空闲 20 分钟里没有别的读。所以只监听壁纸目录的 `IN_CLOSE_NOWRITE`：
//! - 空闲时**零唤醒**，每次休眠只醒一次；不再常驻 `journalctl -f` 子进程（它随全系统每条 journal 醒，
//!   本进程又随 xochitl 每行日志醒——09-24 前的做法，见文末）。
//! - 本服务自己写 `current.png`（原地 truncate 写）产生的是 `IN_CLOSE_WRITE`，不在掩码里，不会自己触发自己；
//!   池图在 `pool/` 子目录，目录监听不递归，网页预览读池图也不触发。
//! - xochitl 已经读完才换图，不和它画休眠屏抢时序（09-03 选在唤醒时轮换也是为了避开这个）。换图若恰好被随后的
//!   内核挂起冻在半路，唤醒后接着写完，下次休眠前早已写好。
//! - 充电/连 USB 时按电源键只切显示状态、内核不挂起（09-03 真机发现，systemd-sleep 钩子因此不可用）——xochitl
//!   照样走 Normal→DeepSleep 画休眠屏、读这个文件，所以同样触发（未单独在充电状态下复核）。
//! - 同一次休眠若被读了不止一次，[`MIN_ROLL_GAP`] 内只轮换一次。
//!
//! 09-24 前：跟 `journalctl -f -u xochitl` 找 `DeepSleep to Normal`（唤醒时轮换）。xochitl 日志走 stdout、
//! journal 里没有可过滤的字段，设备 journalctl 也不支持 `-g`，只能整条跟着。
use crate::store::WallpaperStore;
use inotify::{EventMask, Inotify, WatchMask};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// 两次轮换的最短间隔：同一次休眠里 xochitl 若重复读，只算一次。按**含休眠**的开机时长（`/proc/uptime`，
/// CLOCK_BOOTTIME）计：09-25 前用 `Instant`（CLOCK_MONOTONIC，休眠时不走）——休眠几小时后唤醒、10 秒内又
/// 按电源键休眠，单调时钟只走了几秒，这次读图被当成"同一次休眠的重复读"跳过，下次休眠又显示同一张。
const MIN_ROLL_GAP: Duration = Duration::from_secs(10);
/// 建监听失败（目录还没建好等）时的重试间隔上下限。
const RETRY_MIN: Duration = Duration::from_secs(5);
const RETRY_MAX: Duration = Duration::from_secs(300);

/// 这条事件是不是"有人读完了休眠屏图片"。
pub fn is_sleep_read(mask: EventMask, name: Option<&std::ffi::OsStr>, current_name: &std::ffi::OsStr) -> bool {
    mask.contains(EventMask::CLOSE_NOWRITE) && !mask.contains(EventMask::ISDIR) && name == Some(current_name)
}

/// 距上次轮换够不够久（`last=None` 表示还没轮换过）。时刻为含休眠的开机秒数（见 [`MIN_ROLL_GAP`]）。
pub fn gap_ok(last: Option<f64>, now: f64) -> bool {
    last.is_none_or(|t| now - t >= MIN_ROLL_GAP.as_secs_f64())
}

/// `/proc/uptime` 第一列（含休眠）；读不到（非 Linux 测试环境等）退回进程内单调时钟——只影响去重判定。
fn boottime_secs() -> f64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next().and_then(|x| x.parse::<f64>().ok()))
        .unwrap_or_else(|| START.get_or_init(Instant::now).elapsed().as_secs_f64())
}

/// 监听到出错为止（正常情况下永不返回）。
fn watch_once(store: &WallpaperStore, bus: &rmsvc_core::events::EventBus) -> Result<(), String> {
    let cur = store.current_path();
    let dir = cur.parent().ok_or("current.png 没有父目录")?;
    let current_name = cur.file_name().ok_or("current.png 没有文件名")?.to_os_string();
    let mut ino = Inotify::init().map_err(|e| format!("inotify 初始化失败: {e}"))?;
    ino.watches().add(dir, WatchMask::CLOSE_NOWRITE).map_err(|e| format!("监听 {} 失败: {e}", dir.display()))?;
    let mut buf = [0u8; 1024];
    let mut last: Option<f64> = None;
    loop {
        let events = ino.read_events_blocking(&mut buf).map_err(|e| format!("读 inotify 事件失败: {e}"))?;
        let hit = events.into_iter().any(|ev| is_sleep_read(ev.mask, ev.name, &current_name));
        if !hit {
            continue;
        }
        let now = boottime_secs();
        if !gap_ok(last, now) {
            continue;
        }
        last = Some(now);
        match store.roll() {
            Ok(Some(n)) => {
                println!("[wallpaper-serve] 休眠屏已读 → 轮换到 {n}");
                bus.publish("wallpapers", "pool");
            }
            Ok(None) => {}
            Err(e) => eprintln!("[wallpaper-serve] 轮换失败: {e}"),
        }
    }
}

pub fn spawn(store: Arc<WallpaperStore>, bus: Arc<rmsvc_core::events::EventBus>) {
    std::thread::spawn(move || {
        let mut wait = RETRY_MIN;
        loop {
            let _ = store.ensure(); // 目录不在就建上，免得监听一直失败
            let started = Instant::now();
            if let Err(e) = watch_once(&store, &bus) {
                // 监听正常跑过一阵才出错：是新的一次故障，退避从头算（此前退避只增不减）。
                if started.elapsed() >= RETRY_MAX {
                    wait = RETRY_MIN;
                }
                eprintln!("[wallpaper-serve] {e}，{}s 后重试", wait.as_secs());
            }
            std::thread::sleep(wait);
            wait = (wait * 2).min(RETRY_MAX);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn only_close_nowrite_of_current_png_counts() {
        let cur = OsStr::new("current.png");
        assert!(is_sleep_read(EventMask::CLOSE_NOWRITE, Some(cur), cur));
        assert!(!is_sleep_read(EventMask::CLOSE_WRITE, Some(cur), cur), "本服务自己写 current.png 不算");
        assert!(!is_sleep_read(EventMask::CLOSE_NOWRITE, Some(OsStr::new("pool")), cur));
        assert!(!is_sleep_read(EventMask::CLOSE_NOWRITE | EventMask::ISDIR, Some(cur), cur));
        assert!(!is_sleep_read(EventMask::CLOSE_NOWRITE, None, cur));
    }

    #[test]
    fn repeated_reads_within_gap_roll_once() {
        let t0 = 1_000.0;
        assert!(gap_ok(None, t0));
        assert!(!gap_ok(Some(t0), t0 + 3.0));
        assert!(gap_ok(Some(t0), t0 + MIN_ROLL_GAP.as_secs_f64()));
    }

    #[test]
    fn boottime_is_positive_and_monotone() {
        let a = boottime_secs();
        let b = boottime_secs();
        assert!(a > 0.0 && b >= a);
    }

    /// 真 inotify：别人读完 current.png → 轮换；本服务写 current.png、读池图 → 不触发；同一次休眠重复读只轮换一次。
    #[test]
    fn real_inotify_rolls_on_read_but_not_on_own_write() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = rmsvc_core::paths::Paths::resolve(move |k| if k == "HOME" { Some(h.clone()) } else { None });
        let store = Arc::new(WallpaperStore::new(&paths));
        store.ensure().unwrap();
        let png = |v: u8| {
            let img = image::GrayImage::from_pixel(8, 8, image::Luma([v]));
            let mut out = std::io::Cursor::new(Vec::new());
            img.write_to(&mut out, image::ImageFormat::Png).unwrap();
            out.into_inner()
        };
        std::fs::write(store.pool().join("a.png"), png(10)).unwrap();
        std::fs::write(store.pool().join("b.png"), png(200)).unwrap();
        store.activate("a.png").unwrap();
        spawn(store.clone(), Arc::new(rmsvc_core::events::EventBus::new()));
        std::thread::sleep(Duration::from_millis(300)); // 等监听建好

        // 本服务自己写 current.png + 读池图：不该触发轮换
        store.activate("a.png").unwrap();
        let _ = store.read("b.png").unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(store.state().current.as_deref(), Some("a.png"));

        // 模拟 xochitl 休眠时读 current.png → 轮换到 b
        let _ = std::fs::read(store.current_path()).unwrap();
        for _ in 0..50 {
            if store.state().current.as_deref() == Some("b.png") {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(store.state().current.as_deref(), Some("b.png"));
        // 紧接着又读一次（同一次休眠）：间隔内不再轮换
        let _ = std::fs::read(store.current_path()).unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(store.state().current.as_deref(), Some("b.png"));
    }
}
