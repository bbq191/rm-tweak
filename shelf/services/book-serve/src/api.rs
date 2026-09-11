//! HTTP 适配层（唯一碰 http 类型的地方，只做取参 + 调领域方法 + 回执）。路由（经网关时前缀 `/api/books`）：
//! `GET /status`
//! 母版库：`GET /staging` → `{items, freeBytes}` · `POST /staging`（multipart，原样入库）· `POST /staging/optimize {name}`
//! （2026-09-19 起不再分档位，只有一种"清洗+优化"行为）
//! · `POST /staging/deliver {name, folder?}`（2026-09-19 起投完永远保留母版，不再有 `keep` 参数）· `POST /staging/mark {name, target}` · `POST /staging/fetch-article {url, optimize?}`
//! · `POST /staging/delete {name}` · `POST /staging/rename {name, newName}` · `GET /staging/file?name=`（原件下载，流式）
//! · `POST /staging/direction {names|name, direction}`（按书阅读方向 auto/rtl/ltr，2026-09-25）
//! · 原 PDF 备份：`GET /staging` 的 `originals` · `POST /staging/originals/restore {name}` · `POST /staging/originals/delete {name}`
//! · `GET /events`（SSE：母版库/inbox 变更即推，网页零轮询）。
//! 阅读方向：`GET /reading-direction/{uuid}` → `{rtl}`（xochitl 里 reader-page-turn.qmd 用）。
//! 原生回收站队列：`POST /trash/add {uuid, name}`（name 必须与书库 visibleName 相符）· `GET /trash/pending?wait=` → `{uuids}`（MainView 代理 qmd 长轮询拉取执行）· `GET /trash`。
//! 原生建文件夹队列：`POST /mkdir/add {name}` · `GET /mkdir/pending` → `{names}`（MainView 代理 shelf-mkdir-agent.qmd 拉取执行）· `GET /mkdir`。
//! 代理放弃记录：`GET /agent-failures` → `{items:[{kind,name,uuid?,at}]}` · `POST /agent-failures/clear`（两个队列交满次数仍没做成的项）。
//! 2026-09-05 起规则统一"所有书只落母版库"：旧 `POST /?target=` 直投路已删（`/staging*` 是唯一入口）。
use crate::service_state::State;
use crate::staging::{Reader, StagingStore};
use rmsvc_core::asset::{self, AssetUploadFlow};
use rmsvc_core::http::{bind, ApiError, ApiResult, Reply, Request, Router};
use std::sync::Arc;

/// `GET /mkdir/pending?wait=`、`GET /trash/pending?wait=` 长轮询等待时长上限（秒）。QML 端（shelf-mkdir-agent.qmd）发 wait=290：设备 Qt 6.10
/// 的 QML XHR 不设传输超时（2026-09-24 核实，见 qmd 头注；09-22 版按"缺省 30s 超时"的假设把这里定成 28）。
/// 在等的这段时间服务端不读 socket，所以不受 rmsvc-core 的读空闲超时影响。
const AGENT_WAIT_MAX_SECS: u64 = 300;

pub fn router(st: Arc<State>) -> Router {
    Router::new()
        .get("/status", bind(&st, |s, _| Ok(Reply::ok(&s.status()))))
        .get("/events", bind(&st, |s, _| Ok(s.bus.sse_reply())))
        // ── 母版库（中间层）：入库 / 优化 / 落库 / 删除各自正交 ──
        .get("/staging", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.staging.list(), "freeBytes": s.staging.free_bytes(), "originals": s.staging.list_originals()})))))
        // 原件下载：边读边发（大书上百 MB，不整本读进内存）；网关见到 Content-Disposition 也原样流式转发。
        .get("/staging/file", bind(&st, |s, r| {
            let name = r.q("name").ok_or_else(|| ApiError::bad("缺少 name"))?.to_string();
            let (f, len) = s.staging.open_for_download(&name).map_err(ApiError::bad)?;
            let ctype = bookconv::convert::direct_content_type(&name).map(|c| c.mime()).unwrap_or("application/octet-stream");
            Ok(Reply::sized_stream(ctype, Box::new(std::io::BufReader::new(f)), len).with_header("Content-Disposition", &rmsvc_core::multipart::content_disposition(&name)))
        }))
        .post("/staging/rename", bind(&st, |s, r| {
            let j = r.json()?;
            let new_name = s.staging.rename(j.str("name")?, j.str("newName")?).map_err(ApiError::bad)?;
            s.bus.publish("books", "staging");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "name": new_name})))
        }))
        .post("/staging/direction", bind(&st, set_direction))
        // ── 原 PDF 备份（PDF→EPUB 后保留 7 天，见 Staging::backup_pdf_original）──
        .post("/staging/originals/restore", bind(&st, |s, r| {
            s.staging.restore_original(r.json()?.str("name")?).map_err(ApiError::bad)?;
            staging_changed(s)
        }))
        .post("/staging/originals/delete", bind(&st, |s, r| {
            s.staging.delete_original(r.json()?.str("name")?).map_err(ApiError::bad)?;
            staging_changed(s)
        }))
        .post("/staging", bind(&st, staging_upload))
        .post("/staging/optimize", bind(&st, |s, r| {
            // 异步：耗时的优化（真机实测大漫画能跑到分钟级，见书架白皮书 §05）挪到后台线程，这里立即
            // 回"已开始"；真正结果通过 books/staging 事件 + GET /staging 列表里的 delivered.optimize 呈现。
            let j = r.json()?;
            let name = j.str("name")?.to_string();
            s.staging.spawn_optimize(&name, s.bus.clone()).map_err(ApiError::bad)?;
            s.bus.publish("books", "staging"); // 立即推一次，UI 马上看到这条目进入 busy 状态
            Ok(Reply::ok(&serde_json::json!({"ok": true, "message": format!("《{name}》已开始优化，完成后自动刷新"), "async": true})))
        }))
        .post("/staging/deliver", bind(&st, |s, r| {
            // 异步：耗时的落库（超限漫画按卷拆分要挨个建包+上传，真机能到分钟级）挪到后台线程，这里
            // 立即回"已开始"；真正结果通过 books/staging 事件 + GET /staging 列表里的 delivered.deliver
            // 呈现（渲染自检、mark_delivered 都在线程内部完成，见 spawn_deliver）。
            let j = r.json()?;
            let name = j.str("name")?.to_string();
            // 母版库永远保留（可再投另一读器对照，2026-09-19 起不再有"投完自动删除"这条路）；
            // folder 空＝配置缺省；非空且真不存在会先经 mkdir 队列建出来再投，见 ensure_folder。
            s.staging.spawn_deliver(&name, j.str_or("folder", ""), s.mkdir.clone(), s.bus.clone()).map_err(ApiError::bad)?;
            s.bus.publish("books", "staging"); // 立即推一次，UI 马上看到这条目进入 busy 状态
            Ok(Reply::ok(&serde_json::json!({"ok": true, "message": format!("《{name}》已开始投递，完成后自动刷新"), "async": true})))
        }))
        // 落库记录：KOReader adopt 在 koreader-serve 完成后由前端调这里记一笔（各服务只写自己的目录）。
        .post("/staging/mark", bind(&st, |s, r| {
            let j = r.json()?;
            let reader = Reader::parse(j.str("target")?).map_err(ApiError::bad)?;
            s.staging.mark_delivered(j.str("name")?, reader).map_err(ApiError::bad)?;
            staging_changed(s)
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
        // ── 阅读方向（reader-page-turn.qmd 打开书时查；rtl=从右往左翻页的书，见 reading_direction.rs）──
        .get("/reading-direction/{uuid}", bind(&st, |s, r| {
            let rtl = s.reading_direction.is_rtl(r.param("uuid")).map_err(ApiError::bad)?;
            Ok(Reply::ok(&serde_json::json!({"rtl": rtl})))
        }))
        // ── 漫画页边距待办（QML 代理 shelf-comic-margins.qmd 在书打开时查；见 comic_margins.rs）──
        .get("/margins/{uuid}", bind(&st, |s, r| match s.comic_margins.get(r.param("uuid")) {
            Some(m) => Ok(Reply::ok(&serde_json::json!({"margins": m}))),
            None => Err(ApiError::not_found("没有待设的页边距")),
        }))
        .post("/margins/applied", bind(&st, |s, r| {
            let n = s.comic_margins.applied(r.json()?.str("uuid")?).map_err(ApiError::bad)?;
            Ok(Reply::ok(&serde_json::json!({"ok": true, "pending": n})))
        }))
        // ── 原生书库回收站队列（真正的软删由 xochitl 自己的 selectionMoveToTrash 执行，见 trash.rs / shelf-trash-agent.qmd）──
        .post("/trash/add", bind(&st, |s, r| {
            let j = r.json()?;
            let n = s.trash.add(j.str("uuid")?, j.str("name")?).map_err(ApiError::bad)?;
            s.bus.publish("books", "trash");
            Ok(Reply::ok(&serde_json::json!({"ok": true, "pending": n, "message": "已排队：书库视图下次有动静时移进回收站"})))
        }))
        .get("/trash/pending", bind(&st, |s, r| {
            let wait = r.q("wait").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(AGENT_WAIT_MAX_SECS);
            let (uuids, pruned) = s.trash.pending_wait(std::time::Duration::from_secs(wait)).map_err(ApiError::internal)?;
            if pruned > 0 {
                s.bus.publish("books", "trash");
            }
            Ok(Reply::ok(&serde_json::json!({"uuids": uuids})))
        }))
        .get("/trash", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.trash.list()})))))
        // ── 原生书库建文件夹队列（真正的建夹由 xochitl 自己的 Library.createCollection 执行，见 mkdir.rs / shelf-mkdir-agent.qmd）──
        .post("/mkdir/add", bind(&st, |s, r| {
            let n = s.mkdir.add(r.json()?.str("name")?).map_err(ApiError::bad)?;
            s.invalidate_status(); // 文件夹候选可能变了
            if n > 0 {
                s.bus.publish("books", "mkdir");
            }
            Ok(Reply::ok(&serde_json::json!({"ok": true, "pending": n})))
        }))
        // `?wait=<秒>` 长轮询（上限 [`AGENT_WAIT_MAX_SECS`]）：有待办立即回，否则阻塞到入队或到期回空；缺省 0＝立即返回。
        .get("/mkdir/pending", bind(&st, |s, r| {
            let wait = r.q("wait").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0).min(AGENT_WAIT_MAX_SECS);
            let (names, pruned) = s.mkdir.pending_wait(std::time::Duration::from_secs(wait)).map_err(ApiError::internal)?;
            if pruned > 0 {
                s.bus.publish("books", "mkdir");
            }
            Ok(Reply::ok(&serde_json::json!({"names": names})))
        }))
        .get("/mkdir", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.mkdir.list()})))))
        // ── 代理执行不成、已放弃的记录（网页页头横幅；「知道了」→ clear），见 agent_failures.rs ──
        .get("/agent-failures", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"items": s.agent_failures.list()})))))
        .post("/agent-failures/clear", bind(&st, |s, _| {
            let n = s.agent_failures.clear().map_err(ApiError::internal)?;
            Ok(Reply::ok(&serde_json::json!({"ok": true, "cleared": n})))
        }))
        // 停止正在跑的优化/投递（2026-09-20）：登记取消标记，在下一个安全检查点停下；无法中途停的步骤如实回 cancelled:false。
        .post("/staging/cancel", bind(&st, |s, r| {
            let name = r.json()?.str("name")?.to_string();
            let supported = s.staging.request_cancel(&name).map_err(ApiError::bad)?;
            let message = if supported { "已请求停止，会在当前这一小步结束后停下" } else { "这一步无法中途停止（单文件上传中），会自然跑完" };
            Ok(Reply::ok(&serde_json::json!({"ok": true, "cancelled": supported, "message": message})))
        }))
        .post("/staging/delete", bind(&st, |s, r| {
            s.staging.remove(r.json()?.str("name")?).map_err(ApiError::bad)?;
            staging_changed(s)
        }))
}

/// `POST /staging/direction {names: [...] | name, direction: "auto"|"rtl"|"ltr"}`：按书设阅读方向（可多本）。只存设置，
/// 书本身等下次「优化」才改；已加入过 xochitl 的顺手同步手动清单。回 `{ok, updated, stale, synced, failed:[{name,message}], message}`，
/// 全部失败才回 400。
fn set_direction(st: &State, r: &mut Request<'_>) -> ApiResult {
    let j = r.json()?;
    let raw = j.str("direction")?;
    let dir = match raw {
        "auto" => None,
        _ => Some(bookconv::direction::PageDirection::parse(raw).ok_or_else(|| ApiError::bad("direction 只能是 auto / rtl / ltr"))?),
    };
    let names: Vec<String> = match j.0.get("names").and_then(|v| v.as_array()) {
        Some(a) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
        None => vec![j.str("name")?.to_string()],
    };
    if names.is_empty() {
        return Err(ApiError::bad("缺 names"));
    }
    let (mut updated, mut stale, mut synced) = (0usize, 0usize, 0usize);
    let mut failed = Vec::new();
    for name in &names {
        match st.staging.set_direction(name, dir) {
            Ok(o) => {
                updated += 1;
                stale += usize::from(o.stale);
                synced += usize::from(o.synced.is_some());
                if let Some(e) = o.sync_error {
                    failed.push(serde_json::json!({"name": name, "message": format!("设置已保存，但同步到已加入 xochitl 的那份失败：{e}")}));
                }
            }
            Err(e) => failed.push(serde_json::json!({"name": name, "message": e})),
        }
    }
    if updated == 0 {
        let first = failed.first().and_then(|f| f["message"].as_str()).unwrap_or("没有可设置的书").to_string();
        return Err(ApiError::bad(first));
    }
    st.bus.publish("books", "staging");
    Ok(Reply::ok(&serde_json::json!({"ok": true, "updated": updated, "stale": stale, "synced": synced, "failed": failed})))
}

/// 母版库变更类操作的统一收尾：推一条 `books/staging`（网页据此重拉列表，零轮询）再回 `{ok:true}`。
fn staging_changed(s: &State) -> ApiResult {
    s.bus.publish("books", "staging");
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
    // 不持 spool 锁：暂存名是随机的 `.<uuid>.book.part`，与 inbox 追平互不相干；真正会撞的"挑名 + 落地"
    // 由 `Staging` 内部的落名临界区串行化（见 `staging::Staging` 的 `land` 字段）。
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

#[cfg(test)]
mod tests {
    //! 进程内路由测试：不起 socket，直接 `Router::dispatch`——覆盖参数解析、错误码映射与忙锁冲突（这些以前只能上真机验证）。
    use super::*;
    use rmsvc_core::http::{parse_query, Method};
    use rmsvc_core::paths::Paths;
    use std::collections::HashMap;

    fn state(t: &tempfile::TempDir) -> Arc<State> {
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        let cfg = paths.service_config("book");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, r#"{"xochitlHost":"127.0.0.1:9"}"#).unwrap(); // 关闭端口：连接秒拒，不真等超时
        let mut st = State::new(&paths);
        st.inbox_settle = std::time::Duration::ZERO; // 测试里刚写的 inbox 文件也立即处理
        st.ensure_dirs().unwrap();
        Arc::new(st)
    }

    fn call(router: &Router, m: Method, path: &str, body: &str) -> (u16, serde_json::Value) {
        let mut b = body.as_bytes();
        let mut r = Request { method: m, path: path.into(), query: parse_query(""), params: HashMap::new(), content_type: "application/json".into(), content_length: Some(body.len()), headers: vec![], body: &mut b };
        let rep = router.dispatch(&mut r);
        (rep.status, serde_json::from_slice(&rep.body).unwrap_or(serde_json::Value::Null))
    }

    fn msg(v: &serde_json::Value) -> String {
        v["message"].as_str().unwrap_or("").to_string()
    }

    #[test]
    fn margins_endpoint_returns_pending_then_404_after_applied() {
        const U: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
        let t = tempfile::tempdir().unwrap();
        let st = state(&t);
        let router = router(st.clone());
        let lib = Paths::resolve({
            let h = t.path().to_str().unwrap().to_string();
            move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None }
        })
        .xochitl_dir();
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::write(lib.join(format!("{U}.metadata")), "{}").unwrap();
        let path = format!("/margins/{U}");
        assert_eq!(call(&router, Method::Get, &path, "").0, 404, "没登记 → 404，QML 代理静默不动");
        let qol = Paths::resolve({
            let h = t.path().to_str().unwrap().to_string();
            move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None }
        })
        .home()
        .join(".local/share/cangjie-ime/reading-qol.json");
        std::fs::create_dir_all(qol.parent().unwrap()).unwrap();
        st.comic_margins.add(U, 1).unwrap();
        assert_eq!(call(&router, Method::Get, &path, "").0, 404, "实验室开关默认关：已登记也 404，QML 代理不动");
        std::fs::write(&qol, r#"{"comicMinMargin":true}"#).unwrap();
        let (code, v) = call(&router, Method::Get, &path, "");
        assert_eq!((code, v["margins"].as_u64()), (200, Some(1)));
        assert_eq!(call(&router, Method::Post, "/margins/applied", &format!(r#"{{"uuid":"{U}"}}"#)).0, 200);
        assert_eq!(call(&router, Method::Get, &path, "").0, 404, "销账后不再返回");
        assert_eq!(call(&router, Method::Get, "/margins/not-a-uuid", "").0, 404, "非法 uuid 一律 404");
    }

    #[test]
    fn busy_book_rejects_optimize_deliver_delete_with_400_and_hint() {
        let t = tempfile::tempdir().unwrap();
        let st = state(&t);
        let router = router(st.clone());
        st.staging.stage_new("x.epub", b"PK").unwrap();
        assert!(st.staging.try_start_busy("x.epub"));
        for (path, body) in [
            ("/staging/optimize", r#"{"name":"x.epub"}"#),
            ("/staging/deliver", r#"{"name":"x.epub"}"#),
            ("/staging/delete", r#"{"name":"x.epub"}"#),
        ] {
            let (code, v) = call(&router, Method::Post, path, body);
            assert_eq!(code, 400, "{path}");
            assert!(msg(&v).contains("正在处理中"), "{path}: {v}");
            assert_eq!(v["ok"], false);
        }
        assert!(msg(&call(&router, Method::Post, "/staging/delete", r#"{"name":"x.epub"}"#).1).contains("再删除"), "删除的忙提示带后缀");
        // 解锁后删除恢复正常（200），列表里没有了
        st.staging.end_busy("x.epub");
        assert_eq!(call(&router, Method::Post, "/staging/delete", r#"{"name":"x.epub"}"#).0, 200);
        assert!(st.staging.list().is_empty());
    }

    #[test]
    fn optimize_and_deliver_validate_before_starting_anything() {
        let t = tempfile::tempdir().unwrap();
        let st = state(&t);
        let router = router(st.clone());
        // 不存在的书 / 不支持的格式：400，且没有留下忙锁
        for (path, body, hint) in [
            ("/staging/optimize", r#"{"name":"nope.epub"}"#, "没有这本书"),
            ("/staging/optimize", r#"{"name":"a.cbz"}"#, "只有 EPUB/PDF"),
            ("/staging/deliver", r#"{"name":"nope.epub"}"#, "没有这本书"),
            ("/staging/deliver", r#"{"name":"a.cbz"}"#, "xochitl 只读 EPUB"),
        ] {
            let (code, v) = call(&router, Method::Post, path, body);
            assert_eq!(code, 400, "{path} {body}");
            assert!(msg(&v).contains(hint), "{path} {body}: {v}");
        }
        assert!(!st.staging.is_busy("nope.epub") && !st.staging.is_busy("a.cbz"));
        // 缺字段 400；非法 JSON 400
        assert_eq!(call(&router, Method::Post, "/staging/optimize", "{}").0, 400);
        assert_eq!(call(&router, Method::Post, "/staging/optimize", "not json").0, 400);
    }

    #[test]
    fn cancel_reports_three_states_not_busy_uncancellable_cancellable() {
        let t = tempfile::tempdir().unwrap();
        let st = state(&t);
        let router = router(st.clone());
        let cancel = |name: &str| call(&router, Method::Post, "/staging/cancel", &format!(r#"{{"name":"{name}"}}"#));
        // 没在处理 → 400
        let (code, v) = cancel("x.epub");
        assert_eq!(code, 400);
        assert!(msg(&v).contains("没有在处理"), "{v}");
        // 在处理但这一步不能中途停（如单文件上传）→ 200 cancelled:false
        assert!(st.staging.try_start_busy("x.epub"));
        let (code, v) = cancel("x.epub");
        assert_eq!((code, &v["cancelled"]), (200, &serde_json::json!(false)), "{v}");
        assert!(msg(&v).contains("无法中途停止"));
        // 声明可取消 → 200 cancelled:true，且取消标记已登记
        st.staging.mark_cancellable("x.epub");
        let (code, v) = cancel("x.epub");
        assert_eq!((code, &v["cancelled"]), (200, &serde_json::json!(true)), "{v}");
        assert!(st.staging.is_cancelled("x.epub"));
    }

    #[test]
    fn mark_and_unknown_routes() {
        let t = tempfile::tempdir().unwrap();
        let st = state(&t);
        let router = router(st.clone());
        st.staging.stage_new("m.epub", b"PK").unwrap();
        assert_eq!(call(&router, Method::Post, "/staging/mark", r#"{"name":"m.epub","target":"koreader"}"#).0, 200);
        assert!(st.staging.list()[0].delivered.as_ref().unwrap().koreader.is_some(), "记了一笔 KOReader 落库");
        let (code, v) = call(&router, Method::Post, "/staging/mark", r#"{"name":"m.epub","target":"kindle"}"#);
        assert_eq!(code, 400);
        assert!(msg(&v).contains("native / koreader"));
        // 方法不对 405；路径不存在 404（已删的死路由 /inbox*、/staging/render/* 同样 404）
        assert_eq!(call(&router, Method::Get, "/staging/render/abc", "").0, 404);
        assert_eq!(call(&router, Method::Get, "/inbox", "").0, 404);
        assert_eq!(call(&router, Method::Get, "/staging/optimize", "").0, 405);
        assert_eq!(call(&router, Method::Get, "/nope", "").0, 404);
    }

    /// 按书阅读方向：参数校验、多本部分失败照样回 200（逐本原因在 failed）、全部失败 400、结果体现在列表里。
    #[test]
    fn direction_route_validates_and_reports_per_book() {
        let t = tempfile::tempdir().unwrap();
        let st = state(&t);
        let router = router(st.clone());
        st.staging.stage_new("a.epub", b"PK").unwrap();
        st.staging.stage_new("b.pdf", b"%PDF").unwrap();
        let (code, v) = call(&router, Method::Post, "/staging/direction", r#"{"name":"a.epub","direction":"up"}"#);
        assert_eq!(code, 400);
        assert!(msg(&v).contains("auto / rtl / ltr"), "{v}");
        assert_eq!(call(&router, Method::Post, "/staging/direction", r#"{"names":[],"direction":"rtl"}"#).0, 400);
        let (code, v) = call(&router, Method::Post, "/staging/direction", r#"{"names":["b.pdf"],"direction":"rtl"}"#);
        assert_eq!(code, 400, "全部失败");
        assert!(msg(&v).contains("不是 EPUB"), "{v}");
        let (code, v) = call(&router, Method::Post, "/staging/direction", r#"{"names":["a.epub","b.pdf"],"direction":"rtl"}"#);
        assert_eq!(code, 200, "{v}");
        assert_eq!((v["updated"].as_u64(), v["stale"].as_u64(), v["synced"].as_u64()), (Some(1), Some(1), Some(0)));
        assert_eq!(v["failed"][0]["name"], "b.pdf");
        let a = st.staging.list().into_iter().find(|e| e.name == "a.epub").unwrap();
        assert_eq!((a.direction, a.direction_stale), ("rtl", true));
        assert_eq!(call(&router, Method::Post, "/staging/direction", r#"{"name":"a.epub","direction":"auto"}"#).0, 200, "单本也可用 name");
        assert_eq!(st.staging.list().into_iter().find(|e| e.name == "a.epub").unwrap().direction, "auto");
    }

    /// 回归：网页上传收请求体期间（WiFi 上传大书可达分钟级）不再攥着 spool 锁——inbox 追平照常进行，
    /// 上传收完也照常入库。请求体用一个"等放行信号才吐数据"的 Reader 模拟慢客户端。
    #[test]
    fn slow_upload_does_not_block_inbox_processing() {
        struct Gate(std::sync::mpsc::Receiver<Vec<u8>>, Vec<u8>);
        impl std::io::Read for Gate {
            fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
                if self.1.is_empty() {
                    match self.0.recv() {
                        Ok(b) => self.1 = b,
                        Err(_) => return Ok(0),
                    }
                }
                let n = self.1.len().min(out.len());
                out[..n].copy_from_slice(&self.1[..n]);
                self.1.drain(..n);
                Ok(n)
            }
        }
        let t = tempfile::tempdir().unwrap();
        let st = state(&t);
        let router = router(st.clone());
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let up = std::thread::spawn(move || {
            let mut body = Gate(rx, Vec::new());
            let mut r = Request { method: Method::Post, path: "/staging".into(), query: parse_query(""), params: HashMap::new(), content_type: "multipart/form-data; boundary=B".into(), content_length: None, headers: vec![], body: &mut body };
            router.dispatch(&mut r).status
        });
        tx.send(b"--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"up.epub\"\r\n\r\nPK".to_vec()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100)); // 上传线程此刻卡在读请求体
        std::fs::write(st.spool.inbox().join("scp.epub"), b"x").unwrap();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let st2 = st.clone();
        std::thread::spawn(move || done_tx.send(st2.process_inbox(None)).unwrap());
        let out = done_rx.recv_timeout(std::time::Duration::from_secs(5)).expect("上传收体期间 inbox 追平不该被卡住");
        assert!(out.iter().any(|o| o.ok && o.name == "scp.epub"));
        tx.send(b"\r\n--B--\r\n".to_vec()).unwrap();
        drop(tx);
        assert_eq!(up.join().unwrap(), 200);
        assert!(st.staging.has("up.epub") && st.staging.has("scp.epub"));
    }

    #[test]
    fn download_route_streams_with_mime_length_and_disposition() {
        let t = tempfile::tempdir().unwrap();
        let st = state(&t);
        let router = router(st.clone());
        st.staging.stage_new("书.PDF", b"%PDF-1").unwrap();
        let mut empty: &[u8] = b"";
        let mut r = Request { method: Method::Get, path: "/staging/file".into(), query: parse_query("name=%E4%B9%A6.PDF"), params: HashMap::new(), content_type: String::new(), content_length: None, headers: vec![], body: &mut empty };
        let rep = router.dispatch(&mut r);
        assert_eq!((rep.status, rep.content_type.as_str()), (200, "application/pdf"), "扩展名大小写不敏感");
        let h = |k: &str| rep.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
        assert_eq!(h("Content-Length"), "6");
        assert!(h("Content-Disposition").contains("filename*=UTF-8''%E4%B9%A6.PDF"));
    }

    #[test]
    fn status_route_reports_spool_and_dead_xochitl() {
        let t = tempfile::tempdir().unwrap();
        let st = state(&t);
        let router = router(st.clone());
        let (code, v) = call(&router, Method::Get, "/status", "");
        assert_eq!(code, 200);
        assert_eq!((v["ok"].clone(), v["uploadReachable"].clone()), (serde_json::json!(true), serde_json::json!(false)));
        assert_eq!(v["spool"]["pending"], 0);
    }
}
