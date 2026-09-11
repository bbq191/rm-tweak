//! HTTP 适配层（唯一碰 http 类型的地方，只做取参 + 调领域方法 + 回执），与 book-serve 的 `api.rs` 对称。
//! 路由清单见 `main.rs` 顶部文档。
use crate::annot;
use crate::koreader::{KoStore, KO_ANY};
use crate::service_state::State;
use crate::vocab;
use rmsvc_core::asset::AssetStore;
use rmsvc_core::formats::DICT_EXTS;
use rmsvc_core::fs::plain_name;
use rmsvc_core::http::{bind, ApiError, Reply, Router};
use std::sync::Arc;

pub fn router(st: Arc<State>) -> Router {
    Router::new()
        .get("/status", bind(&st, |s, _| Ok(Reply::ok(&s.status()))))
        .get("/events", bind(&st, |s, _| Ok(s.bus.sse_reply())))
        .get("/books", bind(&st, |s, r| {
            let folder = r.q("folder").unwrap_or("").trim_matches('/').to_string();
            let items = s.ko.list_books(&folder).map_err(ApiError::bad)?;
            Ok(Reply::ok(&serde_json::json!({"folder": folder, "items": items})))
        }))
        // 新建 KOReader 书目录（相对 books/，可多级）：界面"加入 KOReader → 新建文件夹"用。已存在也算成功（幂等）。
        .post("/books/mkdir", bind(&st, |s, r| {
            s.require_installed()?;
            let folder = r.json()?.str("folder")?.to_string();
            let dir = s.ko.subdir(&folder).map_err(ApiError::bad)?;
            if dir == s.ko.subdir("").map_err(ApiError::bad)? {
                return Err(ApiError::bad("文件夹名不能为空"));
            }
            std::fs::create_dir_all(&dir).map_err(|e| ApiError::internal(format!("建目录失败: {e}")))?;
            s.notify("books");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "folder": folder.trim().trim_matches('/')})))
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
            let item = KoStore::new(dest, "koreader-book", KO_ANY, "books/").copy_in(&name, &src).map_err(ApiError::bad)?;
            s.notify("books");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "message": format!("已加入 KOReader《{}》（{} 字节）", name, item.bytes), "note": s.ko.running_note("KOReader 运行中：在其文件浏览器刷新可见")})))
        }))
        .get("/fonts", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.fonts_json()})))))
        .post("/fonts", bind(&st, |s, r| { let rep = s.upload(r, &s.font_store())?; s.notify("fonts"); Ok(rep) }))
        .delete("/fonts/{file}", bind(&st, |s, r| {
            s.font_store().remove(r.param("file")).map_err(|e| ApiError::not_found(format!("删除失败: {e}")))?;
            s.notify("fonts");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "note": s.ko.running_note("KOReader 运行中：重启它后字体列表才更新")})))
        }))
        .get("/annotations", bind(&st, |s, _| {
            let items = annot::scan(s.ko.root(), &s.ko.books_dir(), &s.paths.runtime_dir().join("koreader")).map_err(ApiError::internal)?;
            Ok(Reply::ok(&serde_json::json!({"items": items})))
        }))
        .get("/vocabulary", bind(&st, |s, _| {
            let items = vocab::read(&s.ko.root().join("settings/vocabulary_builder.sqlite3")).map_err(ApiError::internal)?;
            Ok(Reply::ok(&serde_json::json!({"items": items})))
        }))
        .get("/dicts", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.ko.list_dicts()})))))
        .post("/dicts", bind(&st, |s, r| {
            let name = r.q("name").unwrap_or("").trim().to_string();
            plain_name(&name).map_err(|_| ApiError::bad("需要 ?name=<词典目录名>（单层）"))?;
            let rep = s.upload(r, &KoStore::new(s.ko.dict_dir().join(&name), "koreader-dict", DICT_EXTS, format!("词典 {name}/")))?;
            s.notify("dicts");
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
                s.notify("config");
            }
            Ok(Reply::ok(&res))
        }))
}
