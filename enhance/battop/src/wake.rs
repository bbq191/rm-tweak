//! 唤醒源事件（设备/子系统级）：直接读 `/dev/kmsg` 内核环形缓冲区里的 `PM: active wakeup source: <NAME>`，
//! 落 wakes.tsv 缓存（另有 `wakes.cursor` 记本次开机已并入的最大记录序号，见 [`merge_records`]：按序号增量、
//! 用不含休眠的单调时钟换算墙钟——09-25 前按含休眠的 uptime 换算，事件时间被系统性提前）。①按小时级缓存（wakes.tsv 超 50 分钟才刷新）；②`/dev/kmsg` 本身是环形缓冲区，
//! 只有"当前这次开机"的记录——不再有旧实现（fork `journalctl` 查 31 天内核日志）那种跨 boot 查询能力，
//! 这是明知的退化。换来的是彻底消灭"采样进程里唯一的子进程创建"：2026-08-29 真机硬冻结的内核证据是
//! `cgroup_procs_write → percpu_down_write(cgroup_threadgroup_rwsem) → synchronize_rcu` 卡死（见
//! FINDINGS，那次是 systemd 反复拉起 oneshot 服务时的 cgroup 迁移，常驻化已经根治）；2026-09-23 又在
//! 常驻模型下复现一次冻机，冻结前最后一条日志与 wakes.tsv 的刷新时间戳精确重合到秒——不是同一个
//! `cgroup_procs_write` 机制（fork 子进程默认继承父进程 cgroup，不会走那条 syscall），但时间相关性足够
//! 可疑：进程创建（fork/exec/wait）本身仍是这条常驻循环里唯一残留的"非纯内存操作"，干脆去掉，不再赌它
//! 跟内核那条罕见路径有没有关系。
use crate::util::san;
use std::fs::{self, OpenOptions};
use std::io::{ErrorKind, Read, Seek, SeekFrom};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// 缓存保留窗口（与面板最大的 30d 窗口对齐并留 1 天余量）。`/dev/kmsg` 实际只有当前这次开机的记录，
/// 这个窗口现在的意义是"清理 wakes.tsv 里过老的行"，不再代表真能查到这么久——开机越久，这个窗口越接近
/// 真实覆盖范围。
const KEEP_SECS: u64 = 31 * 86400;
/// wakes.tsv 多久没刷新算过期。
const STALE_SECS: u64 = 3000;
/// Linux 通用 ABI（x86/arm/aarch64 等，本设备 aarch64 在内）统一的 `O_NONBLOCK` 值。没有 libc crate
/// （battop 保持零依赖），直接量入常量——非阻塞是为了读完环形缓冲区当前内容后立即返回，不等下一条
/// 未来才会出现的新内核消息（否则会拖死整个采样循环，重蹈 `run_bounded` 当初要防的那类问题）。
const O_NONBLOCK: i32 = 0o4000;
/// 单条 kmsg 记录的读取缓冲；内核消息通常远小于此，超长记录会被内核截断，可接受（丢的是我们不关心
/// 的字段，`active wakeup source: ` 这条消息很短，不会被截）。
const KMSG_BUF: usize = 8192;

/// 一条命中的唤醒源记录：(kmsg 序号, 开机以来微秒数, 源名)。
pub type KmsgWake = (u64, u64, String);

/// 解析单条 `/dev/kmsg` 记录（格式 `<pri>,<seq>,<ts_us>,<flags>[,extra];<message>`，`message` 后可能还
/// 跟着结构化字段续行，只看第一行），命中唤醒源就返回 (seq, ts_us, 源名)。
fn parse_kmsg_record(raw: &[u8]) -> Option<KmsgWake> {
    const KEY: &str = "active wakeup source: ";
    let text = String::from_utf8_lossy(raw);
    let first_line = text.lines().next()?;
    let (header, msg) = first_line.split_once(';')?;
    let mut h = header.split(',');
    let seq: u64 = h.nth(1)?.parse().ok()?;
    let ts_us: u64 = h.next()?.parse().ok()?;
    let idx = msg.find(KEY)?;
    let name = msg[idx + KEY.len()..].trim();
    if name.is_empty() {
        return None;
    }
    Some((seq, ts_us, name.to_string()))
}

/// 读当前 `/dev/kmsg` 环形缓冲区里全部唤醒源记录。None = 打开失败（调用方保留旧缓存）。非阻塞读到
/// `WouldBlock`（缓冲区当前内容已读完）即停，不等待未来的新消息。
fn read_wake_records() -> Option<Vec<KmsgWake>> {
    let mut f = OpenOptions::new().read(true).custom_flags(O_NONBLOCK).open("/dev/kmsg").ok()?;
    // 显式定位到缓冲区最早的记录（内核对 /dev/kmsg 的 SEEK_SET 有专门语义），不依赖新 fd 的默认位置；
    // 失败（比如内核不支持这个 seek）就照旧从当前位置读，尽力而为。
    let _ = f.seek(SeekFrom::Start(0));
    let mut out = Vec::new();
    let mut buf = [0u8; KMSG_BUF];
    // 上限只防意外死循环（环形缓冲区通常几千条）。
    for _ in 0..1_000_000 {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => out.extend(parse_kmsg_record(&buf[..n])),
            Err(e) if e.kind() == ErrorKind::WouldBlock => break,
            // 读的过程中最老的记录被新消息覆盖：内核回一次 EPIPE 并把位置跳到现存最老的一条，接着读即可
            // （09-25 前这里直接 break，丢掉其后的全部记录）。
            Err(e) if e.kind() == ErrorKind::BrokenPipe => continue,
            Err(_) => break,
        }
    }
    Some(out)
}

/// 增量游标：上次刷新时的开机 id 和已并入的最大唤醒记录序号（`wakes.cursor`，一行 `boot_id \t seq`）。
#[derive(Clone, Debug, PartialEq)]
pub struct Cursor {
    pub boot_id: String,
    pub seq: u64,
}

fn load_cursor(dir: &Path) -> Option<Cursor> {
    let t = fs::read_to_string(dir.join("wakes.cursor")).ok()?;
    let (b, s) = t.trim().split_once('\t')?;
    Some(Cursor { boot_id: b.to_string(), seq: s.parse().ok()? })
}

fn write_cursor(dir: &Path, c: &Cursor) {
    let tmp = dir.join("wakes.cursor.tmp");
    if fs::write(&tmp, format!("{}\t{}\n", c.boot_id, c.seq)).is_ok() {
        let _ = fs::rename(&tmp, dir.join("wakes.cursor"));
    }
}

/// 把 kmsg 读回的记录并进缓存（纯计算）。
///
/// **时间换算**：kmsg 的时间戳是内核 `local_clock`，**设备休眠期间不走**（`dmesg` 手册："timestamp could be
/// inaccurate … not updated after system SUSPEND/RESUME"）。09-25 前按"墙钟 − /proc/uptime（含休眠）+ ts"换算，
/// 每条事件都被提前了"开机以来它之前累计的休眠时长"——电纸书大部分时间在休眠，开机两天后的唤醒会被记到两天前，
/// 面板「今日 / 24 小时」的唤醒计数基本对不上。现改为"墙钟 − 单调时钟（同样不含休眠）+ ts"：`mono_offset`
/// 由调用方在刷新时刻算好；误差只剩"这条事件到本次刷新之间设备又休眠了多久"，而唤醒记录写在恢复那一刻、
/// 刷新在醒着的 10 分钟内就会发生，通常只差几秒。
///
/// **去重靠序号不靠时间**：同一次开机里 seq 单调递增，只并入 `seq > cursor.seq` 的记录——换算公式随休眠变化，
/// 同一条事件两次换算出的秒数不同，不能再像旧版那样按"时间 ≥ since 整段替换"。换了开机（或没有游标，
/// 即旧版缓存升级上来）：先丢掉缓存里 `>= boot_start`（本次开机以来）的条目——它们要么是旧算法记歪的、要么马上会
/// 从 kmsg 重新读到——再把 kmsg 里本次开机的记录全部并入。之前开机的条目换算时刻都早于那次关机，不受影响。
#[allow(clippy::too_many_arguments)]
pub fn merge_records(
    mut cache: Vec<(u64, String)>,
    records: Vec<KmsgWake>,
    cursor: Option<&Cursor>,
    boot_id: &str,
    boot_start: u64,
    mono_offset: u64,
    now: u64,
    cutoff: u64,
) -> (Vec<(u64, String)>, Cursor) {
    // boot_id 读不到（空串）时一律按"换了开机"处理：每次都整段重建本次开机的条目，幂等、不会重复。
    let last = cursor.filter(|c| !boot_id.is_empty() && c.boot_id == boot_id).map(|c| c.seq);
    if last.is_none() {
        cache.retain(|(e, _)| *e < boot_start);
    }
    let mut max_seq = last.unwrap_or(0);
    for (seq, ts_us, name) in records {
        if last.is_some_and(|l| seq <= l) {
            continue;
        }
        max_seq = max_seq.max(seq);
        cache.push(((mono_offset + ts_us / 1_000_000).min(now), name));
    }
    cache.retain(|(e, _)| *e >= cutoff);
    (cache, Cursor { boot_id: boot_id.to_string(), seq: max_seq })
}

/// wakes.tsv 是否需要刷新（不存在 / 超 STALE_SECS）。
fn is_stale(dir: &Path, now: u64) -> bool {
    fs::metadata(dir.join("wakes.tsv"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| now.saturating_sub(d.as_secs()) > STALE_SECS)
        .unwrap_or(true)
}

/// 刷新唤醒缓存（仅在过期时）：读 `/dev/kmsg` → 按序号增量并入 → 写 wakes.tsv（epoch \t 原始源名）+ 游标。
/// 打不开 kmsg **不动旧缓存**（mtime 不变，下一轮再试）。
pub fn refresh_if_stale(dir: &Path, now: u64) {
    if !is_stale(dir, now) {
        return;
    }
    let Some(records) = read_wake_records() else { return };
    let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id").map(|s| s.trim().to_string()).unwrap_or_default();
    let boot_start = now.saturating_sub(crate::procs::read_uptime());
    let mono_offset = now.saturating_sub(crate::util::monotonic_secs());
    let cursor = load_cursor(dir);
    let cutoff = now.saturating_sub(KEEP_SECS);
    let (merged, cur) = merge_records(load_cache(dir), records, cursor.as_ref(), &boot_id, boot_start, mono_offset, now, cutoff);
    write_cache(dir, &merged);
    write_cursor(dir, &cur);
}

fn write_cache(dir: &Path, ev: &[(u64, String)]) {
    let mut s = String::new();
    for (ep, name) in ev {
        s.push_str(&format!("{ep}\t{}\n", san(name)));
    }
    let tmp = dir.join("wakes.tsv.tmp");
    if fs::write(&tmp, s).is_ok() {
        let _ = fs::rename(&tmp, dir.join("wakes.tsv"));
    }
}

/// 读唤醒缓存。
pub fn load_cache(dir: &Path) -> Vec<(u64, String)> {
    let Ok(content) = fs::read_to_string(dir.join("wakes.tsv")) else { return Vec::new() };
    content
        .lines()
        .filter_map(|line| {
            let (e, n) = line.split_once('\t')?;
            Some((e.parse::<u64>().ok()?, n.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kmsg_record_wake_source() {
        let raw = b"6,1234,100000000,-;PM: active wakeup source: mwlan";
        assert_eq!(parse_kmsg_record(raw), Some((1234, 100_000_000, "mwlan".to_string())));
    }

    #[test]
    fn parse_kmsg_record_handles_continuation_lines() {
        // dictionary 续行（SUBSYSTEM=/DEVICE=）只看第一行，不影响解析。
        let raw = b"6,1234,100000000,-;PM: active wakeup source: spi1.0\n SUBSYSTEM=platform\n DEVICE=+platform:spi1.0";
        assert_eq!(parse_kmsg_record(raw), Some((1234, 100_000_000, "spi1.0".to_string())));
    }

    #[test]
    fn parse_kmsg_record_trims_whitespace_in_name() {
        let raw = b"6,1,0,-;PM: active wakeup source:   spi1.0  ";
        assert_eq!(parse_kmsg_record(raw), Some((1, 0, "spi1.0".to_string())));
    }

    #[test]
    fn parse_kmsg_record_ignores_unrelated_message() {
        assert_eq!(parse_kmsg_record(b"6,1,0,-;unrelated kernel message"), None);
    }

    #[test]
    fn parse_kmsg_record_rejects_empty_name() {
        assert_eq!(parse_kmsg_record(b"6,1,0,-;PM: active wakeup source: "), None);
    }

    #[test]
    fn parse_kmsg_record_bad_header_is_none() {
        assert_eq!(parse_kmsg_record(b"garbage no semicolon"), None);
        assert_eq!(parse_kmsg_record(b"6,1;PM: active wakeup source: x"), None); // 缺 ts_us 字段
        assert_eq!(parse_kmsg_record(b"6,1,notanumber,-;PM: active wakeup source: x"), None);
        assert_eq!(parse_kmsg_record(b"6,x,0,-;PM: active wakeup source: x"), None); // seq 非数字
    }

    fn ev(v: &[(u64, &str)]) -> Vec<(u64, String)> {
        v.iter().map(|(e, n)| (*e, n.to_string())).collect()
    }

    fn rec(v: &[(u64, u64, &str)]) -> Vec<KmsgWake> {
        v.iter().map(|(s, t, n)| (*s, *t, n.to_string())).collect()
    }

    /// 休眠不计入换算：开机 10000 s（含 9000 s 休眠）、单调时钟 1000 s；ts=900 s 的事件应在"100 s 前"，
    /// 不是旧算法的"9100 s 前"。
    #[test]
    fn epoch_uses_monotonic_offset_not_boottime() {
        let now = 1_000_000;
        let (boot_start, mono_offset) = (now - 10_000, now - 1_000);
        let (m, c) = merge_records(vec![], rec(&[(7, 900_000_000, "rtc")]), None, "B", boot_start, mono_offset, now, 0);
        assert_eq!(m, ev(&[(now - 100, "rtc")]));
        assert_eq!(c, Cursor { boot_id: "B".into(), seq: 7 });
    }

    #[test]
    fn same_boot_appends_only_new_seqs() {
        let cache = ev(&[(100, "old-boot"), (5_000, "a")]);
        let cur = Cursor { boot_id: "B".into(), seq: 3 };
        // seq 2、3 已并入过（换算时刻变了也不能再加一遍），只加 4、5
        let recs = rec(&[(2, 1_000_000, "a"), (3, 2_000_000, "b"), (4, 3_000_000, "c"), (5, 4_000_000, "d")]);
        let (m, c) = merge_records(cache, recs, Some(&cur), "B", 4_000, 5_000, 9_999, 0);
        assert_eq!(m, ev(&[(100, "old-boot"), (5_000, "a"), (5_003, "c"), (5_004, "d")]));
        assert_eq!(c.seq, 5);
        // 什么都没新增：缓存不变、游标不倒退
        let (m2, c2) = merge_records(m.clone(), rec(&[(5, 4_000_000, "d")]), Some(&c), "B", 4_000, 5_000, 9_999, 0);
        assert_eq!((m2, c2.seq), (m, 5));
    }

    #[test]
    fn new_boot_or_upgrade_rebuilds_this_boots_entries_and_trims_old() {
        // 上次开机的条目（< boot_start）保留；本次开机旧算法记下的（>= boot_start）丢掉后从 kmsg 重建。
        let cache = ev(&[(10, "too-old"), (500, "prev-boot"), (2_000, "this-boot-old-calc")]);
        let recs = rec(&[(1, 5_000_000, "x")]);
        for cur in [None, Some(Cursor { boot_id: "OLD".into(), seq: 99 })] {
            let (m, c) = merge_records(cache.clone(), recs.clone(), cur.as_ref(), "NEW", 1_000, 1_500, 9_999, 100);
            assert_eq!(m, ev(&[(500, "prev-boot"), (1_505, "x")]), "cursor={cur:?}");
            assert_eq!(c, Cursor { boot_id: "NEW".into(), seq: 1 });
        }
        // boot_id 读不到：每次都按新开机整段重建，重复刷新不产生重复条目
        let (m1, c1) = merge_records(cache.clone(), recs.clone(), None, "", 1_000, 1_500, 9_999, 100);
        let (m2, _) = merge_records(m1.clone(), recs, Some(&c1), "", 1_000, 1_600, 9_999, 100);
        assert_eq!(m2, ev(&[(500, "prev-boot"), (1_605, "x")]));
    }

    #[test]
    fn epoch_is_clamped_to_now() {
        let (m, _) = merge_records(vec![], rec(&[(1, 50_000_000, "x")]), None, "B", 0, 1_000, 1_010, 0);
        assert_eq!(m, ev(&[(1_010, "x")]));
    }

    #[test]
    fn cursor_roundtrip() {
        let t = crate::util::testutil::tmp();
        assert_eq!(load_cursor(t.path()), None);
        let c = Cursor { boot_id: "0f1e-aa".into(), seq: 42 };
        write_cursor(t.path(), &c);
        assert_eq!(load_cursor(t.path()), Some(c));
        std::fs::write(t.path().join("wakes.cursor"), "garbage").unwrap();
        assert_eq!(load_cursor(t.path()), None);
    }

    #[test]
    fn cache_roundtrip_skips_garbage() {
        let t = crate::util::testutil::tmp();
        std::fs::write(t.path().join("wakes.tsv"), "100\ta\nbad line\nxx\tb\n200\tc d\n").unwrap();
        assert_eq!(load_cache(t.path()), ev(&[(100, "a"), (200, "c d")]));
        write_cache(t.path(), &ev(&[(1, "x\ty")]));
        assert_eq!(load_cache(t.path()), ev(&[(1, "x y")]));
    }

    #[test]
    fn stale_when_missing_or_old() {
        let t = crate::util::testutil::tmp();
        assert!(is_stale(t.path(), 1_000_000_000));
        write_cache(t.path(), &[]);
        let now = crate::util::now_secs();
        assert!(!is_stale(t.path(), now));
        assert!(is_stale(t.path(), now + STALE_SECS + 5));
    }
}
