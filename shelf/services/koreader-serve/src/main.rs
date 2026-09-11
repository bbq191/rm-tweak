//! koreader-serve —— 书架·KOReader（loopback 8791）。
//! 路由（经网关前缀 `/api/koreader`）：`GET /status` · `GET /books[?folder=]` · `POST /books/adopt {name, folder}`（从母版库落库）·
//! `GET /fonts` · `POST /fonts` · `DELETE /fonts/{file}` · `GET /dicts` · `POST /dicts?name=` ·
//! `GET /config/{settings|defaults|gestures}`（原文）· `POST /config/{file}?dry_run=1`（body=补丁 Lua；运行中拒写）。
//! 书只从母版库来（2026-09-05 规则：所有书先落母版库，落库＝纯复制原字节，不优化），本服务不再收直传书。
mod config;
mod koreader;

use config::ConfigSync;
use koreader::{KoReader, KoStore, KO_ANY};
use rmsvc_core::asset::{self, AssetStore, AssetUploadFlow};
use rmsvc_core::formats::{DICT_EXTS, FONT_EXTS};
use rmsvc_core::fs::plain_name;
use rmsvc_core::http::{bind, ApiError, ApiResult, Reply, Request, Router};
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

struct State {
    ko: Arc<KoReader>,
    paths: Paths,
    sync: ConfigSync,
    bus: Arc<rmsvc_core::events::EventBus>,
}

impl State {
    fn require_installed(&self) -> Result<(), ApiError> {
        if self.ko.installed() {
            Ok(())
        } else {
            Err(ApiError { status: 409, message: "KOReader 未安装（appload 目录不存在）".into() })
        }
    }

    fn status(&self) -> serde_json::Value {
        let k = &self.ko;
        serde_json::json!({
            "ok": true,
            "installed": k.installed(),
            "running": k.running(),
            "version": k.version(),
            "root": k.root(),
            "booksDir": k.books_dir(),
            "books": k.list_books("").map(|v| v.iter().filter(|e| e.kind == "file").count()).unwrap_or(0),
            "fonts": koreader::list_files(&k.fonts_dir(), FONT_EXTS).len(),
            "dicts": k.list_dicts().len(),
        })
    }

    /// multipart 多文件 → `store`（fonts/dicts 共用同一 [`AssetUploadFlow`]）。KOReader 未装→409。
    fn upload(&self, r: &mut Request<'_>, store: &KoStore) -> ApiResult {
        self.require_installed()?;
        let boundary = r.multipart_boundary()?;
        let items = AssetUploadFlow::new(&self.paths).run(store, &mut *r.body, &boundary).map_err(ApiError::bad)?;
        Ok(Reply::ok(&asset::receipt(&items, serde_json::json!({"note": self.ko.running_note("KOReader 正在运行：重启它后才生效")}))))
    }

    fn font_store(&self) -> KoStore {
        KoStore::new(self.ko.fonts_dir(), "koreader-font", FONT_EXTS, "fonts/")
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind_addr = service::parse_bind(&args, SPEC.default_bind);
    let paths = Paths::from_env();
    let ko = Arc::new(KoReader::new(paths.koreader_root()));
    let st = Arc::new(State {
        sync: ConfigSync { ko: ko.clone(), backup_dir: paths.state_dir().join("koreader-backups"), tmp_dir: paths.runtime_dir().join("koreader") },
        ko,
        paths: paths.clone(),
        bus: Arc::new(rmsvc_core::events::EventBus::new()),
    });
    let router = Router::new()
        .get("/status", bind(&st, |s, _| Ok(Reply::ok(&s.status()))))
        .get("/events", bind(&st, |s, _| Ok(s.bus.sse_reply())))
        .get("/books", bind(&st, |s, r| {
            let folder = r.q("folder").unwrap_or("").trim_matches('/').to_string();
            let items = s.ko.list_books(&folder).map_err(ApiError::bad)?;
            Ok(Reply::ok(&serde_json::json!({"folder": folder, "items": items})))
        }))
        // 从母版库（book-serve 的 staging/，共享目录）adopt 一本书到 KOReader——落库=纯复制母版字节，不优化
        // （优化是母版库的独立动作；两读器落同一字节才能对照）。前端从 /api/books/staging 列表选书后调这里。
        .post("/books/adopt", bind(&st, |s, r| {
            s.require_installed()?;
            let j = r.json()?;
            let name = plain_name(j.str("name")?).map_err(ApiError::bad)?.to_string();
            let src = s.paths.staging_dir().join(&name);
            if !src.is_file() {
                return Err(ApiError::not_found("母版库里没有这本书"));
            }
            let dest = s.ko.subdir(j.str_or("folder", "")).map_err(ApiError::bad)?;
            let item = KoStore::new(dest, "koreader-book", KO_ANY, "books/").install(&name, &src).map_err(ApiError::bad)?;
            s.bus.publish("koreader", "books");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "message": format!("已加入 KOReader《{}》（{} 字节）", name, item.bytes), "note": s.ko.running_note("KOReader 运行中：在其文件浏览器刷新可见")})))
        }))
        .get("/fonts", bind(&st, |s, _| {
            let dir = s.ko.fonts_dir();
            let items: Vec<serde_json::Value> = koreader::list_files(&dir, FONT_EXTS).into_iter().map(|it| {
                // 中文基本区覆盖率（同原生字体一致的判据），低覆盖当正文会缺字
                let pct = std::fs::read(dir.join(&it.name)).ok().and_then(|b| rmsvc_core::ttf::han_coverage_pct(&b)).unwrap_or(0);
                serde_json::json!({"name": it.name, "bytes": it.bytes, "cjkPct": pct})
            }).collect();
            Ok(Reply::ok(&serde_json::json!({"items": items})))
        }))
        .post("/fonts", bind(&st, |s, r| { let rep = s.upload(r, &s.font_store())?; s.bus.publish("koreader", "fonts"); Ok(rep) }))
        .delete("/fonts/{file}", bind(&st, |s, r| {
            s.font_store().remove(r.param("file")).map_err(|e| ApiError::not_found(format!("删除失败: {e}")))?;
            s.bus.publish("koreader", "fonts");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "note": s.ko.running_note("KOReader 运行中：重启它后字体列表才更新")})))
        }))
        .get("/dicts", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.ko.list_dicts()})))))
        .post("/dicts", bind(&st, |s, r| {
            let name = r.q("name").unwrap_or("").trim().to_string();
            plain_name(&name).map_err(|_| ApiError::bad("需要 ?name=<词典目录名>（单层）"))?;
            let rep = s.upload(r, &KoStore::new(s.ko.dict_dir().join(&name), "koreader-dict", DICT_EXTS, format!("词典 {name}/")))?;
            s.bus.publish("koreader", "dicts");
            Ok(rep)
        }))
        .get("/config/{file}", bind(&st, |s, r| {
            let text = s.sync.read(r.param("file")).map_err(ApiError::bad)?;
            Ok(Reply::bytes("text/plain; charset=utf-8", text.into_bytes()))
        }))
        .post("/config/{file}", bind(&st, |s, r| {
            let file = r.param("file").to_string();
            let dry = r.q_flag("dry_run");
            let patch = String::from_utf8(r.read_small_body().map_err(ApiError::bad)?).map_err(|_| ApiError::bad("补丁不是 UTF-8"))?;
            let res = s.sync.apply(&file, &patch, dry).map_err(|e| ApiError { status: if e.contains("正在运行") { 409 } else { 400 }, message: e })?;
            if !dry {
                s.bus.publish("koreader", "config");
            }
            Ok(Reply::ok(&res))
        }));
    println!("[koreader-serve] root={} installed={}", st.ko.root().display(), st.ko.installed());
    if let Err(e) = service::run(&SPEC, &bind_addr, &paths, router) {
        eprintln!("[koreader-serve] {e}");
        std::process::exit(1);
    }
}
