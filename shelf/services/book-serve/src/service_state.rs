//! 服务组合根：配置、inbox 队列、母版库、xochitl 客户端；inbox 追平处理。
use crate::agent_failures::AgentFailures;
use crate::config::BookConfig;
use crate::mkdir::MkdirQueue;
use crate::spool::Spool;
use crate::staging::{self, Staging};
use crate::comic_margins::ComicMargins;
use crate::trash::TrashQueue;
use serde::Serialize;
use rmsvc_core::cache::TtlCache;
use rmsvc_core::events::EventBus;
use rmsvc_core::formats::{self, BOOK_EXTS};
use rmsvc_core::paths::Paths;
use rmsvc_core::xochitl::Xochitl;
use std::sync::Arc;
use std::time::Duration;

pub struct State {
    pub cfg: BookConfig,
    pub spool: Spool,
    pub staging: Staging,
    pub xochitl: Arc<Xochitl>,
    /// 事件总线：母版库/inbox 每次变更发一条，网关汇聚推给网页（零轮询）。
    pub bus: Arc<EventBus>,
    /// 原生书库「移进回收站」队列（QML 代理 shelf-trash-agent.qmd 拉取执行）。
    pub trash: TrashQueue,
    /// 漫画「页边距」待办（QML 代理 shelf-comic-margins.qmd 在书打开时查、设完销账，见 comic_margins.rs）。
    pub comic_margins: Arc<ComicMargins>,
    /// 原生书库「建文件夹」队列（QML 代理 shelf-mkdir-agent.qmd 拉取执行，2026-09-19 复活，
    /// 见 mkdir.rs 模块文档）；`Arc` 是因为 `Staging::deliver` 的后台线程要跟 `bus` 一样带着走。
    pub mkdir: Arc<MkdirQueue>,
    /// 阅读方向查询（xochitl 阅读器里的 reader-page-turn.qmd 打开书时问，见 reading_direction.rs）。
    pub reading_direction: Arc<crate::reading_direction::ReadingDirection>,
    /// 回收站 / 建文件夹代理执行不成、已放弃的记录（网页页头横幅，见 agent_failures.rs）。
    pub agent_failures: Arc<AgentFailures>,
    /// `GET /status` 的结果缓存（[`STATUS_TTL`]）。网页每次 refresh 都会打这个接口，而它里面有重活：
    /// 对 xochitl 发 HTTP 探活（不可达时要等满 3 秒超时）、读全部 `.metadata` 列文件夹、扫 inbox。
    /// 会被本服务自己的操作改变的部分（inbox 计数、文件夹候选）在操作路径里 [`State::invalidate_status`]
    /// 主动失效；xochitl 是否可达、用户在设备上新建文件夹这类外部变化最多滞后一个 TTL。
    status_cache: TtlCache<serde_json::Value>,
    /// inbox 文件修改时间静止多久才算"写完了"（见 [`State::process_inbox_counting_deferred`]）。缺省 [`INBOX_SETTLE`]，
    /// 须小于 inbox 监听的防抖时长（8 秒），写完那次事件触发的追平才不会再被暂缓。
    pub inbox_settle: Duration,
}

/// 见 [`State::inbox_settle`]。
pub const INBOX_SETTLE: Duration = Duration::from_secs(5);

/// `/status` 缓存时长：够挡住"连续几次 refresh"，又短到外部变化（xochitl 上下线）几秒内就能看到。
const STATUS_TTL: Duration = Duration::from_secs(3);

/// inbox 追平一项的结果（日志）。
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct InboxOutcome {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

impl State {
    pub fn new(paths: &Paths) -> State {
        let cfg = BookConfig::load(paths);
        let xochitl = Arc::new(Xochitl::new(&cfg.xochitl_host, &paths.xochitl_dir(), cfg.upload_timeout_secs));
        let books_state = paths.state_dir().join("books"); // inbox/.work/failed 与三个待办队列共用的状态目录
        let spool = Spool::new(books_state.clone());
        let qol_file = paths.home().join(".local/share/cangjie-ime/reading-qol.json"); // 与网关共享的开关文件（gateway 写、这里读）
        let comic_margins = Arc::new(ComicMargins::new(&books_state, &paths.xochitl_dir(), &qol_file));
        // 阅读方向手动清单：xochitl 阅读器查询（只读），母版库按书设方向时同步写（见 staging/direction.rs）。
        let reading_direction = Arc::new(crate::reading_direction::ReadingDirection::new(&paths.xochitl_dir(), &books_state.join("rtl-overrides.json")));
        let staging = Staging::new(paths.staging_dir(), xochitl.clone(), cfg.native_upload_limit_bytes())
            .with_comic_margins(comic_margins.clone())
            .with_reading_direction(reading_direction.clone());
        let bus = Arc::new(EventBus::new());
        let agent_failures = Arc::new(AgentFailures::new(&books_state, Some(bus.clone())));
        let trash = TrashQueue::new(&books_state, &paths.xochitl_dir()).with_failures(agent_failures.clone());
        let mkdir = Arc::new(MkdirQueue::new(&books_state, &paths.xochitl_dir()).with_failures(agent_failures.clone()));
        State { cfg, spool, staging, xochitl, bus, trash, comic_margins, mkdir, reading_direction, agent_failures, status_cache: TtlCache::new(STATUS_TTL), inbox_settle: INBOX_SETTLE }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        self.spool.ensure()?;
        self.staging.ensure()?;
        let stale_margins = self.comic_margins.prune_missing();
        if stale_margins > 0 {
            println!("[book-serve] 清掉 {stale_margins} 条书已不在库里的页边距待办");
        }
        let (fixed, tmps) = self.staging.recover_interrupted();
        if fixed > 0 || tmps > 0 {
            println!("[book-serve] 修正 {fixed} 条上次被中断的处理记录，清掉 {tmps} 个优化半成品");
        }
        let orphans = self.staging.gc_orphan_sidecars();
        if orphans > 0 {
            println!("[book-serve] 清掉 {orphans} 个没有对应书的落库记录");
        }
        let old_pdfs = self.staging.gc_pdf_originals(crate::staging::PDF_ORIGINALS_KEEP_SECS);
        if old_pdfs > 0 {
            println!("[book-serve] 清掉 {old_pdfs} 份过期的原 PDF 备份");
        }
        // 给"已加入 xochitl 但没有渲染记录"的书补记（大文件通道上线前直接投入的），让列表里渲染徽章统一。幂等。
        let n = self.staging.backfill_render_records();
        if n > 0 {
            println!("[book-serve] 补记 {n} 本已加入 xochitl 的书的渲染记录");
        }
        Ok(())
    }

    /// 让下一次 `/status` 必定重算（改变了 inbox 计数 / 文件夹候选的操作完成后调）。
    pub fn invalidate_status(&self) {
        self.status_cache.invalidate();
    }

    pub fn status(&self) -> serde_json::Value {
        self.status_cache.get_or(|| self.compute_status())
    }

    fn compute_status(&self) -> serde_json::Value {
        let items = self.spool.list();
        serde_json::json!({
            "ok": true,
            "uploadReachable": self.xochitl.reachable(),
            // 原生书库里真实存在的文件夹名（去重排序），给网页「加入原生书库 → 文件夹」下拉候选用——
            // 2026-09-19 取代原来写死的「书库/批注/自定义」三选一预设（`annotFolder` 已删），跟
            // KOReader 那边的目录下拉候选（`GET /koreader/books` 过滤 `kind==='dir'`）同一个道理。
            "xochitlFolders": rmsvc_core::xochitl::list_folders(self.xochitl.library_dir()),
            // 投原生的体积门（字节），网页据此灰掉超限书的「投入原生书库」
            "nativeUploadLimitBytes": self.cfg.native_upload_limit_bytes(),
            "spool": {
                "pending": items.iter().filter(|i| i.state == "pending").count(),
                "failed": items.iter().filter(|i| i.state == "failed").count(),
            },
        })
    }

    /// 处理 inbox（scp 丢进来的 / 重试的）：**原样落母版库**（与网页/CLI 同一规则：所有书只落母版库，去向在网页选）。
    /// 非书籍格式进 failed/ 带原因、不反复重试。`only`=只处理该文件。
    pub fn process_inbox(&self, only: Option<&str>) -> Vec<InboxOutcome> {
        self.process_inbox_counting_deferred(only).0
    }

    /// 同 [`Self::process_inbox`]，另返回因"还在写"（修改时间离现在不足 [`State::inbox_settle`]）而暂缓的文件数。
    ///
    /// **正在写的文件不动**（2026-09-25 第四轮审计）：scp 直接往最终文件名里写，一建出文件就有 CREATE 事件，防抖 8 秒后
    /// 追平——WiFi 传大书超过 8 秒时，这里会把还在写的文件认领、改名进母版库，写入者手里的 fd 跟着 inode 继续写，
    /// 母版库里于是出现一本半截书（可被优化/落库，列表判定按半截内容缓存）。写完时的 CLOSE_WRITE 会再触发一轮追平，
    /// 那时修改时间已经静止超过防抖时长，照常处理。
    pub fn process_inbox_counting_deferred(&self, only: Option<&str>) -> (Vec<InboxOutcome>, usize) {
        let _g = self.spool.guard();
        let mut out = Vec::new();
        let mut deferred = 0;
        let Ok(rd) = std::fs::read_dir(self.spool.inbox()) else { return (out, 0) };
        let now = std::time::SystemTime::now();
        for e in rd.flatten() {
            let p = e.path();
            let Some(name) = p.file_name().and_then(|s| s.to_str()).map(|s| s.to_string()) else { continue };
            if !p.is_file() || only.map(|o| o != name).unwrap_or(false) || name.starts_with('.') {
                continue; // 半成品不动
            }
            let fresh = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|m| now.duration_since(m).ok()).is_some_and(|age| age < self.inbox_settle);
            if fresh {
                deferred += 1;
                continue;
            }
            let Some(work) = self.spool.claim(&name) else { continue };
            let res = if formats::has_ext(&name, BOOK_EXTS) { self.staging.stage_from_path(&name, &work) } else { Err(staging::reject_message()) };
            let o = match res {
                Ok(landed) => InboxOutcome { name: landed, ok: true, message: "已入母版库".into() },
                Err(e) => {
                    self.spool.archive_failed(&work, &e);
                    InboxOutcome { name: name.clone(), ok: false, message: e }
                }
            };
            println!("[book-serve] inbox {} → {}: {}", name, if o.ok { "ok" } else { "fail" }, o.message);
            out.push(o);
        }
        if !out.is_empty() {
            self.invalidate_status(); // inbox 计数变了，先失效再发事件（网页收到事件后马上来取 /status）
            self.bus.publish("books", "inbox");
            if out.iter().any(|o| o.ok) {
                self.bus.publish("books", "staging");
            }
        }
        (out, deferred)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inbox_lands_books_and_fails_non_books() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        let mut st = State::new(&paths);
        st.inbox_settle = Duration::ZERO;
        st.ensure_dirs().unwrap();
        // 2026-09-18 起母版库只收 EPUB/PDF（cbz 已随"仅 KOReader"档退役），接受项夹具改用 .epub。
        std::fs::write(st.spool.inbox().join("b.epub"), b"x").unwrap();
        std::fs::write(st.spool.inbox().join("p.jpg"), b"x").unwrap();
        let out = st.process_inbox(None);
        assert_eq!(out.len(), 2);
        assert!(out.iter().any(|o| o.ok && o.name == "b.epub"));
        assert!(out.iter().any(|o| !o.ok && o.name == "p.jpg" && o.message.contains("不是书籍格式")));
        assert!(st.staging.dir().join("b.epub").is_file() && !st.spool.inbox().join("b.epub").exists());
        assert_eq!(st.spool.list().iter().filter(|e| e.state == "failed").count(), 1);
    }

    /// 造一个 xochitl 指向本机关闭端口（连接秒拒，不会真等 3 秒超时）的 State。
    fn state_with_dead_xochitl(home: &std::path::Path) -> State {
        let h = home.to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        let cfg = paths.service_config("book");
        std::fs::create_dir_all(cfg.parent().unwrap()).unwrap();
        std::fs::write(&cfg, r#"{"xochitlHost":"127.0.0.1:9"}"#).unwrap();
        let mut st = State::new(&paths);
        st.inbox_settle = Duration::ZERO;
        st.ensure_dirs().unwrap();
        st
    }

    #[test]
    fn status_is_cached_within_ttl_and_invalidated_by_inbox_operations() {
        let t = tempfile::tempdir().unwrap();
        let st = state_with_dead_xochitl(t.path());
        assert_eq!(st.status()["spool"]["failed"], 0);
        // 外部（scp）丢进 inbox 一个非书文件：TTL 内 status 仍是缓存的旧值（不重算重活）
        std::fs::write(st.spool.inbox().join("p.jpg"), b"x").unwrap();
        assert_eq!(st.status()["spool"]["pending"], 0, "TTL 内命中缓存");
        // 本服务自己处理 inbox → 主动失效，马上看到 failed=1
        st.process_inbox(None);
        let s = st.status();
        assert_eq!((s["spool"]["pending"].as_u64(), s["spool"]["failed"].as_u64()), (Some(0), Some(1)), "操作后立刻刷新");
        assert_eq!(s["uploadReachable"], false);
        // 外部把失败项清掉后，失效钩子生效前仍是缓存值（api 层的 invalidate_status 就是这个钩子）
        std::fs::remove_file(st.spool.failed().join("p.jpg")).unwrap();
        assert_eq!(st.status()["spool"]["failed"], 1, "没失效前仍是缓存值");
        st.invalidate_status();
        assert_eq!(st.status()["spool"]["failed"], 0);
    }

    /// 回归：还在写的文件（修改时间离现在不足 settle）不认领，写完静止后照常入库。
    #[test]
    fn inbox_defers_files_still_being_written() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        let st = State::new(&paths);
        st.ensure_dirs().unwrap();
        let f = st.spool.inbox().join("scp.epub");
        std::fs::write(&f, b"half").unwrap();
        let (out, deferred) = st.process_inbox_counting_deferred(None);
        assert!(out.is_empty() && deferred == 1, "刚写的文件暂缓");
        assert!(f.exists() && !st.staging.has("scp.epub"));
        let old = std::time::SystemTime::now() - INBOX_SETTLE - Duration::from_secs(1);
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(old).unwrap();
        let (out, deferred) = st.process_inbox_counting_deferred(None);
        assert_eq!((out.len(), deferred), (1, 0));
        assert!(st.staging.has("scp.epub"));
    }

    #[test]
    fn inbox_rejects_retired_host_convertible_exts() {
        // 2026-09-17 EPUB 线架构调整：azw3/mobi/fb2/txt 不再自动转 EPUB，母版库直接拒收。
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        let mut st = State::new(&paths);
        st.inbox_settle = Duration::ZERO;
        st.ensure_dirs().unwrap();
        for name in ["b.azw3", "b.mobi", "b.fb2", "b.txt"] {
            std::fs::write(st.spool.inbox().join(name), b"x").unwrap();
        }
        let out = st.process_inbox(None);
        assert!(out.iter().all(|o| !o.ok && o.message.contains("不是书籍格式")), "{out:?}");
    }
}
