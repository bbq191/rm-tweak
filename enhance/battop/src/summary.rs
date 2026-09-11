//! summary.json:面板用预聚合。
//! 读全部样本 → 4 窗口(当日/7天/30天/全部)× 应用(友好名)/进程(comm)双分组 + 唤醒源计数 + 每窗口放电
//! → 写 summary.json。窗口计算靠行内 epoch。
//!
//! 流式聚合：逐行喂 [`Summarizer`]，直接累加进 4 个窗口的表，内存只与「不同 key 数」成正比。旧实现先把
//! 40 天全部行读进 `Vec<(epoch,bool,String,u64)>`（几十万条各带一个 String）再对 4 个窗口各扫一遍，
//! 每 ~10 分钟白白分配/拷贝几十 MB。输出格式与旧实现逐字节一致（tie 时按名字升序，旧实现 tie 顺序取决于
//! HashMap 随机迭代序）。
//!
//! 按文件缓存（[`SummaryCache`]，2026-09-24）：常驻进程每轮都把 40 天全部样本（约 9MB、29 万行）重新读一遍
//! 解析一遍，host 实测 35ms/轮，设备上是几倍——battop 自己成了排行榜上的耗电项。样本文件按 UTC 日滚动，除了
//! 当天在追加的那个，其余内容不再变；而 4 个窗口里只有 today/7d/30d 三个起点会落在某个文件中间。所以每个
//! 文件缓存一份"整文件聚合"（+ CPU 行 epoch 的最小/最大值、全部 _sys 行），凭 (大小, mtime) 判失效：整个落在
//! 窗口内的直接加整份聚合、整个在窗口外的跳过，只有被窗口起点切开的文件和变过的文件才逐行重读。求和与合并
//! 顺序不影响结果（_sys 行按文件遍历顺序、文件内行序拼接，与逐行喂完全相同），输出逐字节不变。
use crate::store::is_sample_file;
use crate::util::json_esc;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;
use std::time::SystemTime;

const WINDOWS: [&str; 4] = ["today", "7d", "30d", "all"];

pub struct Summarizer {
    now: u64,
    starts: [u64; 4],
    app: [HashMap<String, u64>; 4],
    proc: [HashMap<String, u64>; 4],
    /// (epoch, cap%, charge_uah)
    sys: Vec<(u64, i64, i64)>,
}

fn add(m: &mut HashMap<String, u64>, key: &str, ms: u64) {
    match m.get_mut(key) {
        Some(v) => *v += ms,
        None => {
            m.insert(key.to_string(), ms);
        }
    }
}

impl Summarizer {
    /// `local_off`：本地时区相对 UTC 的偏移秒（决定「今日 0 点」）。
    pub fn new(now: u64, local_off: i64) -> Summarizer {
        let local = now as i64 + local_off;
        let midnight = (local - local.rem_euclid(86400) - local_off) as u64; // 本地今日 0 点(UTC epoch)
        Summarizer {
            now,
            starts: [midnight, now.saturating_sub(7 * 86400), now.saturating_sub(30 * 86400), 0],
            app: Default::default(),
            proc: Default::default(),
            sys: Vec::new(),
        }
    }

    /// 喂一行样本（畸形行静默忽略）。生产路径在 [`write_summary`] 里解析一次、同时喂 Summarizer 与文件缓存。
    #[cfg(test)]
    pub fn feed(&mut self, line: &str) {
        if let Some(r) = parse_row(line) {
            self.add_row(&r);
        }
    }

    fn add_row(&mut self, r: &Row) {
        match *r {
            Row::Cpu { epoch, is_app, key, ms } => {
                for (i, start) in self.starts.iter().enumerate() {
                    if epoch >= *start {
                        add(if is_app { &mut self.app[i] } else { &mut self.proc[i] }, key, ms);
                    }
                }
            }
            Row::Sys(t) => self.sys.push(t),
        }
    }

    /// 某个窗口起点落在文件的 CPU 行时间范围中间（`min < start <= max`）→ 必须逐行喂。
    fn splits(&self, agg: &FileAgg) -> bool {
        agg.cpu_range.is_some_and(|(min, max)| self.starts.iter().any(|&s| min < s && s <= max))
    }

    /// 并入一份没被任何窗口起点切开的整文件聚合：整个落在窗口内的窗口加整份，其余跳过。
    fn merge(&mut self, agg: &FileAgg) {
        if let Some((min, _)) = agg.cpu_range {
            for (i, start) in self.starts.iter().enumerate() {
                if min >= *start {
                    for (k, v) in &agg.app {
                        add(&mut self.app[i], k, *v);
                    }
                    for (k, v) in &agg.proc {
                        add(&mut self.proc[i], k, *v);
                    }
                }
            }
        }
        self.sys.extend_from_slice(&agg.sys);
    }

    /// 汇总成 summary.json 文本。`wakes`：(epoch, 原始唤醒源名)。
    pub fn finish(mut self, wakes: &[(u64, String)]) -> String {
        self.sys.sort_by_key(|t| t.0); // 稳定排序：同 epoch 保持输入序（同旧实现）
        let mut json = format!("{{\"generated\":{},\"windows\":{{", self.now);
        for (wi, (wname, start)) in WINDOWS.iter().zip(self.starts.iter()).enumerate() {
            // 放电:窗口内 _sys 相邻样本,容量% 下降之和(disc)+ 库仑计 µAh 下降之和(精确 mAh,v3)
            let s: Vec<&(u64, i64, i64)> = self.sys.iter().filter(|(e, _, _)| e >= start).collect();
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
            let mah = drained_uah / 1000; // 精确放电 mAh
            let ma = if disc_secs > 0 { drained_uah * 3600 / disc_secs as i64 / 1000 } else { 0 }; // 均放电 mA

            // 唤醒源计数(窗口内 journal 事件按友好名聚合)
            let mut wake: HashMap<String, u64> = HashMap::new();
            for (ep, name) in wakes {
                if ep >= start {
                    add(&mut wake, friendly_wake(name), 1);
                }
            }

            if wi > 0 {
                json.push(',');
            }
            json.push_str(&format!("\"{wname}\":{{\"discharge\":{disc},\"mah\":{mah},\"ma\":{ma},\"samples\":{},", s.len()));
            json.push_str(&format!("\"app\":{},", top_json(&self.app[wi])));
            json.push_str(&format!("\"proc\":{},", top_json(&self.proc[wi])));
            json.push_str(&format!("\"wake\":{}}}", top_json(&wake)));
        }
        json.push_str("}}\n");
        json
    }
}

/// 一行样本解析结果（畸形行 → `parse_row` 返回 None）。
enum Row<'a> {
    /// `epoch \t proc|app \t key \t cpu_ms …`；app 行的 key 已换成友好名。
    Cpu { epoch: u64, is_app: bool, key: &'a str, ms: u64 },
    /// `epoch \t _sys \t status \t awake \t cap \t disc [\t charge \t current]` → (epoch, cap%, charge_uah)
    Sys((u64, i64, i64)),
}

fn parse_row(line: &str) -> Option<Row<'_>> {
    let mut f = [""; 7];
    let mut n = 0;
    for part in line.split('\t').take(7) {
        f[n] = part;
        n += 1;
    }
    if n < 3 {
        return None;
    }
    let epoch = f[0].parse::<u64>().ok()?;
    match f[1] {
        kind @ ("proc" | "app") if n >= 4 => {
            let ms: u64 = f[3].parse().unwrap_or(0);
            let is_app = kind == "app";
            let key = if is_app { friendly(f[2]) } else { f[2] };
            Some(Row::Cpu { epoch, is_app, key, ms })
        }
        "_sys" if n >= 5 => {
            let cap: i64 = f[4].parse().unwrap_or(-1);
            let charge: i64 = if n >= 7 { f[6].parse().unwrap_or(-1) } else { -1 };
            Some(Row::Sys((epoch, cap, charge)))
        }
        _ => None,
    }
}

/// 一个样本文件的整文件聚合 + 让它失效的指纹（见模块头注「按文件缓存」）。
#[derive(Default)]
struct FileAgg {
    len: u64,
    mtime: Option<SystemTime>,
    /// CPU 行 epoch 的 (最小, 最大)；没有 CPU 行 = None。
    cpu_range: Option<(u64, u64)>,
    app: HashMap<String, u64>,
    proc: HashMap<String, u64>,
    /// 全部 _sys 行，按文件内行序。
    sys: Vec<(u64, i64, i64)>,
}

impl FileAgg {
    fn add_row(&mut self, r: &Row) {
        match *r {
            Row::Cpu { epoch, is_app, key, ms } => {
                self.cpu_range = Some(match self.cpu_range {
                    Some((lo, hi)) => (lo.min(epoch), hi.max(epoch)),
                    None => (epoch, epoch),
                });
                add(if is_app { &mut self.app } else { &mut self.proc }, key, ms);
            }
            Row::Sys(t) => self.sys.push(t),
        }
    }
}

/// 跨轮保留的样本文件缓存（常驻进程持有；键 = 文件名）。
#[derive(Default)]
pub struct SummaryCache {
    files: HashMap<String, FileAgg>,
}

/// 读目录下全部样本 → 聚合 → 原子写 summary.json。`cache` 由常驻循环跨轮持有（见模块头注）。
pub fn write_summary(dir: &Path, now: u64, local_off: i64, wakes: &[(u64, String)], cache: &mut SummaryCache) -> std::io::Result<()> {
    let mut sm = Summarizer::new(now, local_off);
    let mut line = String::new();
    let mut seen: Vec<String> = Vec::new();
    for e in fs::read_dir(dir)?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !is_sample_file(&name) {
            continue;
        }
        let md = e.metadata().ok();
        let (len, mtime) = (md.as_ref().map(|m| m.len()).unwrap_or(0), md.and_then(|m| m.modified().ok()));
        let fresh = cache.files.get(&name).filter(|a| mtime.is_some() && a.len == len && a.mtime == mtime);
        if let Some(agg) = fresh {
            if !sm.splits(agg) {
                sm.merge(agg);
                seen.push(name);
                continue;
            }
        }
        // 逐行读：文件变过/没缓存时顺手重建它的整文件聚合；缓存有效、只是被窗口起点切开时不重建。
        let mut rebuilt = if fresh.is_none() { Some(FileAgg { len, mtime, ..Default::default() }) } else { None };
        let Ok(f) = fs::File::open(e.path()) else { continue };
        let mut r = BufReader::new(f);
        loop {
            line.clear();
            match r.read_line(&mut line) {
                Ok(0) | Err(_) => break, // EOF / 非 UTF-8 坏文件：跳过其余（旧实现整文件丢弃）
                Ok(_) => {
                    if let Some(row) = parse_row(line.trim_end_matches(['\n', '\r'])) {
                        sm.add_row(&row);
                        if let Some(a) = rebuilt.as_mut() {
                            a.add_row(&row);
                        }
                    }
                }
            }
        }
        if let (Some(a), Some(_)) = (rebuilt, mtime) {
            cache.files.insert(name.clone(), a);
        }
        seen.push(name);
    }
    cache.files.retain(|k, _| seen.contains(k)); // 被 prune 掉的文件不留脏项
    let json = sm.finish(wakes);
    let tmp = dir.join("summary.json.tmp");
    fs::write(&tmp, json)?;
    fs::rename(&tmp, dir.join("summary.json"))
}

/// 取前 15 名(按值降序，同值按名升序保证输出稳定)→ JSON 数组;pct=占窗口总量百分比。
fn top_json(m: &HashMap<String, u64>) -> String {
    let total: u64 = m.values().sum();
    let mut v: Vec<(&String, &u64)> = m.iter().collect();
    v.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    v.truncate(15);
    let mut s = String::from("[");
    for (i, (k, ms)) in v.iter().enumerate() {
        if i > 0 {
            s.push(',');
        }
        let pct = (**ms * 100).checked_div(total).unwrap_or(0);
        s.push_str(&format!("{{\"name\":\"{}\",\"ms\":{},\"pct\":{}}}", json_esc(k), ms, pct));
    }
    s.push(']');
    s
}

/// systemd unit / comm → 友好应用名(应用视图用;设计 §5)。
fn friendly(unit: &str) -> &str {
    match unit {
        "xochitl" => "reMarkable 核心",
        "memfaultd" | "crashuploader" => "Memfault 遥测",
        "remarkable-counter-metrics" | "slumber-metrics" | "nm-metrics" | "battery-status-metrics" => "reMarkable 度量",
        "mdm-agent" => "reMarkable MDM",
        "rm-sync" | "update-engine" | "swupdate" => "reMarkable 同步/更新",
        "NetworkManager" | "wpa_supplicant" | "systemd-networkd" | "systemd-resolved" => "网络栈",
        "marker-manager" | "tee-supplicant" => "硬件",
        "wr-serve" | "cj-stars" | "wr-renew" | "battop" | "cangjie-wallpaper" => "cang-jie",
        "kernel" => "内核",
        _ if unit.ends_with("-metrics") => "reMarkable 度量",
        _ => unit,
    }
}

/// 唤醒源名 → 友好名。
fn friendly_wake(name: &str) -> &str {
    match name {
        "mwlan" => "WiFi",
        "xochitl.batterymanager" => "电池管理",
        "sleep.resume" => "唤醒锁",
        "udev.charger" => "充电器",
        "gpio-hall-sensors" => "合盖磁吸",
        "rtc0" | "rtc" => "定时器(RTC)",
        _ if name.starts_with("spi") => "触控笔(SPI)",
        _ if name.ends_with("pwrkey") => "电源键",
        // I2C 设备名形如 "0-0048" / "1-0021"
        _ if name.len() >= 3 && name.as_bytes()[0].is_ascii_digit() && name[1..].starts_with('-') => "传感器(I2C)",
        _ => name,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::ymd;

    /// 确定性造 45 天样本（含畸形行、无 charge 的旧格式 _sys 行）。与旧实现在同一数据集上产出的
    /// summary.json 逐字节对拍过（GOLDEN 即旧实现输出）。
    fn gen_days(now: u64) -> HashMap<String, String> {
        let mut day: HashMap<String, String> = HashMap::new();
        let mut ep = now - 45 * 86400;
        let mut i: u64 = 0;
        while ep <= now {
            let mut s = String::new();
            let disc = (i % 5).min(1);
            let keys = ["xochitl", "wr-serve", "memfaultd", "battop", "kernel", "NetworkManager", "foo-metrics"];
            for (k, key) in keys.iter().enumerate() {
                let ms = 100 + (i * 37 + k as u64 * 1013) % 5000 * 10 + k as u64;
                s.push_str(&format!("{ep}\tproc\t{key}\t{ms}\t{disc}\n"));
                s.push_str(&format!("{ep}\tapp\t{key}\t{}\t{disc}\n", ms + 3));
            }
            let cap = 100 - ((i / 3) % 100) as i64;
            let charge = 3_000_000 - (i as i64 % 3000) * 700;
            if i % 7 < 1 {
                s.push_str(&format!("{ep}\t_sys\tDischarging\t{}\t{cap}\t{disc}\n", i * 600));
            } else {
                s.push_str(&format!("{ep}\t_sys\tDischarging\t{}\t{cap}\t{disc}\t{charge}\t-120000\n", i * 600));
            }
            if i % 11 < 1 {
                s.push_str("garbage line\nxx\tproc\n");
            }
            day.entry(ymd(ep)).or_default().push_str(&s);
            ep += 3600 * 3 + 600;
            i += 1;
        }
        day
    }

    fn gen_wakes(now: u64) -> Vec<(u64, String)> {
        let names = ["mwlan", "rtc0", "spi1.0", "0-0048", "foo", "gpio-hall-sensors", "pwrkey"];
        let mut wakes = Vec::new();
        for (k, n) in names.iter().enumerate() {
            for m in 0..=(k as u64) {
                wakes.push((now - 100 - m, n.to_string()));
            }
            wakes.push((now - 8 * 86400, n.to_string()));
            wakes.push((now - 40 * 86400, n.to_string()));
        }
        wakes
    }

    const GOLDEN: &str = include_str!("../tests/golden-summary.json");

    #[test]
    fn matches_legacy_output_byte_for_byte() {
        let now = 1_760_000_000;
        let t = crate::util::testutil::tmp();
        for (k, v) in gen_days(now) {
            fs::write(t.path().join(format!("samples-{k}.tsv")), v).unwrap();
        }
        fs::write(t.path().join("baseline.tsv"), "not a sample").unwrap();
        let mut cache = SummaryCache::default();
        write_summary(t.path(), now, 8 * 3600, &gen_wakes(now), &mut cache).unwrap();
        assert_eq!(fs::read_to_string(t.path().join("summary.json")).unwrap(), GOLDEN);
        assert!(!t.path().join("summary.json.tmp").exists());
        // 第二轮全走缓存（文件都没变）：输出仍与黄金文件逐字节一致
        write_summary(t.path(), now, 8 * 3600, &gen_wakes(now), &mut cache).unwrap();
        assert_eq!(fs::read_to_string(t.path().join("summary.json")).unwrap(), GOLDEN);
    }

    /// 带缓存跨轮运行 vs 每轮新缓存（= 旧的全量逐行读）：窗口起点逐轮前移（跨本地/UTC 零点、跨文件边界）、
    /// 当天文件被追加、旧文件被删、某文件被改成坏行，每一轮输出都必须逐字节相同，且缓存确实被用上。
    #[test]
    fn cached_rounds_match_full_rescan() {
        let t = crate::util::testutil::tmp();
        let base_now = 1_760_000_000;
        for (k, v) in gen_days(base_now) {
            fs::write(t.path().join(format!("samples-{k}.tsv")), v).unwrap();
        }
        let wakes = gen_wakes(base_now);
        let mut cache = SummaryCache::default();
        let mut now = base_now;
        for round in 0..60u64 {
            now += 1_800 + round * 97; // 不规则步长，逐步跨过各窗口起点与文件边界
            let today = t.path().join(format!("samples-{}.tsv", ymd(now)));
            store_append(&today, &format!("{now}\tproc\tround{}\t{}\t1\n{now}\tapp\txochitl\t7\t1\n{now}\t_sys\tDischarging\t1\t{}\t1\t{}\t0\n", round % 4, round * 3, 90 - round as i64 % 7, 2_000_000 - round as i64 * 900));
            if round == 20 {
                // 删掉最老的一个文件（模拟 prune）
                let mut names: Vec<_> = fs::read_dir(t.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| is_sample_file(n)).collect();
                names.sort();
                fs::remove_file(t.path().join(&names[0])).unwrap();
            }
            if round == 35 {
                // 某个历史文件被改写（长度变了）→ 缓存必须失效
                let mut names: Vec<_> = fs::read_dir(t.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| is_sample_file(n)).collect();
                names.sort();
                store_append(&t.path().join(&names[3]), "garbage\n1\tproc\tlate\t5\t1\n");
            }
            write_summary(t.path(), now, 8 * 3600, &wakes, &mut cache).unwrap();
            let cached = fs::read_to_string(t.path().join("summary.json")).unwrap();
            write_summary(t.path(), now, 8 * 3600, &wakes, &mut SummaryCache::default()).unwrap();
            let full = fs::read_to_string(t.path().join("summary.json")).unwrap();
            assert_eq!(cached, full, "round {round} now {now}");
        }
        let files = fs::read_dir(t.path()).unwrap().flatten().filter(|e| is_sample_file(&e.file_name().to_string_lossy())).count();
        assert_eq!(cache.files.len(), files, "缓存只留现存文件");
    }

    fn store_append(p: &Path, s: &str) {
        crate::store::append(p, s).unwrap();
    }

    /// 性能对照（`cargo test --release -- --ignored --nocapture`）：40 天、每轮 30 个进程键 + 20 个应用键。
    #[test]
    #[ignore]
    fn bench_write_summary_40d() {
        let now = 1_760_000_000u64;
        let t = crate::util::testutil::tmp();
        let mut days: HashMap<String, String> = HashMap::new();
        let mut ep = now - 40 * 86400;
        let mut i = 0u64;
        while ep <= now {
            let s = days.entry(ymd(ep)).or_default();
            for k in 0..30 {
                s.push_str(&format!("{ep}\tproc\tcomm-{k}\t{}\t1\n", 10 + (i * 7 + k) % 900));
            }
            for k in 0..20 {
                s.push_str(&format!("{ep}\tapp\tunit-{k}\t{}\t1\n", 10 + (i * 3 + k) % 900));
            }
            s.push_str(&format!("{ep}\t_sys\tDischarging\t{}\t80\t1\t2000000\t-100000\n", i * 600));
            ep += 600;
            i += 1;
        }
        let bytes: usize = days.values().map(|v| v.len()).sum();
        for (k, v) in &days {
            fs::write(t.path().join(format!("samples-{k}.tsv")), v).unwrap();
        }
        let n = 5u32;
        let st = std::time::Instant::now();
        for _ in 0..n {
            write_summary(t.path(), now, 8 * 3600, &[], &mut SummaryCache::default()).unwrap();
        }
        let cold = st.elapsed() / n;
        let mut cache = SummaryCache::default();
        write_summary(t.path(), now, 8 * 3600, &[], &mut cache).unwrap();
        let st = std::time::Instant::now();
        for r in 0..n {
            let later = now + 600 * (r as u64 + 1);
            store_append(&t.path().join(format!("samples-{}.tsv", ymd(later))), &format!("{later}\tproc\tcomm-0\t5\t1\n"));
            write_summary(t.path(), later, 8 * 3600, &[], &mut cache).unwrap();
        }
        eprintln!("BENCH files={} bytes={bytes} full_rescan={cold:?} cached_round={:?}", days.len(), st.elapsed() / n);
    }

    #[test]
    fn today_window_uses_local_midnight() {
        // 本地 +08:00：UTC 2025-10-09 17:00 = 本地 10-10 01:00 → 今日 0 点 = UTC 10-09 16:00
        let now = 1_760_029_200 + 1000; // 2025-10-09 17:16:40 UTC
        let mut sm = Summarizer::new(now, 8 * 3600);
        let midnight = now - (now + 8 * 3600) % 86400;
        sm.feed(&format!("{}\tproc\tin\t10\t1", midnight));
        sm.feed(&format!("{}\tproc\tout\t99\t1", midnight - 1));
        let j = sm.finish(&[]);
        let today = &j[j.find("\"today\"").unwrap()..j.find("\"7d\"").unwrap()];
        assert!(today.contains("\"in\"") && !today.contains("\"out\""), "{today}");
    }

    #[test]
    fn empty_input_is_valid_shape() {
        let j = Summarizer::new(1000, 0).finish(&[]);
        assert!(j.starts_with("{\"generated\":1000,\"windows\":{\"today\":{\"discharge\":0,\"mah\":0,\"ma\":0,\"samples\":0,\"app\":[],\"proc\":[],\"wake\":[]}"));
        assert!(j.ends_with("}}\n"));
    }

    #[test]
    fn top15_ties_are_name_ordered_and_pct_safe() {
        let mut m = HashMap::new();
        for i in 0..20 {
            m.insert(format!("k{i:02}"), 5u64);
        }
        let j = top_json(&m);
        assert!(j.starts_with("[{\"name\":\"k00\""));
        assert_eq!(j.matches("\"name\"").count(), 15);
        assert_eq!(top_json(&HashMap::new()), "[]");
        let mut z = HashMap::new();
        z.insert("a".to_string(), 0u64);
        assert!(top_json(&z).contains("\"pct\":0"), "总量 0 不除零");
    }

    #[test]
    fn friendly_names() {
        assert_eq!(friendly("foo-metrics"), "reMarkable 度量");
        assert_eq!(friendly("weird"), "weird");
        assert_eq!(friendly_wake("0-0048"), "传感器(I2C)");
        assert_eq!(friendly_wake("1-x"), "传感器(I2C)");
        assert_eq!(friendly_wake("ab"), "ab");
        assert_eq!(friendly_wake("é-1"), "é-1", "非 ASCII 首字节不切片也不 panic");
    }
}

