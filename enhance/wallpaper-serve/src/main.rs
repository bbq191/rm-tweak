//! wallpaper-serve —— 书架·壁纸（loopback 8793）+ 子命令。
//! `serve`：路由（经网关前缀 `/api/wallpapers`）`GET /`（池）· `POST /`（multipart 多图，缩放入池，`?activate=1` 顺手激活）·
//!   `PUT /current {name}` · `PUT /mode {mode}` · `DELETE /{name}` · `GET /{name}`（PNG 预览）· `GET /status`。
//! `enable` / `disable`：写 / 删 xochitl.conf 的 `SleepScreenPath`（安装器 / 卸载器 / 手动用）；`roll`：手动轮换；
//! `activate <name>`：命令行激活。**轮换由 serve 内的 journal 唤醒监听触发**（wake.rs）。
//! 休眠屏机制（2026-09-06 起）：原生隐藏键 `SleepScreenPath=current.png`（native.rs），xochitl 每次休眠重读该文件——
//! 不再 bind-mount `/usr/share/remarkable/suspended.png`、不再盖插画卡、不写 `/usr`、没有开机单元和 sleep 钩子。
mod native;
mod store;
mod wake;

use native::Native;
use rmsvc_core::asset::{self, AssetStore, AssetUploadFlow};
use rmsvc_core::http::{bind, ApiError, Reply, Router};
use rmsvc_core::paths::Paths;
use rmsvc_core::service::{self, ServiceSpec};
use std::sync::Arc;
use store::{Mode, WallpaperStore};

const SPEC: ServiceSpec = ServiceSpec {
    name: "wallpaper-serve",
    label: "壁纸",
    version: env!("CARGO_PKG_VERSION"),
    default_bind: "127.0.0.1:8793",
    tab: Some(("壁纸", 40)),
};

struct State {
    store: WallpaperStore,
    native: Native,
    paths: Paths,
    bus: Arc<rmsvc_core::events::EventBus>,
}

impl State {
    fn status(&self) -> serde_json::Value {
        let st = self.store.state();
        serde_json::json!({
            "ok": true, "mode": st.mode, "current": st.current, "pool": self.store.names().len(),
            "native": self.native.status(),
            "screen": {"width": store::W, "height": store::H},
        })
    }
    /// 激活一张并确保原生键就位（首次写键 → 需 `xovi/start` 一次才生效，状态里 `restartPending` 能看到）。
    fn activate(&self, name: &str) -> Result<bool, String> {
        self.store.activate(name)?;
        self.native.enable()
    }
}

fn enable_message(changed: bool) -> String {
    if changed {
        "已写入 xochitl.conf SleepScreenPath（首次生效需重启 xochitl 一次：/home/root/xovi/start）".into()
    } else {
        "SleepScreenPath 已就位".into()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = Paths::from_env();
    let _ = paths.ensure();
    let store = WallpaperStore::new(&paths);
    if let Err(e) = store.ensure() {
        eprintln!("[wallpaper-serve] 建目录失败: {e}");
        std::process::exit(1);
    }
    let native = Native::new(&paths, store.current_path());
    match args.first().map(|s| s.as_str()) {
        Some("enable") => {
            if !store.current_path().is_file() {
                exit_with(Err("还没有激活的壁纸（先上传并激活一张）".into()));
            }
            exit_with(native.enable().map(enable_message))
        }
        Some("disable") => exit_with(native.disable().map(|c| if c { "已删 SleepScreenPath，xochitl 重启后回原生休眠屏".to_string() } else { "本就没有 SleepScreenPath".to_string() })),
        Some("roll") => exit_with(store.roll().map(|n| n.map(|n| format!("轮换到 {n}")).unwrap_or_else(|| "不轮换（fixed 或空池）".into()))),
        Some("activate") => exit_with(args.get(1).ok_or("用法: activate <name>".to_string()).and_then(|n| store.activate(n).and_then(|_| native.enable()).map(|c| format!("已激活 {n}；{}", enable_message(c))))),
        Some("serve") | None => {}
        Some(x) => {
            eprintln!("未知子命令 {x}（serve|enable|disable|roll|activate）");
            std::process::exit(2);
        }
    }
    let bind_addr = service::parse_bind(&args, SPEC.default_bind);
    let bus = Arc::new(rmsvc_core::events::EventBus::new());
    let st = Arc::new(State { store, native, paths: paths.clone(), bus: bus.clone() });
    let router = Router::new()
        .get("/", bind(&st, |s, _| {
            let w = s.store.state();
            Ok(Reply::ok(&serde_json::json!({"items": s.store.list(), "mode": w.mode, "current": w.current})))
        }))
        .get("/status", bind(&st, |s, _| Ok(Reply::ok(&s.status()))))
        .get("/events", bind(&st, |s, _| Ok(s.bus.sse_reply())))
        .post("/", bind(&st, |s, r| {
            let b = r.multipart_boundary()?;
            // `?activate=1` 显式激活首张成功项；池里还没有当前图时也自动激活（上传即可用）。
            let want = r.q_flag("activate") || s.store.state().current.is_none();
            let items = AssetUploadFlow::new(&s.paths).run(&s.store, &mut *r.body, &b).map_err(ApiError::bad)?;
            let mut activated = None;
            let mut changed = false;
            if want {
                if let Some(first) = items.iter().find(|i| i.ok).and_then(|i| i.item.as_ref()) {
                    changed = s.activate(&first.name).map_err(ApiError::internal)?;
                    activated = Some(first.name.clone());
                }
            }
            let note = if changed || s.native.restart_pending() { "首次启用：跑一次 /home/root/xovi/start 后，下次休眠即显示" } else { "下次休眠即显示" };
            if items.iter().any(|i| i.ok) {
                s.bus.publish("wallpapers", "pool");
            }
            Ok(Reply::ok(&asset::receipt(&items, serde_json::json!({"activated": activated, "note": note, "restartPending": s.native.restart_pending()}))))
        }))
        .put("/current", bind(&st, |s, r| {
            let j = r.json()?;
            let name = j.str("name")?;
            s.activate(name).map_err(ApiError::bad)?;
            s.bus.publish("wallpapers", "pool");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "current": name, "native": s.native.status()})))
        }))
        .put("/mode", bind(&st, |s, r| {
            let j = r.json()?;
            let mode: Mode = serde_json::from_value(j.0.get("mode").cloned().unwrap_or_default()).map_err(|_| ApiError::bad("mode ∈ sequential|random|fixed"))?;
            s.store.set_mode(mode).map_err(ApiError::internal)?;
            s.bus.publish("wallpapers", "pool");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "mode": mode})))
        }))
        .delete("/{name}", bind(&st, |s, r| {
            s.store.remove(r.param("name")).map_err(ApiError::bad)?;
            s.bus.publish("wallpapers", "pool");
            Ok(Reply::ok(&serde_json::json!({"ok": true})))
        }))
        .get("/{name}", bind(&st, |s, r| Ok(Reply::bytes("image/png", s.store.read(r.param("name")).map_err(ApiError::not_found)?))));
    wake::spawn(Arc::new(WallpaperStore::new(&paths)), bus);
    println!("[wallpaper-serve] 池 {}，原生休眠屏键 {}；监听 xochitl 唤醒日志轮换", st.store.pool().display(), if st.native.enabled() { "已就位" } else { "未写（激活首张时自动写）" });
    if let Err(e) = service::run(&SPEC, &bind_addr, &paths, router) {
        eprintln!("[wallpaper-serve] {e}");
        std::process::exit(1);
    }
}

fn exit_with(r: Result<String, String>) -> ! {
    match r {
        Ok(m) => {
            println!("[wallpaper-serve] {m}");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("[wallpaper-serve] {e}");
            std::process::exit(1)
        }
    }
}
