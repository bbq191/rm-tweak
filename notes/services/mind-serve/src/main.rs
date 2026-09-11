//! mind-serve —— 笔记·脑（loopback 8797）。**按条目单发**（二期重新设计，取代首期"按分区批量跑"）：
//! 网页给某条目勾了「问AI」+ 填了问题，点提问 → 拼 `书名 + 章节 + 勾画原文 + 转写文本 + 问题` → 文字模型 →
//! 写回 `answer`（经 ink-serve 的 HTTP，条目库唯一写者仍是它）。**没有后台线程、没有事件订阅**——跟
//! transcribe-serve 不一样，这里没有"自动扫待处理条目"这回事，纯粹被动等 HTTP 请求，空闲零 CPU
//! （二期耗电评估第 5 点）。出网服务：设备自己的 WiFi 直连。
//! 路由（经网关前缀 `/api/mind`）：`GET /status` · `GET /config` · `PUT /config`（`apiKey` 只写不读）·
//! `POST /books/{uuid}/entries/{id}/ask`（回答这一条——要求条目已勾「问AI」且填了问题，否则 400）。
//! **第二轮整理区反馈（2026-09-08）**：模型预置表横跨四家厂商，`GET /status` 的 `usageByModel` 按模型
//! 分账、带用户自填单价算出的花费估算，见 `config.rs`/`ledger.rs` 模块文档。
mod backend;
mod config;
mod ink;
mod ledger;
mod prompt;
mod worker;

use backend::TextModel;
use config::MindConfig;
use ink::{EntryStore, InkHttp};
use ledger::Ledger;
use rmsvc_core::http::{bind, ApiError, Reply, Router};
use rmsvc_core::paths::Paths;
use rmsvc_core::service::{self, ServiceSpec};
use std::sync::Arc;
use std::time::Duration;
use vendorcfg::{ConfigCell, VendorConfig};
use worker::Ctx;

pub const APP: &str = "notes";

const SPEC: ServiceSpec = ServiceSpec { name: "mind-serve", label: "笔记·脑", version: env!("CARGO_PKG_VERSION"), default_bind: "127.0.0.1:8797", tab: None };

struct State {
    cfg: ConfigCell<MindConfig>,
    ledger: Ledger,
    store: InkHttp,
}

impl State {
    fn cfg(&self) -> MindConfig {
        self.cfg.get()
    }
    fn model(&self, cfg: &MindConfig) -> Result<Box<dyn TextModel>, String> {
        let c = vendorcfg::ChatClient::from_config(cfg, &cfg.backend, Duration::from_secs(cfg.timeout_secs), "未配置 API key（网页「模型」设置里粘贴，或环境变量 DASHSCOPE_API_KEY）")?;
        Ok(Box::new(c))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind_addr = service::parse_bind(&args, SPEC.default_bind);
    let paths = Paths::from_env();
    // `.migrate()`：老配置文件搬进新形状，不迁移会让真机已存的 key 在升级后凭空消失，见 config.rs 文档；
    // 读→迁移→0600→落盘一次这套启动流程收在 `ConfigCell::load`。
    let cfg = ConfigCell::load(&paths.app_config_dir(APP).join("mind.json"), MindConfig::migrate);
    let st = Arc::new(State { cfg, ledger: Ledger::open(&paths.app_state_dir(APP).join("mind.json")), store: InkHttp::new(paths.clone()) });
    let router = Router::new()
        .get("/status", bind(&st, |s, _| {
            let cfg = s.cfg();
            Ok(Reply::ok(&serde_json::json!({"config": cfg.public(), "usage": s.ledger.snapshot(), "usageByModel": vendorcfg::usage::usage_profile(&cfg, config::PRESETS, &s.ledger.snapshot())})))
        }))
        .get("/config", bind(&st, |s, _| Ok(Reply::ok(&s.cfg().public()))))
        .put("/config", bind(&st, |s, r| {
            let j = r.json()?;
            let next = s.cfg.update(|c| c.apply(&j.0)).map_err(ApiError::bad)?;
            Ok(Reply::ok(&next.public()))
        }))
        .post("/books/{uuid}/entries/{id}/ask", bind(&st, |s, r| {
            let (uuid, id) = (r.param("uuid").to_string(), r.param("id").to_string());
            let cfg = s.cfg();
            let model = s.model(&cfg).map_err(ApiError::bad)?;
            let book = s.store.book(&uuid).map_err(ApiError::not_found)?;
            let e = book.entries.iter().find(|e| e.id == id).ok_or_else(|| ApiError::not_found("没有这条目"))?;
            let now = rmsvc_core::clock::now_secs();
            let ctx = Ctx { store: &s.store, model: model.as_ref(), cfg: &cfg, ledger: &s.ledger, now };
            match worker::ask_entry(&ctx, &uuid, &book.title, e) {
                // 点「提问」弹出这次调用的消耗（token）——不是账本累计，是这一次调用的实际数字。
                Ok(a) => Ok(Reply::ok(&serde_json::json!({"ok": true, "answer": a.text, "promptTokens": a.prompt_tokens, "completionTokens": a.completion_tokens}))),
                Err(msg) => Err(ApiError::bad(msg)),
            }
        }));
    println!("[mind-serve] 配置 {}；后端 {} {}；key {:?}", st.cfg.path().display(), st.cfg().backend, st.cfg().model(), st.cfg().key_source());
    if let Err(e) = service::run(&SPEC, &bind_addr, &paths, router) {
        eprintln!("[mind-serve] {e}");
        std::process::exit(1);
    }
}
