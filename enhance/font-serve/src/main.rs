//! font-serve —— 书架·字体（loopback 8792）。路由（经网关前缀 `/api/fonts`）：
//! `GET /`（按家族归组的清单）· `POST /`（multipart 多文件，装进 fontconfig 用户字体目录；**不碰 KOReader**）·
//! `DELETE /{family}`（删整个家族的全部文件）· `GET /status`。所有字体一视同仁、无"内建"。
//! 字体菜单 qmd 读 `~/.local/share/shelf/fonts.json`（`shelf/xovi/font-menu-dynamic.qmd`）。
mod store;

use rmsvc_core::asset::{self, AssetStore, AssetUploadFlow};
use rmsvc_core::http::{bind, ApiError, Reply, Router};
use rmsvc_core::paths::Paths;
use rmsvc_core::service::{self, ServiceSpec};
use std::sync::Arc;
use store::{FontConfig, FontStore};

/// 网页 tab 叫「xochitl」：原生阅读器的字体在这里管（传书是网关固定页，不由本服务挂 tab）。
const SPEC: ServiceSpec = ServiceSpec {
    name: "font-serve",
    label: "字体",
    version: env!("CARGO_PKG_VERSION"),
    default_bind: "127.0.0.1:8792",
    tab: Some(("xochitl", 10)),
};

struct State {
    store: FontStore,
    paths: Paths,
    bus: Arc<rmsvc_core::events::EventBus>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind_addr = service::parse_bind(&args, SPEC.default_bind);
    let paths = Paths::from_env();
    let _ = paths.ensure();
    let store = FontStore::new(&paths, FontConfig::load(&paths));
    match store.write_index() {
        Ok(f) => println!("[font-serve] 索引 {} 个家族", f.len()),
        Err(e) => eprintln!("[font-serve] 写 fonts.json 失败: {e}"),
    }
    let st = Arc::new(State { store, paths: paths.clone(), bus: Arc::new(rmsvc_core::events::EventBus::new()) });
    let router = Router::new()
        .get("/", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.store.list(), "fontsDir": s.store.fonts_dir(), "index": s.store.json_path()})))))
        .post("/", bind(&st, |s, r| {
            let b = r.multipart_boundary()?;
            let items = AssetUploadFlow::new(&s.paths).run(&s.store, &mut *r.body, &b).map_err(ApiError::bad)?;
            // 真机 2026-09-03（3.27.3.0）：上传后不重启，菜单出现新项、选中即渲染。菜单 onVisibleChanged 差量刷新（S-B）。
            // fontconfig 回退由 write_index 随每次上传重写（weak 绑定：选的字体优先、缺字才回退）。
            let fallback = s.store.cjk_fallback_keys();
            let warns: Vec<String> = items.iter().filter_map(|i| i.item.as_ref().and_then(|it| it.extra.get("warn")).and_then(|w| w.as_str()).filter(|w| !w.is_empty()).map(|w| w.to_string())).collect();
            let mut note = String::from("已装进原生阅读器（fontconfig）；「文字与布局」菜单重开即可选，无需重启。KOReader 请到 KOReader 页单独上传");
            if !fallback.is_empty() {
                note.push_str(&format!("。中文缺字回退链：{}", fallback.join(" → ")));
            }
            if !warns.is_empty() {
                note.push_str(&format!("。⚠ {}", warns.join("；")));
            }
            if items.iter().any(|i| i.ok) {
                s.bus.publish("fonts", "fonts");
            }
            Ok(Reply::ok(&asset::receipt(&items, serde_json::json!({"restartNeeded": false, "fallback": fallback, "note": note}))))
        }))
        .get("/events", bind(&st, |s, _| Ok(s.bus.sse_reply())))
        .delete("/{family}", bind(&st, |s, r| {
            let removed = s.store.remove_family(r.param("family")).map_err(ApiError::bad)?;
            s.bus.publish("fonts", "fonts");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "removed": removed})))
        }))
        .put("/config", bind(&st, |s, r| {
            let on = r.json()?.0.get("emboldenCjkFallback").and_then(|x| x.as_bool()).ok_or_else(|| ApiError::bad("需要 {emboldenCjkFallback: bool}"))?;
            s.store.set_embolden(on).map_err(ApiError::internal)?;
            s.bus.publish("fonts", "config");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "emboldenCjkFallback": on, "note": "已更新，翻书即见（fontconfig 实时生效，无需重启）"})))
        }))
        .get("/status", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"ok": true, "count": s.store.list().len(), "target": "native", "cjkFallback": s.store.cjk_fallback_keys(), "emboldenCjkFallback": s.store.embolden()})))));
    println!("[font-serve] 字体目录 {}，清单 {}", st.store.fonts_dir().display(), st.store.json_path().display());
    if let Err(e) = service::run(&SPEC, &bind_addr, &paths, router) {
        eprintln!("[font-serve] {e}");
        std::process::exit(1);
    }
}
