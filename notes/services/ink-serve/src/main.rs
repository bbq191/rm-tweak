//! ink-serve —— 笔记·矿（loopback 8795）。监听原生书库（事件驱动、防抖），书页 `.rm` 变了就只扫变更页：
//! 勾画（GlyphRange）+ 旁边手写（笔画簇）→ 条目 → 裁图 → 条目库（**唯一写者**，其它服务经这里改字段）。
//! 路由（经网关前缀 `/api/ink`）：`GET /books` · `GET /books/{uuid}` · `GET /books/{uuid}/crops/{file}` ·
//! `POST /books/{uuid}/entries/{id}`（text/style/draft/answer/askAi/question/destination 字段更新，
//! 缺省底座无 PATCH；`text` 现在走 `notecore::model::Entry::apply_marked_text`——行首 `-`/`1.`/`- [ ]`/`口`/`##`/
//! `### ` 标记自动定样式（与设备内置打字样式一一对应，2026-09-25）并从正文剥掉，不再需要网页手动选样式的下拉（整理区第二轮反馈点 1，
//! 2026-09-08，见白皮书 §03u）；`style` 字段仍保留，给 `transcribe-serve::worker` 写草稿时的内部路径用
//! （它走行首标记兜底出的是 `Marker::Style`，不经过 `text` 这条路；09-25 前另有 `subheadHint` 覆盖小节名，已删）——三期（2026-09-08）砍掉了"分区"这个概念，`## 文字`/`section`/`sectionHint`/
//! `PUT .../sections` 整个都没了，AI 触发早就是 `askAi`+`question` 的事，笔记本排版分组也不要了，见白皮书
//! §03s；`askAi`+`question` 是"问AI"勾选框+问题输入框，`mind-serve` 读这两个字段触发按条目单发问答；
//! `GET /books` 只列条目库里还有活条目的书）·
//! `POST /books/{uuid}/entries/{id}/request`（浏览态"转入笔记"：`Mined→Pending`）·
//! `POST /books/{uuid}/entries/{id}/skip`（浏览态"不需要"：`Mined→Skipped`）·
//! `POST /books/{uuid}/entries/{id}/archive`（三期"不要了"：`→Archived`，两处投影都摘掉，见
//! `notecore::model::Entry::set_triage`；已撤销/已归档的条目对以上三个动作都拒绝）·
//! `POST /books/{uuid}/entries/{id}/restore`（回收站"恢复"，整理区第二轮反馈点 3，2026-09-08：
//! `Skipped`/`Revoked`/`Archived` 都能恢复，落点按条目已有内容倒推，见 `notecore::model::Entry::restore`；
//! 只对终态条目生效，对活条目调用会被拒）·
//! `POST /books/{uuid}/purge`（清空回收站：物理删掉 `Archived`/`Revoked`/`Skipped` 这三种终态条目，
//! 手动触发、不可恢复，见 `notecore::model::Book::purge_terminal`）·
//! `POST /books/{uuid}/rescan` · `GET /events`。条目库 `$XDG_STATE_HOME/notes/books/<uuid>.json`，裁图 `$XDG_DATA_HOME/notes/crops/`。
//! `destination` 字段（三期，落设备笔记本/Obsidian/两处都要，缺省两处都要）走通用 PATCH，见下方。
//! `POST /koreader/import`（KOReader 高亮/生词回流，2026-09-16，见 `koreader.rs`+笔记线白皮书 §03al：
//! 拉 koreader-serve 的 `/annotations`+`/vocabulary` 原始数据，按增量规则并入条目库——手动触发，不像
//! xochitl 那条线自动 fswatch，KOReader 那边没有等价的"改了就通知"事件源）。
mod bookdb;
mod config;
mod crop;
mod doc;
mod ingest;
mod koreader;
mod search;

use bookdb::BookDb;
use config::IngestConfig;
use koreader::KoreaderHttp;
use notecore::model::{Answer, Destination, Draft, Entry, Status, Style};
use rmsvc_core::events::EventBus;
use rmsvc_core::fs::plain_name;
use rmsvc_core::http::{bind, ApiError, ApiResult, Reply, Request, Router};
use rmsvc_core::paths::Paths;
use rmsvc_core::service::{self, ServiceSpec};
use std::sync::Arc;

pub const APP: &str = "notes";

const SPEC: ServiceSpec = ServiceSpec { name: "ink-serve", label: "笔记·矿", version: env!("CARGO_PKG_VERSION"), default_bind: "127.0.0.1:8795", tab: None };

struct State {
    paths: Paths,
    cfg: IngestConfig,
    db: BookDb,
    bus: Arc<EventBus>,
    koreader: KoreaderHttp,
}

impl State {
    fn crops_dir(&self) -> std::path::PathBuf {
        self.paths.app_data_dir(APP).join("crops")
    }
    fn ingest(&self, uuid: &str) {
        // 兜住解析 panic（`.rm`/`.epubindex` 是设备写的二进制，解析器难保对所有畸形输入都不 panic）：摄取跑在
        // 书库监听线程里，一次 panic 会让监听线程整个退出、之后再也不摄取，而 HTTP 照常应答、看不出异常。
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ingest::ingest_doc(&self.paths.xochitl_dir(), &self.crops_dir(), &self.db, &self.cfg, uuid, rmsvc_core::clock::now_secs())));
        let r = r.unwrap_or_else(|_| Err("摄取时 panic（已兜住，本书这次跳过）".to_string()));
        match r {
            // `s.merge.revoked > 0` 单独成立的情况＝书被移进回收站/删除、`revoke_stale` 撤了条目但没扫任何页（pages==0）；
            // 这时也要发事件，不然网页「笔记」列表要等到下一次不相干的事件才会把这本书摘掉。
            Ok(Some(s)) if s.pages > 0 || s.merge.revoked > 0 => {
                println!("[ink-serve] {uuid}: 页 {} 新增 {} 变更 {} 不变 {} 撤销 {} 复活 {}", s.pages, s.merge.added, s.merge.changed, s.merge.unchanged, s.merge.revoked, s.merge.revived);
                self.bus.publish("notes", "entries");
            }
            Ok(_) => {}
            Err(e) => eprintln!("[ink-serve] {uuid}: {e}"),
        }
    }
}

/// 对一本书里的一条条目做「读—改—写」，并统一映射结果：书/条目不存在 → 404；业务规则拒绝（`f` 返回 `Err`）→ 400；
/// 成功 → 发 `entries` 事件。改字段（POST entries/{id}）、浏览态动作、回收站恢复共用（此前各写一遍逐行相同的
/// "先 load 判存在 → update → 三分支 match → publish"）。
fn edit_entry(s: &State, uuid: &str, id: &str, f: impl FnOnce(&mut Entry) -> Result<(), String>) -> ApiResult {
    match s.db.update_existing(uuid, |b| b.entries.iter_mut().find(|e| e.id == id).map(f)).map_err(ApiError::internal)? {
        None => return Err(ApiError::not_found("没有这本书的条目")),
        Some(None) => return Err(ApiError::not_found("没有这条目")),
        Some(Some(Err(e))) => return Err(ApiError::bad(e)),
        Some(Some(Ok(()))) => {}
    }
    s.bus.publish("notes", "entries");
    Ok(Reply::ok(&serde_json::json!({"ok": true})))
}

/// 浏览态动作：`Mined→Pending`（转入笔记）/ `Mined→Skipped`（不需要），见 `notecore::model::Entry::set_triage`。
fn triage(s: &State, r: &mut Request<'_>, target: Status) -> ApiResult {
    let now = rmsvc_core::clock::now_secs();
    edit_entry(s, r.param("uuid"), r.param("id"), |e| e.set_triage(target, now))
}

/// 每条条目保留的转写草稿份数（最新在前）。
const MAX_DRAFTS: usize = 10;

/// 通用改字端点（`POST /books/{uuid}/entries/{id}`）的字段规则：网页改字/问题/去处，transcribe-serve 写草稿（带样式建议），
/// mind-serve 写回答。抽成函数便于单测。
fn patch_entry(e: &mut Entry, j: &serde_json::Value, now: u64) -> Result<(), String> {
    // 终态守卫（2026-09-09 审计补）：这条通用改字端点原来不检查状态，能把已"跳过/撤销/
    // 删除"的条目通过 apply_marked_text/写草稿悄悄拉回 Draft，绕开 set_triage/restore
    // 明文规定的业务规则——先恢复（`/restore`）才能再改。
    if e.is_terminal() {
        return Err("这条已跳过/撤销/删除，不能再改，请先在回收站里恢复".to_string());
    }
    // 用户直接在网页文本框改字：跟转写草稿写回同一套行首标记规则（`notecore::model::Entry::
    // apply_marked_text`）——`-`/`1.`/`口`/`##`/`### ` 都认，样式不再靠单独的下拉手动选
    // （整理区第二轮反馈点 1，2026-09-08，见白皮书 §03u）。
    if let Some(t) = j.get("text").and_then(|v| v.as_str()) {
        e.apply_marked_text(t, now);
    }
    let draft = j.get("draft").and_then(|v| serde_json::from_value::<Draft>(v.clone()).ok());
    // 随草稿带来的样式（转写结果的行首标记）只是建议：条目已经有人校对过的文字时不动它的样式——
    // 补了几笔触发的再转写不能把用户定稿那条的圆点/编号改掉（增量规则"校对文本永不被覆盖"，
    // 样式是它的一部分；2026-09-25 第四轮审计）。
    let style_is_suggestion = draft.is_some() && e.text.is_some();
    if let Some(v) = j.get("style").and_then(|v| serde_json::from_value::<Style>(v.clone()).ok()).filter(|_| !style_is_suggestion) {
        e.style = v;
    }
    if let Some(d) = draft {
        e.drafts.insert(0, d);
        // 草稿只留最近几份：每补几笔/每点一次「重新转写」都会加一份，不设上限条目库会一直长，每次写回都要重写整本。
        e.drafts.truncate(MAX_DRAFTS);
        if e.text.is_none() {
            e.status = Status::Draft;
        }
    }
    if let Some(a) = j.get("answer") {
        e.answer = serde_json::from_value::<Answer>(a.clone()).ok();
    }
    // 「问AI」勾选框 + 问题输入框（二期按条目单发，mind-serve 触发的动作端点另开，见白皮书 §03n）。
    if let Some(v) = j.get("askAi").and_then(|v| v.as_bool()) {
        e.ask_ai = v;
    }
    if let Some(v) = j.get("question") {
        e.question = v.as_str().filter(|s| !s.trim().is_empty()).map(str::to_string);
    }
    // 落设备笔记本 / 落 Obsidian / 两处都要（三期），见 notecore::model::Destination。
    if let Some(v) = j.get("destination").and_then(|v| serde_json::from_value::<Destination>(v.clone()).ok()) {
        e.destination = v;
    }
    e.updated = now;
    Ok(())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bind_addr = service::parse_bind(&args, SPEC.default_bind);
    let paths = Paths::from_env();
    let cfg: IngestConfig = rmsvc_core::config::load_or_seed(&paths.app_config_dir(APP).join("ink.json"));
    let db = BookDb::new(paths.app_state_dir(APP).join("books"));
    let koreader = KoreaderHttp::new(paths.clone());
    let st = Arc::new(State { paths: paths.clone(), cfg, db, bus: Arc::new(EventBus::new()), koreader });
    if let Err(e) = std::fs::create_dir_all(st.crops_dir()).and_then(|_| st.db.ensure()) {
        eprintln!("[ink-serve] 建目录失败: {e}");
        std::process::exit(1);
    }
    // 追平 + 监听：启动扫一遍有手写页的 EPUB，**外加**条目库里已知但这次没扫到的书（回收站/删除清场，见
    // `ingest::revoke_stale`——上次运行之后被移到回收站/删掉的书，不追平一次不会被摘出网页列表）；
    // 之后书库目录有写入（合上书 xochitl 重写 .content/.metadata）防抖后只扫涉及的文档。
    {
        let st = st.clone();
        std::thread::spawn(move || {
            let mut catchup: std::collections::BTreeSet<String> = ingest::candidate_docs(&st.paths.xochitl_dir()).into_iter().collect();
            catchup.extend(st.db.list().into_iter().map(|b| b.uuid.clone()));
            for u in catchup {
                st.ingest(&u);
            }
            let lib = st.paths.xochitl_dir();
            let debounce = std::time::Duration::from_secs(st.cfg.debounce_secs.max(1));
            rmsvc_core::fswatch::watch_debounced(&lib, debounce, |names| {
                let mut seen = std::collections::BTreeSet::new();
                for n in names {
                    if let Some(u) = doc::uuid_of_event(n) {
                        seen.insert(u.to_string());
                    }
                }
                for u in seen {
                    st.ingest(&u);
                }
            });
        });
    }
    let router = Router::new()
        .get("/events", bind(&st, |s, _| Ok(s.bus.sse_reply())))
        .get("/books", bind(&st, |s, _| {
            let items: Vec<serde_json::Value> = s.db.list_active().iter().map(|b| serde_json::json!({"uuid": b.uuid, "title": b.title, "chapters": b.chapters.len(), "entries": b.entries.iter().filter(|e| e.status != Status::Revoked).count(), "pending": b.entries.iter().filter(|e| e.needs_transcribe()).count()})).collect();
            Ok(Reply::ok(&serde_json::json!({"items": items})))
        }))
        // 全文搜索：跨书搜勾画原文/定稿/草稿/提问/AI 回答/书名，见 search.rs。`limit` 缺省 50、上限 200。
        .get("/search", bind(&st, |s, r| {
            let q = r.q("q").unwrap_or_default();
            let limit = r.q("limit").and_then(|v| v.parse::<usize>().ok()).unwrap_or(50).clamp(1, 200);
            Ok(Reply::ok(&serde_json::json!({"items": search::search(&s.db.list(), q, limit)})))
        }))
        .get("/books/{uuid}", bind(&st, |s, r| {
            let b = s.db.read(r.param("uuid")).map_err(ApiError::internal)?.ok_or_else(|| ApiError::not_found("没有这本书的条目"))?;
            Ok(Reply::ok(&*b))
        }))
        .get("/books/{uuid}/crops/{file}", bind(&st, |s, r| {
            let f = plain_name(r.param("file")).map_err(ApiError::bad)?;
            let bytes = std::fs::read(s.crops_dir().join(f)).map_err(|_| ApiError::not_found("没有这张裁图"))?;
            Ok(Reply::bytes("image/png", bytes))
        }))
        .post("/books/{uuid}/entries/{id}", bind(&st, |s, r| {
            let (uuid, id) = (r.param("uuid").to_string(), r.param("id").to_string());
            let j = r.json()?;
            let now = rmsvc_core::clock::now_secs();
            edit_entry(s, &uuid, &id, |e| patch_entry(e, &j.0, now))
        }))
        .post("/books/{uuid}/entries/{id}/request", bind(&st, |s, r| triage(s, r, Status::Pending)))
        .post("/books/{uuid}/entries/{id}/skip", bind(&st, |s, r| triage(s, r, Status::Skipped)))
        .post("/books/{uuid}/entries/{id}/archive", bind(&st, |s, r| triage(s, r, Status::Archived)))
        .post("/books/{uuid}/entries/{id}/restore", bind(&st, |s, r| {
            let now = rmsvc_core::clock::now_secs();
            edit_entry(s, r.param("uuid"), r.param("id"), |e| e.restore(now))
        }))
        .post("/books/{uuid}/purge", bind(&st, |s, r| {
            let removed = s.db.update_existing(r.param("uuid"), |b| b.purge_terminal()).map_err(ApiError::internal)?.ok_or_else(|| ApiError::not_found("没有这本书的条目"))?;
            if removed > 0 {
                s.bus.publish("notes", "entries");
            }
            Ok(Reply::ok(&serde_json::json!({"ok": true, "removed": removed})))
        }))
        .post("/books/{uuid}/rescan", bind(&st, |s, r| {
            let uuid = plain_name(r.param("uuid")).map_err(ApiError::bad)?.to_string(); // ingest 会拼 xochitl 目录路径，同样要防穿越
            // 强制：清掉页 mtime 记录再摄取
            // 只对条目库里已有的书清（`update_existing`）：此前用 `update(.., Default::default)` 会给一个从未摄取过的 uuid
            // 建出一份 uuid/标题都是空串的空书，之后摄取沿用它、书就永远带着空 uuid。
            let _ = s.db.update_existing(&uuid, |b| b.page_mtimes.clear());
            s.ingest(&uuid);
            Ok(Reply::ok(&serde_json::json!({"ok": true})))
        }))
        .post("/koreader/import", bind(&st, |s, _| {
            let stats = koreader::import(&s.db, &s.koreader, rmsvc_core::clock::now_secs()).map_err(ApiError::internal)?;
            if stats.highlight_books > 0 || stats.highlights.added > 0 || stats.vocab.added > 0 {
                s.bus.publish("notes", "entries");
            }
            Ok(Reply::ok(&serde_json::json!({"ok": true, "highlightBooks": stats.highlight_books, "highlightsAdded": stats.highlights.added, "highlightsRevoked": stats.highlights.revoked, "vocabAdded": stats.vocab.added})))
        }));
    println!("[ink-serve] 条目库 {}；裁图 {}；监听 {}", st.db.dir().display(), st.crops_dir().display(), st.paths.xochitl_dir().display());
    if let Err(e) = service::run(&SPEC, &bind_addr, &paths, router) {
        eprintln!("[ink-serve] {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: Option<&str>, style: Style) -> Entry {
        let mut e: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0,"status":"pending"}"#).unwrap();
        e.text = text.map(str::to_string);
        e.style = style;
        e
    }
    fn draft_body(text: &str, style: &str) -> serde_json::Value {
        serde_json::json!({"draft": {"text": text, "backend": "b", "at": 1, "hash": "h"}, "style": style})
    }

    /// 回归：补笔触发的再转写（带样式建议）不改已校对条目的样式；没校对过的照常采纳。
    #[test]
    fn draft_style_suggestion_never_overrides_reviewed_entry() {
        let mut reviewed = entry(Some("我定稿的正文"), Style::Body);
        reviewed.status = Status::Reviewed;
        patch_entry(&mut reviewed, &draft_body("查作者", "bullet"), 5).unwrap();
        assert_eq!((reviewed.style, reviewed.status, reviewed.text.as_deref()), (Style::Body, Status::Reviewed, Some("我定稿的正文")));
        assert_eq!(reviewed.drafts.len(), 1, "草稿照收，作为建议");

        let mut pending = entry(None, Style::Body);
        patch_entry(&mut pending, &draft_body("查作者", "bullet"), 5).unwrap();
        assert_eq!((pending.style, pending.status), (Style::Bullet, Status::Draft));
    }

    #[test]
    fn drafts_are_capped_newest_first_and_terminal_is_refused() {
        let mut e = entry(None, Style::Body);
        for i in 0..MAX_DRAFTS + 5 {
            patch_entry(&mut e, &draft_body(&format!("第{i}份"), "body"), i as u64).unwrap();
        }
        assert_eq!(e.drafts.len(), MAX_DRAFTS);
        assert_eq!(e.drafts[0].text, format!("第{}份", MAX_DRAFTS + 4), "最新的在前");
        e.status = Status::Archived;
        assert!(patch_entry(&mut e, &serde_json::json!({"text": "x"}), 99).is_err());
    }
}
