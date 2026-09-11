//! note-serve —— 笔记·本（loopback 8798）。它是网页「笔记」tab 的注册方（tab 只挂一个服务，前端组合 ink/transcribe/mind/notes 四段）；
//! 本体职责是把条目库投影成设备笔记本（复用书本自己所在的设备文件夹，一章一本，xochitl 7 种打字样式）与 md 导出
//! （`export.rs`，落 `$XDG_DATA_HOME/notes/vault/<书名>/`，供 host `notes pull` 拉走）。
//! 生成编排见 `publish.rs`：只读 ink-serve 的条目库（改字段仍是 ink-serve 的事）、按章指纹判断要不要重传，
//! 传完按 `visibleName`+时间窗认领设备新分配的 uuid，旧版本入 `book-serve` 回收站队列（真机验证过的软删路）。
//! 路由（经网关前缀 `/api/notes`）：`GET /status` · `GET /events` ·
//! `GET /books/{uuid}/notebooks`（本地记着的各章生成状态）· `GET /books/{uuid}/exports`（本地记着的
//! 各章导出状态，同上但对应 md）·
//! `POST /books/{uuid}/generate`（全书重新投影+按需上传）· `POST /books/{uuid}/chapters/{idx}/generate`（单章）·
//! `POST /books/{uuid}/import-md`（单篇 markdown → 新设备笔记本文档，独立于条目库，见 `publish::import_markdown`，
//! 白皮书 §03af）·
//! `POST /books/{uuid}/export`（全书导出 md，落设备盘，指纹没变的章节自动跳过）·
//! `POST /books/{uuid}/chapters/{idx}/export`（单章，同上，响应带 `status`：written/unchanged/empty）·
//! `GET /books/{uuid}/chapters/{idx}/export.md`（单章同一份内容当浏览器下载吐回去，`Content-Disposition`，
//! 三期新增：光落设备盘用户够不着，见 `export.rs`）·
//! `GET /books/{uuid}/sync`（整理区第三轮反馈新增：每章设备笔记本/Obsidian md 是否跟当前条目内容
//! 同步，前端拿这个决定"生成完成后移出待处理列表"，见白皮书 §03x）·
//! `GET /books/{uuid}/vault.json`（2026-09-16 新增：读回已落盘的 vault 目录内容，供 host
//! `shelf notes pull` 拉到本机 Obsidian vault，见 `export::manifest` 文档）。
mod chapter_store;
mod config;
mod export;
mod export_state;
mod ink;
mod notebooks;
mod publish;
mod rmdoc;
mod trash;

use config::NoteConfig;
use export_state::ExportState;
use ink::{EntryStore, InkHttp};
use notebooks::NotebookState;
use publish::{generate_book, generate_chapter, ChapterResult, Ctx, Uploader, XochitlUploader};
use rmsvc_core::events::EventBus;
use rmsvc_core::http::{bind, ApiError, ApiResult, Reply, Request, Router};
use rmsvc_core::paths::Paths;
use rmsvc_core::service::{self, ServiceSpec};
use std::sync::Arc;
use trash::{BookServeTrash, TrashSink};

pub const APP: &str = "notes";

const SPEC: ServiceSpec = ServiceSpec { name: "note-serve", label: "笔记·本", version: env!("CARGO_PKG_VERSION"), default_bind: "127.0.0.1:8798", tab: Some(("笔记", 25)) };

struct State {
    paths: Paths,
    cfg: NoteConfig,
    store: Box<dyn EntryStore>,
    uploader: Box<dyn Uploader>,
    trash: Box<dyn TrashSink>,
    notebooks: NotebookState,
    exports: ExportState,
    bus: Arc<EventBus>,
    /// 生成/导入/导出串行化：同一章两次「推送本章」并发（双击、两个浏览器页）时，两边都看到指纹变了、各传一份，
    /// 再按同一个 visibleName 认领到同一个 uuid——另一份成了追踪不到的孤儿笔记本；导入撞名去重同理（2026-09-24）。
    publish_lock: std::sync::Mutex<()>,
}

impl State {
    fn publishing(&self) -> std::sync::MutexGuard<'_, ()> {
        rmsvc_core::sync::lock(&self.publish_lock)
    }
    fn ctx(&self, now_ms: u64) -> Ctx<'_> {
        Ctx { store: self.store.as_ref(), uploader: self.uploader.as_ref(), trash: self.trash.as_ref(), state: &self.notebooks, now_ms }
    }
}

/// 路径参数 `{idx}` → 章序号（非数字 400）。
fn chapter_idx(r: &Request<'_>) -> Result<usize, ApiError> {
    r.param("idx").parse().map_err(|_| ApiError::bad("章序号不对"))
}

/// 每章"设备笔记本 / Obsidian md 是否跟当前条目内容同步"（`GET /books/{uuid}/sync` 的本体，纯读）：当前活条目算出的
/// 指纹 vs 上次成功生成/导出时记的指纹。`xxxNeeded`：这一章有没有条目要投这个去处——没有的话 `xxxSynced` 恒真
/// （俩指纹都是 `None`），但网页得知道是"没什么要同步的"还是"已经同步过"，两种意思不一样，靠这个字段区分
/// （前端据此决定要不要显示对应的 📓/🔗 徽章）。
fn sync_status(book: &notecore::model::Book, notebooks: &NotebookState, exports: &ExportState) -> Vec<serde_json::Value> {
    // 两份记录各读一次整本（此前逐章 `get`，每章各读+解析一遍文件，40 章的书一次刷新就是 80 遍）。
    let (nb_all, ob_all) = (notebooks.list(&book.uuid), exports.list(&book.uuid));
    (0..book.chapters.len())
        .map(|idx| {
            let nb_fp = notecore::project::fingerprint_chapter(book, idx);
            let nb_rec = nb_all.get(&idx);
            let nb_synced = nb_fp.as_deref() == nb_rec.as_ref().map(|r| r.fingerprint.as_str());
            let ob_fp = notecore::export::fingerprint_chapter(book, idx);
            let ob_rec = ob_all.get(&idx);
            let ob_synced = ob_fp.as_deref() == ob_rec.as_ref().map(|r| r.fingerprint.as_str());
            serde_json::json!({
                "chapter": idx,
                "notebookNeeded": nb_fp.is_some(), "notebookSynced": nb_synced, "notebookGeneratedAt": nb_rec.map(|r| r.generated_at),
                "obsidianNeeded": ob_fp.is_some(), "obsidianSynced": ob_synced, "obsidianExportedAt": ob_rec.map(|r| r.exported_at),
            })
        })
        .collect()
}

fn results_reply(results: &[ChapterResult]) -> ApiResult {
    Ok(Reply::ok(&serde_json::json!({"chapters": results})))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind_addr = service::parse_bind(&args, SPEC.default_bind);
    let paths = Paths::from_env();
    let cfg: NoteConfig = rmsvc_core::config::load_or_seed(&paths.app_config_dir(APP).join("note.json"));
    let notebooks = NotebookState::new(paths.app_state_dir(APP).join("notebooks"));
    if let Err(e) = notebooks.ensure() {
        eprintln!("[note-serve] 建目录失败: {e}");
        std::process::exit(1);
    }
    let exports = ExportState::new(paths.app_state_dir(APP).join("exports"));
    if let Err(e) = exports.ensure() {
        eprintln!("[note-serve] 建目录失败: {e}");
        std::process::exit(1);
    }
    let uploader = XochitlUploader::new(&cfg.xochitl_host, &paths.xochitl_dir(), cfg.upload_timeout_secs);
    let st = Arc::new(State {
        store: Box::new(InkHttp::new(paths.clone())),
        uploader: Box::new(uploader),
        trash: Box::new(BookServeTrash::new(paths.clone())),
        notebooks,
        exports,
        cfg,
        paths: paths.clone(),
        bus: Arc::new(EventBus::new()),
        publish_lock: std::sync::Mutex::new(()),
    });
    let router = Router::new()
        .get("/events", bind(&st, |s, _| Ok(s.bus.sse_reply())))
        .get("/status", bind(&st, |s, _| Ok(Reply::ok(&serde_json::json!({"ok": true, "vault": s.paths.app_data_dir(APP).join("vault"), "xochitlHost": s.cfg.xochitl_host})))))
        .get("/books", bind(&st, |s, _| {
            let items = s.store.list_books().map_err(ApiError::bad)?;
            // `title` 是 2026-09-16 加的：host `shelf notes pull` 靠它认书（不用另起一个 book detail
            // 端点），别的既有消费方（前端）本来就不用这个列表拿标题，加字段不影响它们。
            let out: Vec<serde_json::Value> = items.iter().map(|b| serde_json::json!({"uuid": b.uuid, "title": b.title, "notebooks": s.notebooks.list(&b.uuid)})).collect();
            Ok(Reply::ok(&serde_json::json!({"items": out})))
        }))
        .get("/books/{uuid}/notebooks", bind(&st, |s, r| {
            let items: std::collections::BTreeMap<usize, notebooks::ChapterRecord> = s.notebooks.list(r.param("uuid"));
            Ok(Reply::ok(&serde_json::json!({"chapters": items})))
        }))
        .get("/books/{uuid}/exports", bind(&st, |s, r| {
            let items: std::collections::BTreeMap<usize, export_state::ExportRecord> = s.exports.list(r.param("uuid"));
            Ok(Reply::ok(&serde_json::json!({"chapters": items})))
        }))
        .post("/books/{uuid}/generate", bind(&st, |s, r| {
            let uuid = r.param("uuid").to_string();
            let _g = s.publishing();
            let results = generate_book(&s.ctx(rmsvc_core::clock::now_ms()), &uuid).map_err(ApiError::bad)?;
            s.bus.publish("notes", "notebooks");
            results_reply(&results)
        }))
        .post("/books/{uuid}/chapters/{idx}/generate", bind(&st, |s, r| {
            let uuid = r.param("uuid").to_string();
            let idx = chapter_idx(r)?;
            let _g = s.publishing();
            let book = s.store.book(&uuid).map_err(ApiError::bad)?;
            let result = generate_chapter(&s.ctx(rmsvc_core::clock::now_ms()), &book, idx);
            s.bus.publish("notes", "notebooks");
            results_reply(std::slice::from_ref(&result))
        }))
        // 单篇 markdown → 一个新的设备笔记本文档，独立于条目库（不经章节投影/指纹追踪，见
        // `publish::import_markdown` 文档）。body：`{title, markdown}`，两者都必填。
        .post("/books/{uuid}/import-md", bind(&st, |s, r| {
            let uuid = r.param("uuid").to_string();
            let j = r.json()?;
            let title = j.str("title")?.to_string();
            let markdown = j.str("markdown")?.to_string();
            let _g = s.publishing();
            let (visible_name, doc_uuid) = publish::import_markdown(&s.ctx(rmsvc_core::clock::now_ms()), &uuid, &title, &markdown).map_err(ApiError::bad)?;
            Ok(Reply::ok(&serde_json::json!({"ok": true, "uuid": doc_uuid, "visibleName": visible_name})))
        }))
        .post("/books/{uuid}/export", bind(&st, |s, r| {
            let _g = s.publishing();
            let book = s.store.book(r.param("uuid")).map_err(ApiError::bad)?;
            let outcomes = export::export_book(&s.paths.app_data_dir(APP), &book, &s.exports).map_err(ApiError::internal)?;
            let files = outcomes.iter().filter(|o| o.has_content()).count();
            Ok(Reply::ok(&serde_json::json!({"ok": true, "files": files})))
        }))
        .post("/books/{uuid}/chapters/{idx}/export", bind(&st, |s, r| {
            let idx = chapter_idx(r)?;
            let _g = s.publishing();
            let book = s.store.book(r.param("uuid")).map_err(ApiError::bad)?;
            if book.chapters.get(idx).is_none() {
                return Err(ApiError::bad("没有这一章"));
            }
            // 单章按钮也整本重导：文件都很小，重写比"只动一个文件+另外判断索引要不要变"更简单可靠——
            // 索引页"哪些章有内容"本来就得看全书才能算对；指纹没变的章节 export_book 内部会自己跳过，
            // 不会白白重写没变化的其它章节。
            let outcomes = export::export_book(&s.paths.app_data_dir(APP), &book, &s.exports).map_err(ApiError::internal)?;
            let outcome = outcomes.get(idx).copied().unwrap_or(export::ExportOutcome::Empty);
            Ok(Reply::ok(&serde_json::json!({"ok": true, "wrote": outcome.has_content(), "status": outcome})))
        }))
        // 「整理」区第三轮反馈：每章"设备笔记本/Obsidian md 是不是已经跟当前条目内容同步"——比较
        // 当前活条目算出的指纹和上次成功生成/导出时记的指纹，没有要投的条目算"没什么要同步的"（true）。
        // 只读，不碰任何文件/网络（生成/导出本身该点对应按钮，这里只是查状态）。
        .get("/books/{uuid}/sync", bind(&st, |s, r| {
            let book = s.store.book(r.param("uuid")).map_err(ApiError::bad)?;
            Ok(Reply::ok(&serde_json::json!({"chapters": sync_status(&book, &s.notebooks, &s.exports)})))
        }))
        // 光落设备盘用户够不着（得 SSH）——这个额外把同一份内容当浏览器下载直接吐回去，配合网关
        // 新转发的 Content-Disposition 头，点「导出 md」之后浏览器会像正常网页下载一样存到本地
        // （存到哪由浏览器自己的下载设置决定：没配置就是系统默认下载目录，配了"每次询问"就会弹框
        // 让用户选，网关/服务端管不到也不该管这一层）。
        .get("/books/{uuid}/chapters/{idx}/export.md", bind(&st, |s, r| {
            let idx = chapter_idx(r)?;
            let book = s.store.book(r.param("uuid")).map_err(ApiError::bad)?;
            let title = book.chapters.get(idx).ok_or_else(|| ApiError::bad("没有这一章"))?.clone();
            let md = notecore::export::export_chapter_md(&book, idx).ok_or_else(|| ApiError::not_found("本章没有可导出的内容"))?;
            let filename = format!("{}.md", notecore::export::chapter_stem(idx, &title));
            Ok(Reply::bytes("text/markdown; charset=utf-8", md.into_bytes()).with_header("Content-Disposition", &export::content_disposition(&filename)))
        }))
        // host `shelf notes pull` 用：读回 `POST .../export` 已经落盘的 vault 目录内容（不触发导出，
        // 纯读——调用方该自己先 POST export 保证内容是最新的）。见 `export::manifest` 文档。
        .get("/books/{uuid}/vault.json", bind(&st, |s, r| {
            let book = s.store.book(r.param("uuid")).map_err(ApiError::bad)?;
            let m = export::manifest(&s.paths.app_data_dir(APP), &book.title).map_err(ApiError::internal)?;
            Ok(Reply::ok(&m))
        }));
    println!("[note-serve] 状态 {}；xochitl {}", st.notebooks.dir().display(), st.cfg.xochitl_host);
    if let Err(e) = service::run(&SPEC, &bind_addr, &paths, router) {
        eprintln!("[note-serve] {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notecore::model::{Book, Entry, Status, Style};
    use std::collections::HashMap;

    fn entry(id: &str, chapter: usize, dest: notecore::model::Destination) -> Entry {
        Entry { id: id.into(), page: "p".into(), page_index: 0, chapter: Some(chapter), chapter_title: String::new(), subhead: None, quote: None, ink: None, drafts: vec![], text: Some("正文".into()), style: Style::Body, ask_ai: false, question: None, answer: None, status: Status::Reviewed, destination: dest, source: Default::default(), created: 0, updated: 0 }
    }

    #[test]
    fn sync_status_distinguishes_needed_synced_and_never_pushed() {
        use notecore::model::Destination;
        let t = tempfile::tempdir().unwrap();
        let (notebooks, exports) = (NotebookState::new(t.path().join("nb")), ExportState::new(t.path().join("ex")));
        notebooks.ensure().unwrap();
        exports.ensure().unwrap();
        let book = Book { uuid: "b".into(), title: "书".into(), chapters: vec!["一".into(), "二".into()], entries: vec![entry("e1", 0, Destination::Both), entry("e2", 1, Destination::Obsidian)], ..Default::default() };
        // 都没推送过：第 0 章两个去处都需要且未同步；第 1 章只要 Obsidian，笔记本"不需要"（fp=None 两边 None → 恒同步）
        let v = sync_status(&book, &notebooks, &exports);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0]["notebookNeeded"].as_bool(), v[0]["notebookSynced"].as_bool(), v[0]["obsidianSynced"].as_bool()), (Some(true), Some(false), Some(false)));
        assert_eq!((v[1]["notebookNeeded"].as_bool(), v[1]["notebookSynced"].as_bool(), v[1]["obsidianNeeded"].as_bool()), (Some(false), Some(true), Some(true)));
        assert!(v[0]["notebookGeneratedAt"].is_null() && v[0]["obsidianExportedAt"].is_null());
        // 记下当前指纹后同步
        let fp = notecore::export::fingerprint_chapter(&book, 0).unwrap();
        exports.set("b", 0, export_state::ExportRecord { fingerprint: fp, exported_at: 7 }).unwrap();
        let v = sync_status(&book, &notebooks, &exports);
        assert_eq!((v[0]["obsidianSynced"].as_bool(), v[0]["obsidianExportedAt"].as_u64()), (Some(true), Some(7)));
    }

    #[test]
    fn chapter_idx_parses_or_rejects() {
        let mut empty: &[u8] = b"";
        let mut req = Request { method: rmsvc_core::http::Method::Get, path: String::new(), query: HashMap::new(), params: HashMap::new(), content_type: String::new(), content_length: None, headers: vec![], body: &mut empty };
        req.params.insert("idx".into(), "3".into());
        assert_eq!(chapter_idx(&req).unwrap(), 3);
        req.params.insert("idx".into(), "x".into());
        assert!(chapter_idx(&req).is_err());
    }
}
