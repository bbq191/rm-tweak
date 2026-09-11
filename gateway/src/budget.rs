//! 全局并发/内存预算闸门：漫画→PDF 那条改动做完后真机测出"optimize/超限分卷投递的内存峰值
//! ≈ 这次要处理的文件体积本身"（比如 245MB 源书 optimize 峰值 206MB）。`book-serve` 的忙锁
//! 是按书名分别加的，点不同的书互不阻塞——同时点几本大部头漫画，内存峰值会线性叠加，这台设备
//! 只有 ~2GB 内存、`systemd MemoryMax` 又没有真正生效，没有安全网，是真实的 OOM 风险。
//!
//! 落在这里（`gateway`）而不是各服务自己做：`book-serve`/`koreader-serve`/`gateway` 是三个
//! 完全独立的进程，进程内的 `Mutex`/`HashSet` 忙锁天然不跨进程；但所有跨服务请求（"优化"/
//! "加入xochitl"/"加入KOReader"）物理上都要经过网关这一个转发关口（见 `proxy.rs`），网关是
//! 天然的单点，用进程内的锁就够，不需要引入任何跨进程锁/共享内存/IPC。
//!
//! 不做"连续字节预算求和"这种精细模型——没有足够数据给不同操作类型/书籍类型精确的内存倍率，
//! 强行量化是假精确。按用户原话直接做成**两档**：大文件（超过 [`LARGE_THRESHOLD_BYTES`]）
//! 同一时刻最多一个在跑；小文件允许 [`MAX_SMALL_CONCURRENT`] 个同时跑。

use std::collections::HashSet;
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 超过这个体积算"大档"。独立于 book-serve 的 `native_limit`（xochitl 上传硬限）——两个数字
/// 恰好都是 90MB 只是巧合，语义不同（那边是"传不传得上 xochitl"，这边是"值不值得单独占一个大
/// 档名额"），不做跨进程配置同步。
pub const LARGE_THRESHOLD_BYTES: u64 = 90 * 1024 * 1024;
const MAX_SMALL_CONCURRENT: u32 = 3;
/// 排队等名额的上限——不是永久卡死，超时给清楚的错误文案，跟项目里 `FOLDER_WAIT_TIMEOUT`/
/// `PIECE_RENDER_TIMEOUT` 那种"等一个条件、有超时兜底"的既有模式一致。
const ADMIT_WAIT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tier {
    Large,
    Small,
}

/// 体积 → 档位。
pub fn tier_of(bytes: u64) -> Tier {
    if bytes > LARGE_THRESHOLD_BYTES {
        Tier::Large
    } else {
        Tier::Small
    }
}

/// `admit` 拿不到名额的原因；`status()` 是给 HTTP 层用的状态码。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmitError {
    /// 同名的书已经在排队或处理中（`pending`/`active` 都以书名为键，同名并存会互相抹掉记录、
    /// 第二个排队者还取消不掉，所以在入口直接拒绝）。
    Duplicate,
    /// 排队时被 [`Budget::cancel`] 取消。
    Cancelled,
    /// 等名额超过 [`ADMIT_WAIT_TIMEOUT`]。
    Timeout,
}

impl AdmitError {
    pub fn message(&self) -> String {
        match self {
            AdmitError::Duplicate => "这本书已经在排队或处理中，请等它完成（或先取消排队）".into(),
            AdmitError::Cancelled => "已取消排队".into(),
            AdmitError::Timeout => "排队等待并发处理名额超时，请稍后重试（可能有大部头正在处理）".into(),
        }
    }
    /// 409 冲突（重复提交）/ 503 暂时不可用（超时、被取消）。
    pub fn status(&self) -> u16 {
        match self {
            AdmitError::Duplicate => 409,
            _ => 503,
        }
    }
}

#[derive(Debug, Default)]
struct State {
    large: u32,
    small: u32,
    /// 书名集合，纯展示/取消用，**不参与准入判断本身**（`large`/`small` 计数已经是判断依据，
    /// 这两个集合只是给它们配一份"是哪本书"的可读信息）。2026-09-19 用户反馈：关掉浏览器/换
    /// 一个会话打开页面后，原来那套"取消排队"完全是浏览器标签页内存里的 JS 状态
    /// （`batchActive`/`batchAbort`），标签页一关就彻底没了——但网关这边真正排队等名额的书
    /// 完全不受影响、还在傻等，新打开的页面对此一无所知，队列里的书既看不出"正在排队"也点不了
    /// 停止，还能被当成"闲置条目"删除/再次提交，造成冲突。这两个集合是把"谁在排队/谁在跑"这份
    /// 状态从浏览器标签页挪到网关进程本身，天然跨会话/跨设备/跨标签页关闭都读得到。
    pending: HashSet<String>,
    active: HashSet<String>,
    /// 排队中被要求取消的书名——`admit_within` 每次被唤醒（含收到 `notify_all`）都会检查一遍，
    /// 命中就带着清楚的"已取消"错误提前放弃排队，不是超时。已经拿到名额、正在真正处理的救不
    /// 回来（[`Budget::cancel`] 只对 `pending` 生效），是诚实边界，不是没做全。
    cancel_requested: HashSet<String>,
}

#[derive(Debug)]
pub struct Budget {
    state: Mutex<State>,
    cv: Condvar,
}

impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}

impl Budget {
    pub fn new() -> Budget {
        Budget { state: Mutex::new(State::default()), cv: Condvar::new() }
    }

    /// 取状态锁；被毒化（持锁线程 panic）时照用内部数据——这里的状态只是计数和名字集合，
    /// 每次修改都是自洽的单步操作，宁可继续服务，也不要让一次 panic 让所有后续的优化/加入请求都跟着 panic。
    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        rmsvc_core::sync::lock(&self.state)
    }

    /// 阻塞直到拿到这个档位的名额（或等到 [`ADMIT_WAIT_TIMEOUT`] 超时/被 [`Budget::cancel`]
    /// 取消），返回一个 RAII guard，`Drop` 时自动释放名额、唤醒其他等待者。`name` 只用于
    /// 展示/取消（[`Budget::snapshot`]/[`Budget::cancel`]），不参与准入判断。
    pub fn admit(&self, tier: Tier, name: &str) -> Result<Slot<'_>, AdmitError> {
        self.admit_within(tier, name, ADMIT_WAIT_TIMEOUT)
    }

    /// `admit` 的实现，超时时长可注入——真实调用方永远用 [`ADMIT_WAIT_TIMEOUT`]（分钟级），
    /// 单测用毫秒级超时验证"占满后确实会超时报错"这条路径，不用真的空等 30 分钟。
    pub fn admit_within(&self, tier: Tier, name: &str, timeout: Duration) -> Result<Slot<'_>, AdmitError> {
        let deadline = Instant::now() + timeout;
        let mut guard = self.lock();
        if guard.pending.contains(name) || guard.active.contains(name) {
            return Err(AdmitError::Duplicate);
        }
        guard.cancel_requested.remove(name); // 清掉上一次遗留（排队超时的同一刻收到 cancel）的取消标记，别误杀这次
        guard.pending.insert(name.to_string());
        crate::events::notify_books("budget"); // 排队状态变了（在锁内通知只是往有界通道 try_send，不阻塞）
        loop {
            if guard.cancel_requested.remove(name) {
                guard.pending.remove(name);
                self.cv.notify_all();
                crate::events::notify_books("budget");
                return Err(AdmitError::Cancelled);
            }
            let has_room = match tier {
                Tier::Large => guard.large == 0,
                Tier::Small => guard.small < MAX_SMALL_CONCURRENT,
            };
            if has_room {
                match tier {
                    Tier::Large => guard.large += 1,
                    Tier::Small => guard.small += 1,
                }
                guard.pending.remove(name);
                guard.active.insert(name.to_string());
                crate::events::notify_books("budget");
                return Ok(Slot { budget: self, tier, name: name.to_string() });
            }
            let now = Instant::now();
            if now >= deadline {
                guard.pending.remove(name);
                guard.cancel_requested.remove(name);
                crate::events::notify_books("budget");
                return Err(AdmitError::Timeout);
            }
            let (g2, _) = self.cv.wait_timeout(guard, deadline - now).unwrap_or_else(|e| e.into_inner());
            guard = g2; // 醒来（虚假唤醒/真超时/真释放/真取消都在这里）重新判一次条件，不额外分支处理
        }
    }

    fn release(&self, tier: Tier, name: &str) {
        let mut guard = self.lock();
        match tier {
            Tier::Large => guard.large = guard.large.saturating_sub(1),
            Tier::Small => guard.small = guard.small.saturating_sub(1),
        }
        guard.active.remove(name);
        self.cv.notify_all();
        crate::events::notify_books("budget");
    }

    /// 当前排队中/正在跑的书名快照（`pending`, `active`）——给 `GET /api/budget/status` 用，
    /// 让任何会话（含关掉浏览器重开、换一台设备）都能看到网关真实的排队/处理状态，不用依赖
    /// 提交那次请求的浏览器标签页还活着。
    pub fn snapshot(&self) -> (Vec<String>, Vec<String>) {
        let guard = self.lock();
        (guard.pending.iter().cloned().collect(), guard.active.iter().cloned().collect())
    }

    /// 取消一个还在排队等待名额的书名——已经拿到名额、真正在跑的救不回来，老实返回 `false`，
    /// 不假装能中断正在执行的操作。`admit_within` 的等待循环每次被唤醒都会检查一遍
    /// `cancel_requested`，命中就带着"已取消"提前放弃排队。给"关掉浏览器/换一台设备后想停掉
    /// 还卡在排队里的项目"这个场景用——不依赖发起那次排队的会话还活着，是这次要修的根本问题。
    pub fn cancel(&self, name: &str) -> bool {
        let mut guard = self.lock();
        if !guard.pending.contains(name) {
            return false;
        }
        guard.cancel_requested.insert(name.to_string());
        self.cv.notify_all();
        true
    }
}

/// 一个名额；`Drop` 时自动归还。可以安全地 move 进另一个线程（比如等异步任务真正跑完再释放）。
#[derive(Debug)]
pub struct Slot<'a> {
    budget: &'a Budget,
    tier: Tier,
    name: String,
}

impl Drop for Slot<'_> {
    fn drop(&mut self) {
        self.budget.release(self.tier, &self.name);
    }
}

/// 进程内唯一实例——网关本身就是单进程，不需要通过 `bind()` 挂共享状态穿透各路由处理函数，
/// 用一个懒初始化的 `'static` 单例最省事，`proxy::forward` 直接调这个函数即可。
pub fn global() -> &'static Budget {
    static B: OnceLock<Budget> = OnceLock::new();
    B.get_or_init(Budget::new)
}

/// 判断"这个名字对应的条目是不是已经不再忙"——给异步操作（优化/落库）完成侦测用，输入是
/// `GET /staging` 原样返回的 JSON（`{"items":[...]}`）。两种情况都算"已完成"：条目还在列表里
/// 且 `busy==false`；条目已经不在列表里了（比如漫画→PDF 优化把 `<stem>.epub` 改名成
/// `<stem>.pdf`，原名字这时候找不到了，不能死等一个永远不会再出现的 `busy:false`）。只有"条目
/// 还在且 busy==true"才算没完成。解析失败（网络抖动/服务重启瞬间）也当"已完成"处理——宁可提前
/// 放行下一个排队的，不要因为侦测本身出错就把名额锁死。
pub fn is_settled(list_json: &serde_json::Value, name: &str) -> bool {
    let Some(items) = list_json.get("items").and_then(|v| v.as_array()) else { return true };
    match items.iter().find(|it| it.get("name").and_then(|v| v.as_str()) == Some(name)) {
        Some(it) => !it.get("busy").and_then(|v| v.as_bool()).unwrap_or(false),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn tier_of_boundary() {
        assert_eq!(tier_of(LARGE_THRESHOLD_BYTES), Tier::Small, "刚好等于阈值算小档（严格大于才算大）");
        assert_eq!(tier_of(LARGE_THRESHOLD_BYTES + 1), Tier::Large);
        assert_eq!(tier_of(1024), Tier::Small);
    }

    #[test]
    fn small_tier_allows_up_to_max_concurrent() {
        let b = Budget::new();
        let s1 = b.admit(Tier::Small, "a.epub").unwrap();
        let s2 = b.admit(Tier::Small, "b.epub").unwrap();
        let s3 = b.admit(Tier::Small, "c.epub").unwrap();
        assert_eq!(b.state.lock().unwrap().small, 3);
        drop((s1, s2, s3));
        assert_eq!(b.state.lock().unwrap().small, 0, "释放后计数应归零");
    }

    #[test]
    fn large_tier_second_admit_blocks_until_first_drops() {
        let b = Arc::new(Budget::new());
        let s1 = b.admit(Tier::Large, "big1.pdf").unwrap();
        let started = Arc::new(AtomicU32::new(0));
        let admitted_at = Arc::new(Mutex::new(None::<Instant>));

        let (b2, started2, admitted_at2) = (b.clone(), started.clone(), admitted_at.clone());
        let handle = std::thread::spawn(move || {
            started2.store(1, Ordering::SeqCst);
            let _s2 = b2.admit(Tier::Large, "big2.pdf").unwrap();
            *admitted_at2.lock().unwrap() = Some(Instant::now());
        });

        // 等第二个线程确实开始排队了
        while started.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(50));
        assert!(admitted_at.lock().unwrap().is_none(), "第一个名额还没释放，第二个不该被放行");

        let released_at = Instant::now();
        drop(s1);
        handle.join().unwrap();
        let got_at = admitted_at.lock().unwrap().unwrap();
        assert!(got_at >= released_at, "第二个必须在第一个释放之后才被放行");
    }

    #[test]
    fn admit_times_out_when_stuck() {
        let b = Budget::new();
        let _held = b.admit(Tier::Large, "held.pdf").unwrap(); // 占满 Large 档且不释放
        let started = Instant::now();
        let err = b.admit_within(Tier::Large, "stuck.pdf", Duration::from_millis(80)).unwrap_err();
        assert_eq!(err, AdmitError::Timeout);
        assert!(err.message().contains("超时"), "{}", err.message());
        assert!(started.elapsed() >= Duration::from_millis(80), "应该是真的等到超时才返回，不是立刻失败");
    }

    #[test]
    fn is_settled_true_when_entry_missing_or_not_busy() {
        let list = serde_json::json!({"items": [
            {"name": "a.epub", "busy": true},
            {"name": "b.pdf", "busy": false},
        ]});
        assert!(!is_settled(&list, "a.epub"), "还在忙不该算完成");
        assert!(is_settled(&list, "b.pdf"), "busy=false 算完成");
        assert!(is_settled(&list, "c.epub"), "条目已经不存在（比如改名）也算完成");
    }

    #[test]
    fn is_settled_defaults_to_true_on_malformed_json() {
        assert!(is_settled(&serde_json::json!({}), "a.epub"), "解析不出 items 时宁可放行不要锁死名额");
    }

    #[test]
    fn snapshot_reports_pending_and_active_names() {
        let b = Arc::new(Budget::new());
        let _s1 = b.admit(Tier::Large, "running.pdf").unwrap();
        let (b2, done) = (b.clone(), Arc::new(AtomicU32::new(0)));
        let handle = std::thread::spawn({
            let done = done.clone();
            // 测试末尾会 cancel 这次排队来收尾（不然线程会悬挂等到真超时），这里不能 unwrap()，
            // 取消是预期内的正常结束路径，不是异常。
            move || {
                let _ = b2.admit(Tier::Large, "queued.pdf");
                done.store(1, Ordering::SeqCst);
            }
        });
        // 等第二个真的排上队（进了 pending）再核对快照，避免线程调度时序偶发失败。
        let deadline = Instant::now() + Duration::from_secs(2);
        while !b.snapshot().0.contains(&"queued.pdf".to_string()) && Instant::now() < deadline {
            std::thread::yield_now();
        }
        let (pending, active) = b.snapshot();
        assert_eq!(pending, vec!["queued.pdf".to_string()]);
        assert_eq!(active, vec!["running.pdf".to_string()]);
        b.cancel("queued.pdf"); // 让排队线程能退出，测试不悬挂
        handle.join().unwrap();
    }

    #[test]
    fn cancel_unblocks_a_pending_admit_with_clear_error() {
        let b = Arc::new(Budget::new());
        let _held = b.admit(Tier::Large, "held.pdf").unwrap(); // 占满 Large 档
        let (b2, result) = (b.clone(), Arc::new(Mutex::new(None::<Result<(), AdmitError>>)));
        let (started, result2) = (Arc::new(AtomicU32::new(0)), result.clone());
        let handle = std::thread::spawn({
            let started = started.clone();
            move || {
                started.store(1, Ordering::SeqCst);
                let r = b2.admit_within(Tier::Large, "waiting.pdf", Duration::from_secs(5)).map(|_| ());
                *result2.lock().unwrap() = Some(r);
            }
        });
        while started.load(Ordering::SeqCst) == 0 {
            std::thread::yield_now();
        }
        std::thread::sleep(Duration::from_millis(30));
        assert!(b.cancel("waiting.pdf"), "确实在排队，取消应该成功");
        handle.join().unwrap();
        let got = result.lock().unwrap().take().unwrap();
        assert_eq!(got.unwrap_err(), AdmitError::Cancelled, "应该是清楚的取消错误，不是超时/挤占错误");
        assert!(!b.snapshot().0.contains(&"waiting.pdf".to_string()), "取消后不该再留在 pending 快照里");
    }

    #[test]
    fn duplicate_name_is_rejected_while_pending_or_active() {
        let b = Arc::new(Budget::new());
        let held = b.admit(Tier::Large, "same.pdf").unwrap();
        // 已在处理中 → 同名直接 409，且不影响原记录
        assert_eq!(b.admit(Tier::Small, "same.pdf").unwrap_err(), AdmitError::Duplicate);
        assert_eq!(AdmitError::Duplicate.status(), 409);
        assert_eq!(b.snapshot().1, vec!["same.pdf".to_string()], "原来的 active 记录不能被抹掉");
        // 排队中的同名也拒绝
        let b2 = b.clone();
        let h = std::thread::spawn(move || b2.admit_within(Tier::Large, "q.pdf", Duration::from_secs(5)).map(|_| ()));
        while !b.snapshot().0.contains(&"q.pdf".to_string()) {
            std::thread::yield_now();
        }
        assert_eq!(b.admit(Tier::Large, "q.pdf").unwrap_err(), AdmitError::Duplicate);
        assert!(b.cancel("q.pdf"), "排队者仍然取消得掉");
        assert_eq!(h.join().unwrap().unwrap_err(), AdmitError::Cancelled);
        drop(held);
        assert!(b.admit(Tier::Small, "same.pdf").is_ok(), "释放后可再次提交");
    }

    #[test]
    fn stale_cancel_flag_does_not_kill_next_admit() {
        let b = Budget::new();
        {
            let mut g = b.state.lock().unwrap();
            g.cancel_requested.insert("x.pdf".into()); // 模拟上次超时同一刻遗留的取消标记
        }
        assert!(b.admit(Tier::Small, "x.pdf").is_ok(), "遗留标记不该误杀新的提交");
    }

    #[test]
    fn cancel_returns_false_for_name_not_pending() {
        let b = Budget::new();
        assert!(!b.cancel("nonexistent.pdf"), "根本没有这个名字在排队，取消该老实报 false");
        let _s = b.admit(Tier::Small, "active.epub").unwrap();
        assert!(!b.cancel("active.epub"), "已经拿到名额在跑的不该被取消掉");
    }
}
