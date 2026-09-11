//! 服务端批量队列（2026-09-20 用户反馈驱动）。
//!
//! 此前批量是**浏览器逐个提交**（前端 `for…await` 循环）：①要先逐行勾选，100 本要点 100 下；②关掉页面/切设备，
//! 剩下没提交的就永远不会跑了。现在批量任务提交给网关，由网关自己的后台线程按顺序逐本执行，页面只负责提交
//! 和展示进度——关掉浏览器、换设备重开，队列照跑，状态照看。
//!
//! 放网关而不是 `book-serve`：网关是三个独立服务（优化/加入 xochitl 在 `book-serve`，加入 KOReader 在
//! `koreader-serve`）的唯一转发关口，且并发/内存预算闸门（[`crate::budget`]）就在这里——批量里每一本
//! 仍走同一道闸门（[`crate::budget::Budget::admit`]），跟别的操作互相排队、不叠加内存。
//! **顺序执行**：一次只处理一本（设备双核，优化内部已经在并行处理图片，见 `bookconv::imgpool`）。
//! **状态落盘**（`state/batch.json`，每次变化写一次）：网关重启（比如部署新版本）后 [`resume`] 读回未完成的队列
//! 继续跑——"关闭浏览器再回来能保持上次的未完记录并继续操作"（用户 2026-09-20 要求）。恢复时按最新母版库状态重新
//! 校验每一本（已经优化完的不会重做）。任务自带动作，一个队列里可以混合优化/加入 xochitl/加入 KOReader。
use rmsvc_core::paths::Paths;
use rmsvc_core::registry::{self, SvcClient};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::VecDeque;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Optimize,
    Deliver,
    Koreader,
}

impl Action {
    pub fn parse(s: &str) -> Option<Action> {
        match s {
            "optimize" => Some(Action::Optimize),
            "deliver" => Some(Action::Deliver),
            "koreader" => Some(Action::Koreader),
            _ => None,
        }
    }
    fn key(self) -> &'static str {
        match self {
            Action::Optimize => "optimize",
            Action::Deliver => "deliver",
            Action::Koreader => "koreader",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Job {
    action: Action,
    name: String,
    folder: String,
    /// 这一项被 worker 开始处理的次数（每次 pop 出来 +1，落盘）。网关在处理某本书途中崩溃/被重启时，
    /// `resume` 靠它区分"偶发中断、值得重放一次"和"这本书大概率就是崩溃元凶、别再重放"，见 [`recover_interrupted`]。
    #[serde(default)]
    attempts: u32,
}

/// 同一项中断后最多重放一次：`attempts` 达到这个数还落在 `current` 里 = 已经开始处理过 2 次都没走完。
const MAX_ATTEMPTS: u32 = 2;

#[derive(Default, Serialize, Deserialize)]
struct State {
    queue: VecDeque<Job>,
    current: Option<Job>,
    total: u32,
    done: u32,
    failed: Vec<(String, String)>,
    /// 最近一次处理的动作（给界面显示"批量优化/批量加入…"用；队列里混动作时取当前这一本的）。
    action: Option<Action>,
    #[serde(skip)]
    worker_alive: bool,
}

fn state() -> &'static Mutex<State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(State::default()))
}

fn lock() -> std::sync::MutexGuard<'static, State> {
    rmsvc_core::sync::lock(state())
}

fn file_of(paths: &Paths) -> std::path::PathBuf {
    paths.state_dir().join("batch.json")
}

/// 落盘互斥：**序列化与写盘放在同一把锁里**。此前 `persist` 在状态锁内序列化、出锁后才写文件，HTTP 线程
/// （`enqueue`/`stop`）与 worker 线程各自调用，较早序列化的旧快照可能比新快照后写盘，把队列回退到过期状态
/// （重启后 `resume` 会读到它）。现在后取得本锁的一定序列化得更晚，落盘顺序 = 状态变化顺序。
fn persist_lock() -> &'static Mutex<()> {
    static L: OnceLock<Mutex<()>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(()))
}

/// 落盘当前状态（每次变化调一次；失败只影响"重启后续跑"这一能力，不影响本次运行，静默）。
fn persist(paths: &Paths) {
    let _g = rmsvc_core::sync::lock(persist_lock());
    let json = { serde_json::to_vec(&*lock()).ok() };
    if let Some(b) = json {
        let _ = std::fs::create_dir_all(paths.state_dir());
        let _ = rmsvc_core::fs::write_atomic(&file_of(paths), &b);
    }
    // 状态每变一次就落一次盘，正好是"该通知网页刷新"的时机（前端不再批量运行时每 3 秒轮询）。
    crate::events::notify_books("batch");
}

/// 这本书该动作是否有意义（跟界面批量按钮同一套资格条件）：优化=EPUB/PDF 且还没优化；加入 xochitl=EPUB/PDF；
/// 加入 KOReader=KOReader 已安装。
pub fn eligible(action: Action, item: &Value, koreader_installed: bool) -> bool {
    let format = item.get("format").and_then(|v| v.as_str()).unwrap_or("");
    let is_book = format == "epub" || format == "pdf";
    match action {
        Action::Optimize => is_book && !item.get("optimized").and_then(|v| v.as_bool()).unwrap_or(false),
        Action::Deliver => is_book,
        Action::Koreader => koreader_installed,
    }
}

pub struct Enqueued {
    pub queued: usize,
    pub skipped: usize,
}

/// 到另一个服务的客户端（每次现建：`registry::find` 已是 O(1)，端口变了/服务重启后自然取到新地址）。
fn client(paths: &Paths, svc: &'static str, secs: u64) -> SvcClient {
    SvcClient::new(paths.clone(), svc, secs)
}

fn staging_items(paths: &Paths) -> Result<Vec<Value>, String> {
    let list = client(paths, "book-serve", 10).get_json("/staging")?;
    Ok(list.get("items").and_then(|v| v.as_array()).cloned().unwrap_or_default())
}

/// 入队。`names=None` 表示"母版库里所有该动作适用的书"；`Some` 只处理点名的。已经在队列里/正在处理的同名书跳过
/// （不重复排）；不适用的（如已优化的又点优化）计入 skipped。
pub fn enqueue(paths: &Paths, action: Action, names: Option<Vec<String>>, folder: &str) -> Result<Enqueued, String> {
    let koreader = registry::find(paths, "koreader-serve").is_some();
    let items = staging_items(paths)?;
    let wanted: Vec<String> = match &names {
        Some(n) => n.clone(),
        None => items.iter().filter_map(|it| it.get("name").and_then(|v| v.as_str()).map(str::to_string)).collect(),
    };
    let mut queued = 0usize;
    let mut skipped = 0usize;
    let spawn = {
        let mut st = lock();
        for name in wanted {
            let item = items.iter().find(|it| it.get("name").and_then(|v| v.as_str()) == Some(name.as_str()));
            let ok = item.map(|it| eligible(action, it, koreader)).unwrap_or(false);
            let dup = st.current.as_ref().map(|j| j.name == name && j.action == action).unwrap_or(false) || st.queue.iter().any(|j| j.name == name && j.action == action);
            if !ok {
                if names.is_some() {
                    skipped += 1; // 点名的不适用才算"跳过"；`names=None`（全部）时不适用的是预期内的筛选，不计
                }
                continue;
            }
            if dup {
                skipped += 1;
                continue;
            }
            st.queue.push_back(Job { action, name, folder: folder.to_string(), attempts: 0 });
            queued += 1;
        }
        if queued > 0 {
            start_round(&mut st, queued);
            st.action = Some(action);
            let spawn = !st.worker_alive;
            st.worker_alive = true;
            spawn
        } else {
            false
        }
    };
    if queued > 0 {
        persist(paths);
        if spawn {
            let paths = paths.clone();
            std::thread::spawn(move || worker(&paths));
        }
    }
    Ok(Enqueued { queued, skipped })
}

/// 记入这次新入队的 `queued` 本。一轮批量从空闲开始才重置计数（已经在跑的时候追加，累加进同一轮）；重置时**队列里
/// 可能还留着上一轮没跑的**（[`resume`] 等不到 book-serve 放弃时队列保留在内存里、没有 worker），它们会跟这次一起跑，
/// 所以总数按"重置后队列里实际有多少本"算——此前直接清零再加 `queued`，进度会显示成"5/2"。
fn start_round(st: &mut State, queued: usize) {
    if !st.worker_alive {
        st.done = 0;
        st.failed.clear();
        st.total = (st.queue.len() - queued) as u32;
    }
    st.total += queued as u32;
}

/// 当前批量状态（任何会话都能看）。
pub fn status() -> Value {
    let st = lock();
    json!({
        "running": st.worker_alive,
        "action": st.current.as_ref().map(|j| j.action.key()).or(st.action.map(Action::key)),
        "total": st.total,
        "done": st.done,
        "current": st.current.as_ref().map(|j| j.name.clone()),
        "queued": st.queue.iter().take(200).map(|j| j.name.clone()).collect::<Vec<_>>(),
        "queuedCount": st.queue.len(),
        "failed": st.failed.iter().map(|(n, m)| json!({"name": n, "message": m})).collect::<Vec<_>>(),
    })
}

/// **全部中止**：清空还没开始的；正在处理的那一本：还卡在并发闸门排队 → 取消排队（[`crate::budget::Budget::cancel`]）；
/// 已经在 `book-serve` 里跑 → 请求它取消（EPUB 优化、按卷拆分投递支持中途停，其它步骤会自然跑完，见
/// `Staging::request_cancel`）。返回被清掉的数量。
pub fn stop(paths: &Paths) -> usize {
    let (n, current) = {
        let mut st = lock();
        let n = st.queue.len();
        st.queue.clear();
        // 被清掉的不会再处理，总数同步扣掉，否则停止后进度还显示"1/4"（其实只有 1 本要做）。
        st.total = st.total.saturating_sub(n as u32);
        (n, st.current.clone())
    };
    if let Some(c) = current {
        if !crate::budget::global().cancel(&c.name) {
            let _ = post(paths, "book-serve", "/staging/cancel", json!({"name": c.name}), 10); // 尽力而为：没在跑/不可中断都无所谓
        }
    }
    persist(paths);
    n
}

/// 上次进程退出时还"进行中"的那一本：第一次中断放回队首重放（重新校验后从头再来）；已经处理过
/// [`MAX_ATTEMPTS`] 次都没走完的，记为失败不再重放——否则某本书稳定触发崩溃时，网关每次被 systemd 拉起都会
/// 先重放它再崩，形成崩溃循环，后面排队的书永远轮不到。
fn recover_interrupted(saved: &mut State) {
    let Some(cur) = saved.current.take() else { return };
    if cur.attempts >= MAX_ATTEMPTS {
        saved.done += 1;
        saved.failed.push((cur.name, format!("处理途中网关连续 {} 次中断（可能是这本书触发的崩溃），已跳过；可稍后手动重试", cur.attempts)));
    } else {
        saved.queue.push_front(cur);
    }
}

/// 网关启动时调用：读回上次没跑完的队列继续跑。**后台线程里等 `book-serve` 就绪**（开机/整体重启时网关可能先起），
/// 再按最新母版库状态重新校验每一本——上次进行中的那本如果已经优化完/不存在，就不再重做。
pub fn resume(paths: &Paths) {
    let Ok(text) = std::fs::read_to_string(file_of(paths)) else { return };
    let Ok(mut saved) = serde_json::from_str::<State>(&text) else { return };
    recover_interrupted(&mut saved);
    if saved.queue.is_empty() {
        // 没有未完成的：只把"上次结果"（完成数/失败原因）读回来供界面展示。
        *lock() = saved;
        return;
    }
    // 先把读回的队列装进内存（`worker_alive=true` 表示"有 worker 会来处理它"）：状态页立刻能看到排队项，
    // 等待 book-serve 期间用户再提交新批量也会并进这一份（不会 spawn 第二个 worker，也不会覆盖它）。
    // 此前是 120 秒等不到 book-serve 就直接 return，队列既没进内存、也没人再管，随后任何一次入队的
    // `persist` 都会把磁盘上这份未完成队列覆盖掉——开机时 book-serve 起得慢就会丢整个队列。
    {
        let mut st = lock();
        *st = saved;
        st.worker_alive = true;
    }
    let paths = paths.clone();
    std::thread::spawn(move || {
        let mut waited = Duration::ZERO;
        let mut n = 0u32;
        let items = loop {
            match staging_items(&paths) {
                Ok(i) => break Some(i),
                Err(_) if waited >= RESUME_WAIT_MAX => break None,
                Err(_) => {
                    // 开机/整体重启时网关可能先起：前 30 秒每 2 秒试一次，之后放宽到每 30 秒（极少见的长等待不必勤快探测）。
                    let step = Duration::from_secs(if n < 15 { 2 } else { 30 });
                    std::thread::sleep(step);
                    waited += step;
                    n += 1;
                }
            }
        };
        match items {
            Some(items) => {
                let koreader = registry::find(&paths, "koreader-serve").is_some();
                validate_queue(&mut lock(), &items, koreader);
            }
            // 等了 RESUME_WAIT_MAX 仍没有 book-serve：队列**保留**在内存和磁盘上（不清空、不丢），只是不再有人主动跑；
            // 下一次入队会带起 worker 连同这份旧队列一起处理，或用户在页面点"全部中止"清掉。
            None => {
                lock().worker_alive = false;
                eprintln!("[gateway] 批量队列恢复：等了 {} 分钟 book-serve 仍不可用，队列已保留，待下次入队时继续", RESUME_WAIT_MAX.as_secs() / 60);
                return;
            }
        }
        persist(&paths);
        worker(&paths); // 队列被 stop 清空则 worker 一进来就结束
    });
}

/// 等 book-serve 就绪的上限（见 [`resume`]）。
const RESUME_WAIT_MAX: Duration = Duration::from_secs(30 * 60);

/// 按最新母版库状态重新校验队列：不存在/已不适用（比如上次进行中的那本已经优化完）的项剔除，总数同步扣减。
fn validate_queue(st: &mut State, items: &[Value], koreader: bool) {
    let before = st.queue.len();
    st.queue.retain(|j| items.iter().find(|it| it.get("name").and_then(|v| v.as_str()) == Some(j.name.as_str())).map(|it| eligible(j.action, it, koreader)).unwrap_or(false));
    let dropped = (before - st.queue.len()) as u32;
    st.total = st.total.saturating_sub(dropped);
}

fn worker(paths: &Paths) {
    loop {
        let job = {
            let mut st = lock();
            match st.queue.pop_front() {
                Some(mut j) => {
                    j.attempts += 1; // 先记账再落盘：处理途中进程崩了，磁盘上的 current 已带着这次计数
                    st.current = Some(j.clone());
                    j
                }
                None => {
                    st.current = None;
                    st.worker_alive = false;
                    drop(st);
                    persist(paths);
                    return;
                }
            }
        };
        persist(paths);
        // release 已是 panic=unwind（见 Cargo.toml），这里能真正兜住 run_one 内的 panic，只让这一本失败。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_one(paths, &job))).unwrap_or_else(|_| Err("批量处理内部异常（已捕获）".to_string()));
        {
            let mut st = lock();
            st.done += 1;
            if let Err(e) = result {
                st.failed.push((job.name.clone(), e));
            }
        }
        persist(paths);
    }
}

/// POST 并只取给用户看的失败原因（服务端错误体里的 message，不带 "book-serve POST /x:" 这类前缀）。
fn post(paths: &Paths, svc: &'static str, path: &str, body: Value, secs: u64) -> Result<Value, String> {
    client(paths, svc, secs).try_post_json(path, &body).map_err(|e| e.message)
}

/// 这本书在母版库列表里的 `delivered.<kind>` 终态（`ok`/`failed`/`cancelled` + 文案）；条目没了（优化时改名）→ None。
fn final_check(paths: &Paths, name: &str, kind: &str) -> Option<(String, String)> {
    let list = client(paths, "book-serve", 10).get_json("/staging").ok()?;
    let it = list.get("items")?.as_array()?.iter().find(|it| it.get("name").and_then(|v| v.as_str()) == Some(name))?;
    let c = it.get("delivered")?.get(kind)?;
    Some((c.get("status")?.as_str()?.to_string(), c.get("message").and_then(|m| m.as_str()).unwrap_or("").to_string()))
}

fn run_one(paths: &Paths, job: &Job) -> Result<(), String> {
    registry::find(paths, "book-serve").ok_or("book-serve 未安装或未运行")?;
    let bytes = paths.staging_dir().join(&job.name).metadata().map(|m| m.len()).unwrap_or(0);
    // 同单条操作一样过并发/内存预算闸门；批量顺序执行，所以通常立即放行，只在别处同时在跑大书时才排队。
    let slot = crate::budget::global().admit(crate::budget::tier_of(bytes), &job.name).map_err(|e| e.message())?;
    let settled = |kind: &str, slot: crate::budget::Slot<'static>| -> Result<(), String> {
        crate::proxy::poll_until_settled(&client(paths, "book-serve", 10), &job.name);
        drop(slot);
        match final_check(paths, &job.name, kind) {
            Some((s, m)) if s == "failed" || s == "cancelled" => Err(m),
            _ => Ok(()),
        }
    };
    match job.action {
        Action::Optimize => {
            post(paths, "book-serve", "/staging/optimize", json!({"name": job.name}), 60)?;
            settled("optimize", slot)
        }
        Action::Deliver => {
            post(paths, "book-serve", "/staging/deliver", json!({"name": job.name, "folder": job.folder}), 60)?;
            settled("deliver", slot)
        }
        Action::Koreader => {
            registry::find(paths, "koreader-serve").ok_or("koreader-serve 未安装或未运行")?;
            let r = post(paths, "koreader-serve", "/books/adopt", json!({"name": job.name, "folder": job.folder}), 900);
            drop(slot);
            r?;
            // 记一笔"已加入 KOReader"（各服务只写自己的目录，落库记录归 book-serve），失败不算这本书失败。
            let _ = post(paths, "book-serve", "/staging/mark", json!({"name": job.name, "target": "koreader"}), 30);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(format: &str, optimized: bool) -> Value {
        json!({"name": "x", "format": format, "optimized": optimized})
    }

    #[test]
    fn eligibility_matches_selection_bar_buttons() {
        assert!(eligible(Action::Optimize, &item("epub", false), false));
        assert!(eligible(Action::Optimize, &item("pdf", false), false));
        assert!(!eligible(Action::Optimize, &item("epub", true), false), "已优化的不再优化");
        assert!(!eligible(Action::Optimize, &item("cbz", false), false), "只有 EPUB/PDF 能优化");
        assert!(eligible(Action::Deliver, &item("epub", true), false));
        assert!(!eligible(Action::Deliver, &item("cbz", false), false), "xochitl 只收 EPUB/PDF");
        assert!(eligible(Action::Koreader, &item("cbz", false), true));
        assert!(!eligible(Action::Koreader, &item("epub", false), false), "KOReader 没装不能加入");
    }

    #[test]
    fn action_parse_roundtrip() {
        for a in [Action::Optimize, Action::Deliver, Action::Koreader] {
            assert_eq!(Action::parse(a.key()), Some(a));
        }
        assert_eq!(Action::parse("nope"), None);
    }

    #[test]
    fn state_roundtrips_through_json_and_skips_worker_flag() {
        let mut st = State::default();
        st.queue.push_back(Job { action: Action::Deliver, name: "a.epub".into(), folder: "乱马1/2".into(), attempts: 0 });
        st.current = Some(Job { action: Action::Optimize, name: "b.epub".into(), folder: String::new(), attempts: 0 });
        st.total = 3;
        st.done = 1;
        st.failed.push(("c.epub".into(), "boom".into()));
        st.worker_alive = true;
        let text = serde_json::to_string(&st).unwrap();
        assert!(!text.contains("worker_alive"), "运行时标志不落盘");
        let back: State = serde_json::from_str(&text).unwrap();
        assert_eq!((back.total, back.done, back.queue.len(), back.failed.len()), (3, 1, 1, 1));
        assert_eq!(back.current.unwrap().name, "b.epub");
        assert_eq!(back.queue[0].folder, "乱马1/2", "任务自带的目标文件夹必须保留");
        assert!(!back.worker_alive);
    }

    #[test]
    fn recover_interrupted_replays_once_then_fails() {
        let job = |n: u32| Job { action: Action::Optimize, name: "b.epub".into(), folder: String::new(), attempts: n };
        // 第一次中断（attempts=1）：放回队首重放
        let mut st = State { current: Some(job(1)), total: 2, ..Default::default() };
        st.queue.push_back(Job { action: Action::Deliver, name: "c.epub".into(), folder: String::new(), attempts: 0 });
        recover_interrupted(&mut st);
        assert!(st.current.is_none() && st.failed.is_empty());
        assert_eq!(st.queue.front().unwrap().name, "b.epub", "第一次中断重放，且排在队首");
        assert_eq!(st.queue.len(), 2);
        // 重放后又中断（attempts=2）：记失败、不再重放，后面的书照常继续
        let mut st = State { current: Some(job(2)), total: 2, ..Default::default() };
        st.queue.push_back(Job { action: Action::Deliver, name: "c.epub".into(), folder: String::new(), attempts: 0 });
        recover_interrupted(&mut st);
        assert_eq!(st.queue.len(), 1, "只剩后面那本");
        assert_eq!((st.done, st.failed.len()), (1, 1));
        assert!(st.failed[0].1.contains("中断"), "{:?}", st.failed);
        // 老版本落盘文件没有 attempts 字段：默认 0，按第一次中断处理
        let old: Job = serde_json::from_str(r#"{"action":"optimize","name":"x","folder":""}"#).unwrap();
        assert_eq!(old.attempts, 0);
    }

    #[test]
    fn validate_queue_drops_missing_or_done_and_adjusts_total() {
        let job = |n: &str, a: Action| Job { action: a, name: n.into(), folder: String::new(), attempts: 0 };
        let mut st = State { total: 3, ..Default::default() };
        st.queue.push_back(job("keep.epub", Action::Optimize));
        st.queue.push_back(job("done.epub", Action::Optimize)); // 已优化 → 不再适用
        st.queue.push_back(job("gone.epub", Action::Deliver)); // 母版库里没了
        let items = vec![json!({"name": "keep.epub", "format": "epub", "optimized": false}), json!({"name": "done.epub", "format": "epub", "optimized": true})];
        validate_queue(&mut st, &items, false);
        assert_eq!(st.queue.iter().map(|j| j.name.as_str()).collect::<Vec<_>>(), ["keep.epub"]);
        assert_eq!(st.total, 1);
    }

    /// 回归：resume 放弃等待后队列留在内存里（没有 worker），下一次入队开新一轮时总数要把这些遗留项算进去。
    #[test]
    fn new_round_counts_leftover_queue_in_total() {
        let job = |n: &str| Job { action: Action::Optimize, name: n.into(), folder: String::new(), attempts: 0 };
        let mut st = State { total: 9, done: 7, ..Default::default() };
        st.failed.push(("old".into(), "x".into()));
        st.queue.push_back(job("left1"));
        st.queue.push_back(job("left2"));
        st.queue.push_back(job("new")); // 这次入队的 1 本
        start_round(&mut st, 1);
        assert_eq!((st.total, st.done, st.failed.len()), (3, 0, 0), "遗留 2 本 + 新入队 1 本");
        // 已经在跑时追加：只累加，不重置
        st.worker_alive = true;
        st.done = 1;
        st.queue.push_back(job("more"));
        start_round(&mut st, 1);
        assert_eq!((st.total, st.done), (4, 1));
    }

    #[test]
    fn stop_clears_pending_persists_and_adjusts_total() {
        let t = tempfile::tempdir().unwrap();
        let paths = Paths::resolve(|k| if k == "XDG_STATE_HOME" { Some(t.path().to_string_lossy().to_string()) } else { None });
        {
            let mut st = lock();
            st.queue.clear();
            st.queue.push_back(Job { action: Action::Optimize, name: "a".into(), folder: String::new(), attempts: 0 });
            st.queue.push_back(Job { action: Action::Optimize, name: "b".into(), folder: String::new(), attempts: 0 });
            st.total = 3; // 1 本已在做 + 2 本排队
        }
        assert_eq!(stop(&paths), 2);
        assert_eq!(status()["total"], 1, "停止后总数应只剩正在做的那 1 本");
        let saved: State = serde_json::from_str(&std::fs::read_to_string(file_of(&paths)).unwrap()).unwrap();
        assert!(saved.queue.is_empty(), "停止后落盘的队列也必须是空的，否则重启会把被中止的又跑起来");
    }
}
