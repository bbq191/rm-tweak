//! transcribe-serve —— 笔记·转写（loopback 8796）。订阅 ink-serve 事件：条目库有新手写 → 取裁图 → 视觉模型 → 草稿写回
//! （经 ink-serve 的 HTTP，条目库唯一写者仍是它；已校对 `text` 永不被覆盖）。唯一出网的笔记服务：设备自己的 WiFi 直连。
//! 路由（经网关前缀 `/api/transcribe`）：`GET /status` · `GET /config` · `PUT /config`（`apiKey` 只写不读）·
//! `POST /run`（同步跑一轮）· `POST /books/{uuid}/entries/{id}`（强制转写一条）· `POST /retry`（清失败记录）· `GET /events`。
mod backend;
mod config;
mod ink;
mod ledger;
mod prompt;
mod worker;

use backend::{OpenAiCompat, Vision};
use config::TranscribeConfig;
use ink::{EntryStore, InkHttp};
use ledger::Ledger;
use rmsvc_core::events::{parse_sse_line, EventBus};
use rmsvc_core::http::{bind, ApiError, Reply, Router};
use rmsvc_core::paths::Paths;
use rmsvc_core::registry;
use rmsvc_core::service::{self, ServiceSpec};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::mpsc::{sync_channel, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use vendorcfg::VendorConfig;
use worker::{Ctx, Failures, Target};

pub const APP: &str = "notes";

const SPEC: ServiceSpec = ServiceSpec { name: "transcribe-serve", label: "笔记·转写", version: env!("CARGO_PKG_VERSION"), default_bind: "127.0.0.1:8796", tab: None };
/// 事件到跑之间的防抖：合上书 ink-serve 会连发几条。
const DEBOUNCE: Duration = Duration::from_secs(3);

/// 「各个模型的用量花费 profile」（整理区第二轮反馈点 2，2026-09-08）：预置表里的每个模型都出一行
/// （哪怕还没调用过，用量全 0——方便用户先把价格填上）；账本里出现过但不在预置表里的（比如用过的
/// 自定义模型）也补进来。花费只在用户填过单价（`config.rs` 的 `prices`）时才算，没填就是 `null`，
/// 网页只显示 token 数不显示金额——理由见 `config.rs` 模块文档"花费不做官方定价表"。
fn usage_profile(cfg: &TranscribeConfig, usage: &ledger::Usage) -> serde_json::Value {
    let mut keys: Vec<String> = config::PRESETS.iter().map(|p| p.id.to_string()).collect();
    for k in usage.by_model.keys() {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    let rows: Vec<serde_json::Value> = keys
        .into_iter()
        .map(|k| {
            let label = config::PRESETS.iter().find(|p| p.id == k).map(|p| p.label.to_string()).unwrap_or_else(|| k.clone());
            let m = usage.by_model.get(&k).cloned().unwrap_or_default();
            let price = cfg.prices.get(&k).copied().unwrap_or_default();
            let cost = if price.input_per1k > 0.0 || price.output_per1k > 0.0 {
                Some((m.prompt_tokens as f64 / 1000.0) * price.input_per1k + (m.completion_tokens as f64 / 1000.0) * price.output_per1k)
            } else {
                None
            };
            serde_json::json!({"id": k, "label": label, "active": k == cfg.usage_key(),
                "calls": m.calls, "ok": m.ok, "failed": m.failed,
                "promptTokens": m.prompt_tokens, "completionTokens": m.completion_tokens,
                "lastError": m.last_error, "lastAt": m.last_at,
                "price": price, "costEstimate": cost})
        })
        .collect();
    serde_json::Value::Array(rows)
}

struct State {
    paths: Paths,
    cfg_path: PathBuf,
    cfg: Mutex<TranscribeConfig>,
    ledger: Ledger,
    failures: Failures,
    bus: Arc<EventBus>,
    store: InkHttp,
    /// 同一时刻只跑一轮（自动与手动互斥）。
    run_lock: Mutex<()>,
    trigger: SyncSender<()>,
}

impl State {
    fn cfg(&self) -> TranscribeConfig {
        self.cfg.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn vision(&self, cfg: &TranscribeConfig) -> Result<Box<dyn Vision>, String> {
        let key = cfg.key().ok_or("未配置 API key（网页「转写设置」里粘贴，或环境变量 DASHSCOPE_API_KEY）")?;
        Ok(Box::new(OpenAiCompat::new(&cfg.backend, cfg.base_url(), cfg.model(), &key, Duration::from_secs(cfg.timeout_secs))))
    }
    /// 跑一轮（阻塞拿锁）。没 key → 直接报告不出网。
    fn run(&self, only: Option<Target<'_>>) -> ledger::RunReport {
        let _g = self.run_lock.lock().unwrap_or_else(|e| e.into_inner());
        let cfg = self.cfg();
        let now = rmsvc_core::clock::now_secs();
        let report = match self.vision(&cfg) {
            Ok(v) => worker::run_once(&Ctx { store: &self.store, vision: v.as_ref(), cfg: &cfg, ledger: &self.ledger, failures: &self.failures, now }, only),
            Err(e) => {
                let r = ledger::RunReport { at: now, note: e, ..Default::default() };
                self.ledger.record_run(r.clone());
                r
            }
        };
        if report.done > 0 || report.failed > 0 {
            println!("[transcribe-serve] 一轮：扫 {} 成 {} 败 {} 跳 {} 余 {} {}", report.scanned, report.done, report.failed, report.skipped, report.left, report.note);
        }
        self.bus.publish("notes", "transcribe");
        report
    }
    fn kick(&self) {
        match self.trigger.try_send(()) {
            Ok(()) | Err(TrySendError::Full(())) => {}
            Err(TrySendError::Disconnected(())) => eprintln!("[transcribe-serve] 工作线程没了"),
        }
    }
}

/// 订阅 ink-serve `/events`：条目变了就踢一下工作线程（断线 3 s 重连，与网关汇聚同一套路）。
fn watch_ink(st: Arc<State>) {
    let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(3)).build();
    loop {
        if let Some(info) = registry::find(&st.paths, "ink-serve") {
            if let Ok(resp) = agent.get(&format!("{}/events", info.base_url())).call() {
                for line in BufReader::new(resp.into_reader()).lines().map_while(Result::ok) {
                    let Some(json) = parse_sse_line(&line) else { continue };
                    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
                    if v["area"] == "notes" && v["kind"] == "entries" && st.cfg().auto {
                        st.kick();
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_secs(3));
    }
}

/// 工作线程：收到踢 → 防抖 → 跑一轮。
fn work_loop(st: Arc<State>, rx: Receiver<()>) {
    while rx.recv().is_ok() {
        std::thread::sleep(DEBOUNCE);
        while rx.try_recv().is_ok() {}
        st.run(None);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind_addr = service::parse_bind(&args, SPEC.default_bind);
    let paths = Paths::from_env();
    let cfg_path = paths.app_config_dir(APP).join("transcribe.json");
    // `.migrate()`：老配置文件（重做模型预置表之前，2026-09-08 上午之前落盘的，单一 model/baseUrl/apiKey
    // 三件套）搬进新形状——不迁移的话真机已经保存的 key 会在升级后凭空消失，见 config.rs 模块文档。
    let cfg = rmsvc_core::config::load_or_seed::<TranscribeConfig>(&cfg_path).migrate();
    rmsvc_core::fs::set_mode(&cfg_path, 0o600);
    let _ = rmsvc_core::config::save(&cfg_path, &cfg, Some(0o600)); // 迁移后落盘一次，文件形状跟运行时一致
    let (tx, rx) = sync_channel::<()>(2);
    let st = Arc::new(State {
        paths: paths.clone(),
        cfg_path,
        cfg: Mutex::new(cfg),
        ledger: Ledger::open(&paths.app_state_dir(APP).join("transcribe.json")),
        failures: Failures::default(),
        bus: Arc::new(EventBus::new()),
        store: InkHttp::new(paths.clone()),
        run_lock: Mutex::new(()),
        trigger: tx,
    });
    {
        let s = st.clone();
        std::thread::spawn(move || work_loop(s, rx));
        let s = st.clone();
        std::thread::spawn(move || watch_ink(s));
        // 追平：启动后踢一次（ink-serve 可能已积了待转写；没 key 则一轮空跑只记 note）
        if st.cfg().auto {
            st.kick();
        }
    }
    let router = Router::new()
        .get("/events", bind(&st, |s, _| Ok(s.bus.sse_reply())))
        .get("/status", bind(&st, |s, _| {
            let (ink, pending) = match s.store.list_books() {
                Ok(b) => (true, b.iter().map(|x| x.pending).sum::<usize>()),
                Err(_) => (false, 0),
            };
            let cfg = s.cfg();
            Ok(Reply::ok(&serde_json::json!({"config": cfg.public(), "usage": s.ledger.snapshot(), "usageByModel": usage_profile(&cfg, &s.ledger.snapshot()), "failures": s.failures.list(), "inkReachable": ink, "pending": pending})))
        }))
        .get("/config", bind(&st, |s, _| Ok(Reply::ok(&s.cfg().public()))))
        .put("/config", bind(&st, |s, r| {
            let j = r.json()?;
            let mut cfg = s.cfg.lock().unwrap_or_else(|e| e.into_inner());
            let mut next = cfg.clone();
            next.apply(&j.0).map_err(ApiError::bad)?;
            rmsvc_core::config::save(&s.cfg_path, &next, Some(0o600)).map_err(ApiError::internal)?;
            *cfg = next.clone();
            drop(cfg);
            let has_key = next.key().is_some();
            s.bus.publish("notes", "transcribe");
            if has_key && next.auto {
                s.kick();
            }
            Ok(Reply::ok(&next.public()))
        }))
        .post("/run", bind(&st, |s, _| Ok(Reply::ok(&s.run(None)))))
        .post("/books/{uuid}/entries/{id}", bind(&st, |s, r| {
            let (uuid, id) = (r.param("uuid").to_string(), r.param("id").to_string());
            let rep = s.run(Some(Target { uuid: &uuid, id: &id }));
            if rep.done == 1 {
                // 点「重转」弹出这次调用的消耗（token）——不是账本累计，是这一次调用的实际数字。
                Ok(Reply::ok(&serde_json::json!({"ok": true, "promptTokens": rep.prompt_tokens, "completionTokens": rep.completion_tokens})))
            } else {
                Err(ApiError::bad(if rep.note.is_empty() { "没有这条目或它没有手写".to_string() } else { rep.note }))
            }
        }))
        .post("/retry", bind(&st, |s, _| {
            s.failures.clear();
            s.kick();
            Ok(Reply::ok(&serde_json::json!({"ok": true})))
        }));
    println!("[transcribe-serve] 配置 {}；后端 {} {}；key {:?}", st.cfg_path.display(), st.cfg().backend, st.cfg().model(), st.cfg().key_source());
    if let Err(e) = service::run(&SPEC, &bind_addr, &paths, router) {
        eprintln!("[transcribe-serve] {e}");
        std::process::exit(1);
    }
}
