//! inotify 防抖目录监听（单层目录）。**剥离移植**自旧项目 device-core `fswatch.rs` 的省电思路：
//! 空闲时阻塞读事件（进程睡死、零唤醒，配合设备 suspend），只有真有写入后才在防抖窗口内定时等待，
//! 静默结束回调一次并带上受影响的文件名集合。书架用它给 spool 目录追平（如上传中断留下的半成品）。
//! 两种形态：`watch_debounced` 常驻永不返回；`watch_until` 限时、回调说"完了"就撤——用于有头有尾的等待
//! （投原生后等 xochitl 渲染完），不给书库目录留常驻监听（§03z"不监听全盘"约束）。
use inotify::{Inotify, WatchMask};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const MASK: WatchMask = WatchMask::CLOSE_WRITE.union(WatchMask::MOVED_TO).union(WatchMask::CREATE).union(WatchMask::DELETE).union(WatchMask::MOVED_FROM);

/// 监听 `dir` 下文件的 CLOSE_WRITE/MOVED_TO/CREATE/DELETE/MOVED_FROM，防抖后回调（删除也算：网关看注册表目录用）。**永不返回**（init 失败返回）。
pub fn watch_debounced<F>(dir: &Path, debounce: Duration, mut on_settle: F)
where
    F: FnMut(&HashSet<String>),
{
    run(dir, debounce, None, |s| {
        on_settle(s);
        false
    });
}

/// 限时监听：同一套事件/防抖，但回调返回 `true` 即结束（本函数返回 `true`）；到 `timeout` 还没结束返回 `false`。
/// 返回时监听随之撤掉（读线程被"踢醒"退出，inotify 释放）。
pub fn watch_until<F>(dir: &Path, debounce: Duration, timeout: Duration, on_settle: F) -> bool
where
    F: FnMut(&HashSet<String>) -> bool,
{
    run(dir, debounce, Some(Instant::now() + timeout), on_settle)
}

fn run<F>(dir: &Path, debounce: Duration, deadline: Option<Instant>, mut on_settle: F) -> bool
where
    F: FnMut(&HashSet<String>) -> bool,
{
    let mut inotify = match Inotify::init() {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[fswatch] inotify init 失败: {e}");
            return false;
        }
    };
    if let Err(e) = inotify.watches().add(dir, MASK) {
        eprintln!("[fswatch] watch {} 失败: {e}", dir.display());
        return false;
    }
    // 限时模式：读线程阻塞在 read_events_blocking 上，目录不再有动静它就醒不来；结束时往私有"踢醒"目录写一个文件，
    // 让它看到通道已关而退出（inotify 随线程释放）。踢醒目录建不了就退化成"下次目录有动静时退出"。
    let kick = deadline.and_then(|_| {
        let k = kick_dir();
        std::fs::create_dir_all(&k).ok()?;
        inotify.watches().add(&k, WatchMask::CREATE).ok()?;
        Some(k)
    });
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            let events = match inotify.read_events_blocking(&mut buf) {
                Ok(ev) => ev,
                Err(e) => {
                    eprintln!("[fswatch] 读事件失败: {e}");
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            };
            for ev in events {
                if let Some(name) = ev.name.and_then(|n| n.to_str().map(|s| s.to_string())) {
                    if tx.send(name).is_err() {
                        return;
                    }
                }
            }
        }
    });
    let mut dirty: HashSet<String> = HashSet::new();
    let mut last = Instant::now();
    let done = loop {
        let now = Instant::now();
        if deadline.is_some_and(|d| now >= d) {
            break false;
        }
        if dirty.is_empty() {
            let got = match deadline {
                Some(d) => rx.recv_timeout(d - now),
                None => rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
            };
            match got {
                Ok(n) => {
                    dirty.insert(n);
                    last = Instant::now();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break false,
            }
            continue;
        }
        let elapsed = last.elapsed();
        if elapsed >= debounce {
            if on_settle(&dirty) {
                break true;
            }
            dirty.clear();
            continue;
        }
        let mut wait = debounce - elapsed;
        if let Some(d) = deadline {
            wait = wait.min(d.saturating_duration_since(Instant::now()));
        }
        match rx.recv_timeout(wait) {
            Ok(n) => {
                dirty.insert(n);
                last = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break false,
        }
    };
    drop(rx); // 先关通道，再踢醒读线程：它下一次 send 失败即退出
    if let Some(k) = kick {
        let _ = std::fs::write(k.join("kick"), b"");
        std::thread::sleep(Duration::from_millis(50));
        let _ = std::fs::remove_dir_all(&k);
    }
    done
}

fn kick_dir() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    std::env::temp_dir().join(format!("shelf-fswatch-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn coalesces_burst_into_one_callback() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().to_path_buf();
        let seen: Arc<Mutex<Vec<HashSet<String>>>> = Arc::new(Mutex::new(vec![]));
        let seen2 = seen.clone();
        let d2 = dir.clone();
        std::thread::spawn(move || {
            watch_debounced(&d2, Duration::from_millis(200), move |s| seen2.lock().unwrap().push(s.clone()));
        });
        std::thread::sleep(Duration::from_millis(150));
        for n in ["a.epub", "b.epub", "a.epub"] {
            std::fs::write(dir.join(n), b"x").unwrap();
            std::thread::sleep(Duration::from_millis(30));
        }
        std::thread::sleep(Duration::from_millis(600));
        let got = seen.lock().unwrap();
        assert_eq!(got.len(), 1, "应合并成一次回调: {:?}", *got);
        assert_eq!(got[0], ["a.epub", "b.epub"].into_iter().map(String::from).collect::<HashSet<_>>());
    }

    #[test]
    fn watch_until_returns_true_when_callback_done_and_false_on_timeout() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().to_path_buf();
        let d2 = dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            std::fs::write(d2.join("x.content"), b"{}").unwrap();
        });
        let t0 = Instant::now();
        let ok = watch_until(&dir, Duration::from_millis(50), Duration::from_secs(5), |s| s.contains("x.content"));
        assert!(ok && t0.elapsed() < Duration::from_secs(2), "看到文件即结束");
        let t0 = Instant::now();
        let ok = watch_until(&dir, Duration::from_millis(50), Duration::from_millis(300), |_| true);
        assert!(!ok && t0.elapsed() >= Duration::from_millis(300) && t0.elapsed() < Duration::from_secs(2), "没动静到点返回 false");
    }
}
