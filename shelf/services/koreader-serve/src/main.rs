//! koreader-serve —— 书架·KOReader（loopback 8791）。
//! 路由（经网关前缀 `/api/koreader`）：`GET /status` · `GET /books[?folder=]` · `POST /books/adopt {name, folder}`（从母版库落库）·
//! `GET /fonts` · `POST /fonts` · `DELETE /fonts/{file}` · `GET /dicts` · `POST /dicts?name=` ·
//! `GET /config/{settings|defaults|gestures|directory|profiles}`（原文）· `POST /config/{file}?dry_run=1`（body=补丁 Lua；运行中拒写）·
//! `GET /annotations`（`books/` 下每本书的高亮标注，读 `<book>.sdr/metadata.*.lua`，见 `annot.rs`）·
//! `GET /vocabulary`（生词本插件数据库 `settings/vocabulary_builder.sqlite3`，见 `vocab.rs`；2026-09-16
//! 真机核对过路径——第一版想当然写成 `data/`，实际在 `settings/`，跟其它 sqlite 状态文件同目录）——两个都是
//! 笔记线 ink-serve 拉去回流成条目用的原始数据端点，见笔记线白皮书 §03al；本服务只读，不碰条目库。
//! 书只从母版库来（2026-09-05 规则：所有书先落母版库，落库＝纯复制原字节，不优化），本服务不再收直传书。
mod annot;
mod api;
mod config;
mod koreader;
mod service_state;
mod sqlite_min;
mod vocab;

use rmsvc_core::paths::Paths;
use rmsvc_core::service::{self, ServiceSpec};
use std::sync::Arc;

const SPEC: ServiceSpec = ServiceSpec {
    name: "koreader-serve",
    label: "KOReader",
    version: env!("CARGO_PKG_VERSION"),
    default_bind: "127.0.0.1:8791",
    tab: Some(("KOReader", 20)),
};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind_addr = service::parse_bind(&args, SPEC.default_bind);
    let paths = Paths::from_env();
    let st = Arc::new(service_state::State::new(&paths));
    let n = st.clean_upload_dir();
    if n > 0 {
        println!("[koreader-serve] 清掉 {n} 个上次未完成的上传暂存");
    }
    let router = api::router(st.clone());
    println!("[koreader-serve] root={} installed={}", st.ko.root().display(), st.ko.installed());
    if let Err(e) = service::run(&SPEC, &bind_addr, &paths, router) {
        eprintln!("[koreader-serve] {e}");
        std::process::exit(1);
    }
}
