//! 投原生后的**渲染自检**：xochitl 导入 EPUB 后渲染（真机：导入当下同步渲染），渲染完在 `<uuid>.content` 写 `pageCount`。
//! 整章渲染失败（同一标签双 id 等，《消失的爱人》只出 7 页）以前要用户翻到才发现；现在投书后起一条线程，
//! **限时**监听书库目录（`fswatch::watch_until`，最长 [`TIMEOUT`]，结束即撤、不常驻），等到页数就与
//! `bookconv::stats` 的期望页数比：低于 [`WARN_RATIO`] → `warn`。结果写进母版库边车 `.<name>.delivered` 的
//! `render` 字段（事件是有损信号，状态必须落盘），并推 `books/render` 事件（带 name/status/pages）。
//! 认书：`/upload` 不回 uuid，visibleName 取自 EPUB 元数据不等于文件名 → 按 `createdTime >= 投书时刻` 圈候选，
//! 书名（dc:title / 文件名 stem）相符者优先，否则取最新一本。只读 `.metadata/.content`，绝不写 xochitl 目录。
use crate::sidecar::RenderCheck;
use crate::staging::{RenderPlan, Staging};
use rmsvc_core::clock::now_secs as now;
use rmsvc_core::events::EventBus;
use rmsvc_core::fswatch::watch_until;
use rmsvc_core::xochitl::{find_documents_since, page_count, DocInfo};
use std::path::Path;
use std::time::Duration;

/// 等渲染的上限；超时状态 `timeout`（不是错误：xochitl 可能延后渲染，设备上打开一次就有）。
pub const TIMEOUT: Duration = Duration::from_secs(600);
/// 书库目录写入防抖（xochitl 导入时连写 metadata/content/缩略图）。
pub const DEBOUNCE: Duration = Duration::from_secs(3);
/// 实际页数 / 期望页数低于此 → warn。自检发生在导入当下，xochitl 用的是缺省字号/边距（`.content` 里 textScale 1 /
/// margins 56），页数只随书的文字密度浮动（真机：两本真书 0.99，随机词探针 0.86）；坏章在 xochitl 里各占 1 页空白，
/// 4 章坏 3 章的探针 10/29=0.34——30% 抓不住，定 50%（2026-09-06 真机标定）。
pub const WARN_RATIO: f64 = 0.5;

pub fn run(staging: &Staging, bus: &EventBus, lib_dir: &Path, plan: &RenderPlan) {
    run_with(staging, bus, lib_dir, plan, DEBOUNCE, TIMEOUT)
}

/// 同 [`run`]，防抖与总时限可调（单测用毫秒级值，不真等 3 秒 / 10 分钟）。
pub fn run_with(staging: &Staging, bus: &EventBus, lib_dir: &Path, plan: &RenderPlan, debounce: Duration, timeout: Duration) {
    let write = |uuid: &str, pages: u64, status: &str| {
        // 用户在自检期间把书从母版库删了（投完就删很常见）：结果没处可记，静默跳过，不当成失败刷日志。
        if !staging.has(&plan.name) {
            return;
        }
        let rc = RenderCheck { uuid: uuid.to_string(), pages, expected: plan.expected, status: status.to_string(), at: now() };
        if let Err(e) = staging.set_render(&plan.name, rc) {
            println!("[book-serve] 渲染自检记录《{}》失败: {e}", plan.name);
        }
        bus.publish_raw(&serde_json::json!({"area":"books","kind":"render","name":plan.name,"status":status,"pages":pages,"expected":plan.expected,"at":now()}).to_string());
        println!("[book-serve] 渲染自检《{}》: {status} pages={pages} expected={} uuid={uuid}", plan.name, plan.expected);
    };
    write("", 0, "pending");
    let check = || {
        probe(lib_dir, plan).map(|(uuid, pages)| {
            if plan.comic {
                staging.register_comic_margins(&uuid, &plan.name);
            }
            write(&uuid, pages, verdict(pages, plan.expected))
        })
    };
    if check().is_some() {
        return;
    }
    let found = watch_until(lib_dir, debounce, timeout, |_| check().is_some());
    if !found && check().is_none() {
        write("", 0, "timeout");
    }
}

/// 在书库里找这本刚投的书并读页数；没渲染完 → None。
pub fn probe(lib_dir: &Path, plan: &RenderPlan) -> Option<(String, u64)> {
    let docs = find_documents_since(lib_dir, plan.since_ms);
    let doc = pick(&docs, plan.title.as_deref(), &plan.name)?;
    page_count(lib_dir, &doc.uuid).map(|n| (doc.uuid.clone(), n))
}

/// 候选（新→旧）里挑：visibleName 与 dc:title / 文件名 stem 相符（忽略大小写）者优先，否则最新一本。
pub fn pick<'a>(docs: &'a [DocInfo], title: Option<&str>, name: &str) -> Option<&'a DocInfo> {
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    let eq = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    docs.iter().find(|d| title.is_some_and(|t| eq(&d.visible_name, t)) || eq(&d.visible_name, stem)).or(docs.first())
}

pub fn verdict(pages: u64, expected: u64) -> &'static str {
    if expected > 0 && (pages as f64) < (expected as f64) * WARN_RATIO {
        "warn"
    } else {
        "ok"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(uuid: &str, name: &str, t: u64) -> DocInfo {
        DocInfo { uuid: uuid.into(), visible_name: name.into(), created_ms: t }
    }

    use crate::sidecar;
    use rmsvc_core::xochitl::Xochitl;
    use std::io::Read;
    use std::sync::Arc;

    /// 母版库里有 a.epub，书库目录 `xochitl/`（空）；返回 (staging, 书库目录)。
    fn setup(t: &tempfile::TempDir) -> (Staging, std::path::PathBuf) {
        let lib = t.path().join("xochitl");
        std::fs::create_dir_all(&lib).unwrap();
        let x = Arc::new(Xochitl::new("127.0.0.1:1", &lib, 1));
        let s = Staging::new(t.path().join("staging"), x, 1024 * 1024);
        s.ensure().unwrap();
        s.stage_new("a.epub", b"PK").unwrap();
        (s, lib)
    }

    /// 造一份"xochitl 已渲染完"的文档：`.metadata`（createdTime 晚于投书时刻）+ `.content`（pageCount）。
    fn render_doc(lib: &Path, uuid: &str, name: &str, pages: u64) {
        std::fs::write(lib.join(format!("{uuid}.metadata")), format!(r#"{{"type":"DocumentType","visibleName":"{name}","parent":"","createdTime":"5000"}}"#)).unwrap();
        std::fs::write(lib.join(format!("{uuid}.content")), format!(r#"{{"pageCount":{pages}}}"#)).unwrap();
    }

    fn plan(expected: u64) -> RenderPlan {
        RenderPlan { name: "a.epub".into(), title: None, expected, since_ms: 1000, comic: false }
    }

    const MS: fn(u64) -> Duration = Duration::from_millis;

    fn render_of(s: &Staging) -> Option<RenderCheck> {
        sidecar::read(&s.dir().join("a.epub")).and_then(|d| d.render)
    }

    #[test]
    fn run_records_ok_when_already_rendered_and_publishes_events() {
        let t = tempfile::tempdir().unwrap();
        let (s, lib) = setup(&t);
        render_doc(&lib, "u1", "a", 100);
        let bus = EventBus::new();
        let mut sub = bus.subscribe();
        run_with(&s, &bus, &lib, &plan(100), MS(20), MS(200));
        let rc = render_of(&s).unwrap();
        assert_eq!((rc.status.as_str(), rc.pages, rc.uuid.as_str(), rc.expected), ("ok", 100, "u1", 100));
        let mut buf = [0u8; 1024];
        let n = sub.read(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains(r#""kind":"render""#), "应推 books/render 事件");
    }

    #[test]
    fn run_registers_margins_only_for_comic_plans() {
        // 新版管线的漫画：导入完成（找到 uuid）后登记"首次打开时设页边距"；文字书不登记。
        const U: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
        for (comic, expect) in [(true, Some(bookconv::imgopt::EPUB_COMIC_MARGINS)), (false, None)] {
            let t = tempfile::tempdir().unwrap();
            let (s, lib) = setup(&t);
            std::fs::write(t.path().join("qol.json"), r#"{"comicMinMargin":true}"#).unwrap();
            let q = std::sync::Arc::new(crate::comic_margins::ComicMargins::new(t.path(), &lib, &t.path().join("qol.json")));
            let s = s.with_comic_margins(q.clone());
            render_doc(&lib, U, "a", 100);
            let mut p = plan(100);
            p.comic = comic;
            run_with(&s, &EventBus::new(), &lib, &p, MS(20), MS(200));
            assert_eq!(q.get(U), expect, "comic={comic}");
        }
    }

    #[test]
    fn run_records_warn_when_pages_far_below_expected() {
        let t = tempfile::tempdir().unwrap();
        let (s, lib) = setup(&t);
        render_doc(&lib, "u1", "a", 100);
        run_with(&s, &EventBus::new(), &lib, &plan(300), MS(20), MS(200));
        assert_eq!(render_of(&s).unwrap().status, "warn", "100/300 < 50% → 整章渲染失败嫌疑");
    }

    #[test]
    fn run_records_timeout_when_book_never_appears() {
        let t = tempfile::tempdir().unwrap();
        let (s, lib) = setup(&t);
        let started = std::time::Instant::now();
        run_with(&s, &EventBus::new(), &lib, &plan(100), MS(20), MS(150));
        let rc = render_of(&s).unwrap();
        assert_eq!((rc.status.as_str(), rc.pages), ("timeout", 0));
        assert!(started.elapsed() < Duration::from_secs(5), "限时监听按给定 timeout 收工");
    }

    #[test]
    fn run_picks_up_book_that_renders_after_check_started() {
        let t = tempfile::tempdir().unwrap();
        let (s, lib) = setup(&t);
        let lib2 = lib.clone();
        std::thread::spawn(move || {
            std::thread::sleep(MS(200));
            render_doc(&lib2, "u2", "a", 42);
        });
        run_with(&s, &EventBus::new(), &lib, &plan(40), MS(30), Duration::from_secs(10));
        let rc = render_of(&s).unwrap();
        assert_eq!((rc.status.as_str(), rc.pages, rc.uuid.as_str()), ("ok", 42, "u2"), "watch_until 应在书出现后很快检出");
    }

    #[test]
    fn run_skips_silently_when_book_deleted_during_check() {
        // 投完就删很常见：结果没处可记，静默跳过——不 panic、不复活 sidecar、不推事件。
        let t = tempfile::tempdir().unwrap();
        let (s, lib) = setup(&t);
        render_doc(&lib, "u1", "a", 100);
        s.remove("a.epub").unwrap();
        let bus = EventBus::new();
        let _sub = bus.subscribe();
        run_with(&s, &bus, &lib, &plan(100), MS(20), MS(100));
        assert!(sidecar::read(&s.dir().join("a.epub")).is_none(), "书已删，不该再生成边车");
        assert!(!s.has("a.epub"));
    }

    #[test]
    fn picks_by_title_then_stem_then_newest() {
        let docs = vec![d("n", "Other", 3), d("t", "tell me your dreams", 2), d("s", "Tell Me Your Dreams (v6)", 1)];
        assert_eq!(pick(&docs, Some("Tell Me Your Dreams"), "Tell Me Your Dreams (v6).epub").unwrap().uuid, "t", "dc:title 优先（忽略大小写）");
        assert_eq!(pick(&docs, None, "Tell Me Your Dreams (v6).epub").unwrap().uuid, "s", "退而按文件名 stem");
        assert_eq!(pick(&docs, Some("没有的"), "x.epub").unwrap().uuid, "n", "都不符取最新");
        assert!(pick(&[], Some("a"), "a.epub").is_none());
    }

    #[test]
    fn verdict_thresholds() {
        assert_eq!(verdict(7, 300), "warn");
        assert_eq!(verdict(10, 29), "warn", "真机探针：4 章坏 3 章");
        assert_eq!(verdict(149, 300), "warn");
        assert_eq!(verdict(150, 300), "ok");
        assert_eq!(verdict(25, 29), "ok", "真机探针：好书 0.86");
        assert_eq!(verdict(600, 300), "ok");
        assert_eq!(verdict(1, 0), "ok", "没期望值不判");
    }
}
