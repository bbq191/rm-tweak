//! HTTP 适配层（唯一碰 http 类型的地方，只做取参 + 调领域方法 + 回执）。路由（经网关时前缀 `/api/books`）：
//! `GET /status` · `GET /inbox` · `POST /inbox/retry {name}` · `POST /inbox/delete {name}`
//! 母版库：`GET /staging` → `{items, freeBytes}` · `POST /staging`（multipart，原样入库）· `POST /staging/optimize {name, mode}`
//! · `POST /staging/deliver {name, folder?, keep?}` · `POST /staging/mark {name, target}` · `POST /staging/fetch-article {url, optimize?}`
//! · `POST /staging/delete {name}` · `GET /staging/render/{uuid}`（xochitl 渲染缓存 PDF，doctor --render 用）· `GET /events`（SSE：母版库/inbox 变更即推，网页零轮询）。
//! 原生回收站队列：`POST /trash/add {uuid, name}`（name 必须与书库 visibleName 相符）· `GET /trash/pending` → `{uuids}`（Sidebar 代理 qmd 拉取执行）· `GET /trash`。
//! 2026-09-05 起规则统一"所有书只落母版库"：旧 `POST /?target=` 直投路已删（`/staging*` 是唯一入口）。
use crate::service_state::State;
use crate::staging::{OptimizeMode, Reader, StagingStore};
use rmsvc_core::asset::{self, AssetUploadFlow};
use rmsvc_core::http::{bind, ApiError, ApiResult, Reply, Request, Router};
use std::sync::Arc;

pub fn router(st: Arc<State>) -> Router {
    Router::new()
        .get("/status", bind(&st, |s, _| Ok(Reply::ok(&s.status()))))
        .get("/inbox", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.spool.list()})))))
        .post("/inbox/retry", bind(&st, |s, r| {
            let name = r.json()?.str("name")?.to_string();
            s.spool.retry(&name).map_err(ApiError::bad)?;
            Ok(Reply::ok(&serde_json::json!({"ok": true, "items": s.process_inbox(Some(&name))})))
        }))
        .post("/inbox/delete", bind(&st, |s, r| {
            s.spool.delete_failed(r.json()?.str("name")?).map_err(ApiError::bad)?;
            s.bus.publish("books", "inbox");
            ok()
        }))
        .get("/events", bind(&st, |s, _| Ok(s.bus.sse_reply())))
        // ── 母版库（中间层）：入库 / 优化 / 落库 / 删除各自正交 ──
        .get("/staging", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.staging.list(), "freeBytes": s.staging.free_bytes()})))))
        .post("/staging", bind(&st, staging_upload))
        .post("/staging/optimize", bind(&st, |s, r| {
            let j = r.json()?;
            let msg = s.staging.optimize(j.str("name")?, OptimizeMode::parse(j.str_or("mode", "auto"))).map_err(ApiError::bad)?;
            s.bus.publish("books", "staging");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "message": msg})))
        }))
        .post("/staging/deliver", bind(&st, |s, r| {
            let j = r.json()?;
            // 母版库默认保留（可再投另一读器对照）；folder 空＝配置缺省。
            let out = s.staging.deliver(j.str("name")?, j.str_or("folder", ""), j.bool_or("keep", true)).map_err(ApiError::bad)?;
            if let Some(plan) = out.render {
                s.spawn_render_check(plan); // EPUB：等 xochitl 渲染完核对页数（结果写边车 + 推 books/render）
            }
            s.bus.publish("books", "staging");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "message": out.message})))
        }))
        // 落库记录：KOReader adopt 在 koreader-serve 完成后由前端调这里记一笔（各服务只写自己的目录）。
        .post("/staging/mark", bind(&st, |s, r| {
            let j = r.json()?;
            let reader = Reader::parse(j.str("target")?).map_err(ApiError::bad)?;
            s.staging.mark_delivered(j.str("name")?, reader).map_err(ApiError::bad)?;
            s.bus.publish("books", "staging");
            ok()
        }))
        .post("/staging/fetch-article", bind(&st, |s, r| {
            let j = r.json()?;
            let out = s.staging.fetch_article(j.str("url")?, j.bool_or("optimize", false)).map_err(ApiError::bad)?;
            s.bus.publish("books", "staging");
            let message = match &out.optimize_error {
                Some(e) => format!("已抓取《{}》入母版库（同步优化失败：{e}，可在列表里手动点「优化」）", out.title),
                None if out.optimized => format!("已抓取《{}》入母版库并同步优化", out.title),
                None => format!("已抓取《{}》入母版库", out.title),
            };
            Ok(Reply::ok(&serde_json::json!({"ok": true, "name": out.name, "title": out.title, "message": message})))
        }))
        .get("/staging/render/{uuid}", bind(&st, |s, r| {
            let pdf = s.staging.render_pdf(r.param("uuid")).map_err(ApiError::not_found)?;
            Ok(Reply::bytes("application/pdf", pdf))
        }))
        // ── 原生书库回收站队列（真正的软删由 xochitl 自己的 selectionMoveToTrash 执行，见 trash.rs / shelf-trash-agent.qmd）──
        .post("/trash/add", bind(&st, |s, r| {
            let j = r.json()?;
            let n = s.trash.add(j.str("uuid")?, j.str("name")?).map_err(ApiError::bad)?;
            s.bus.publish("books", "trash");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "pending": n, "message": "已排队：书库视图下次有动静时移进回收站"})))
        }))
        .get("/trash/pending", bind(&st, |s, _| {
            let (uuids, pruned) = s.trash.pending().map_err(ApiError::internal)?;
            if pruned > 0 {
                s.bus.publish("books", "trash");
            }
            Ok(Reply::ok(&serde_json::json!({"uuids": uuids})))
        }))
        .get("/trash", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.trash.list()})))))
        .post("/staging/delete", bind(&st, |s, r| {
            s.staging.remove(r.json()?.str("name")?).map_err(ApiError::bad)?;
            s.bus.publish("books", "staging");
            ok()
        }))
}

fn ok() -> ApiResult {
    Ok(Reply::ok(&serde_json::json!({"ok": true})))
}

/// multipart 逐文件原样落母版库（不优化、不落库）：走共享上传模板，暂存在 spool `.work/`（与母版库同分区，入库 rename）。
/// 可选 `?srcName=&srcBytes=`（CLI push 洗书产物才带）：记这份产物的原始输入身份到 sidecar，供下次
/// push 同一份原始文件时**处理前**就能查到已经处理过，见 `sidecar::SourceRef` 文档。
fn staging_upload(st: &State, r: &mut Request<'_>) -> ApiResult {
    let boundary = r.multipart_boundary()?;
    let source = match (r.q("srcName"), r.q("srcBytes").and_then(|v| v.parse::<u64>().ok())) {
        (Some(name), Some(bytes)) => Some(crate::sidecar::SourceRef { name: name.to_string(), bytes }),
        _ => None,
    };
    let _g = st.spool.guard();
    let items = AssetUploadFlow::in_dir(st.spool.work()).run(&StagingStore(&st.staging), &mut *r.body, &boundary).map_err(ApiError::bad)?;
    if let Some(src) = &source {
        // 一次 staging_upload 请求实际上永远只有一个文件部分（CLI/网页都逐文件各发一个 POST），
        // 但这里不假设，多个成功项就都记同一个来源——理论上不会发生，发生了也无害（都是同一份
        // 原始输入触发的上传）。
        for it in items.iter().filter(|i| i.ok) {
            let _ = st.staging.set_source(&it.name, src.clone());
        }
    }
    if items.iter().any(|i| i.ok) {
        st.bus.publish("books", "staging");
    }
    Ok(Reply::ok(&asset::receipt(&items, serde_json::Value::Null)))
}
