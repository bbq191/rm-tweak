//! 一轮转写的编排（纯逻辑，依赖全是 trait，可用内存桩单测）：
//! 列书 → 每本取条目 → 挑 `needs_transcribe`（或指定的一条强制）→ 取裁图 → 视觉模型 → 草稿写回（行首标记兜底判样式）。
//! 失败记在 `Failures`（同指纹超过 `max_attempts` 不再自动重试，网页可清）；每轮最多 `max_per_run` 条。
use crate::backend::Vision;
use crate::config::TranscribeConfig;
use crate::ink::EntryStore;
use crate::ledger::{Ledger, RunReport};
use notecore::marker::split_leading_marker;
use notecore::model::{Draft, Style};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use vendorcfg::VendorConfig;

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Failure {
    pub book: String,
    pub id: String,
    pub hash: String,
    pub attempts: u32,
    pub error: String,
    pub at: u64,
}

#[derive(Default)]
pub struct Failures(Mutex<HashMap<String, Failure>>);

impl Failures {
    fn key(uuid: &str, id: &str) -> String {
        format!("{uuid}/{id}")
    }
    /// 同指纹已达上限 → 不再自动试。
    pub fn exhausted(&self, uuid: &str, id: &str, hash: &str, max: u32) -> bool {
        rmsvc_core::sync::lock(&self.0).get(&Self::key(uuid, id)).map(|f| f.hash == hash && f.attempts >= max).unwrap_or(false)
    }
    pub fn note(&self, uuid: &str, id: &str, hash: &str, err: &str, at: u64) {
        let mut m = rmsvc_core::sync::lock(&self.0);
        let f = m.entry(Self::key(uuid, id)).or_insert_with(|| Failure { book: uuid.into(), id: id.into(), hash: hash.into(), attempts: 0, error: String::new(), at });
        if f.hash != hash {
            f.hash = hash.into();
            f.attempts = 0;
        }
        f.attempts += 1;
        f.error = err.chars().take(200).collect();
        f.at = at;
    }
    pub fn clear_one(&self, uuid: &str, id: &str) {
        rmsvc_core::sync::lock(&self.0).remove(&Self::key(uuid, id));
    }
    pub fn clear(&self) {
        rmsvc_core::sync::lock(&self.0).clear();
    }
    pub fn list(&self) -> Vec<Failure> {
        let mut v: Vec<Failure> = rmsvc_core::sync::lock(&self.0).values().cloned().collect();
        v.sort_by_key(|f| std::cmp::Reverse(f.at));
        v
    }
}

/// 强制转写指定一条（忽略 needs_transcribe 与失败上限）。
pub struct Target<'a> {
    pub uuid: &'a str,
    pub id: &'a str,
}

pub struct Ctx<'a> {
    pub store: &'a dyn EntryStore,
    pub vision: &'a dyn Vision,
    pub cfg: &'a TranscribeConfig,
    pub ledger: &'a Ledger,
    pub failures: &'a Failures,
    pub now: u64,
}

/// 一条转写成功调用花的 token（点「重转」弹出消耗要用，2026-09-08 第三轮反馈）。文本本身已经在函数里
/// `post_draft` 写回条目库了，调用方不需要再要一份，这里只带调用方真正要用的数字。
struct Transcribed {
    prompt_tokens: u64,
    completion_tokens: u64,
}

/// 转写一条并写回；返回这次调用的 token 消耗。
fn transcribe_entry(c: &Ctx<'_>, uuid: &str, e: &notecore::model::Entry) -> Result<Transcribed, String> {
    let ink = e.ink.as_ref().ok_or("没有手写")?;
    if ink.crop.is_empty() {
        return Err("没有裁图".into());
    }
    let png = c.store.crop(uuid, &ink.crop)?;
    let prompt = crate::prompt::build(&c.cfg.prompt, e.quote.as_ref().map(|q| q.text.as_str()));
    // 用量账本只记模型调用本身：调用失败记一次失败；调用成功就记 token（钱已经花了），即使随后写回
    // ink-serve 失败也照记——此前写回失败会被当成一次模型失败记账、把已花的 token 丢掉，取不到裁图这类
    // 本地错误也会被算成模型调用失败（2026-09-24 第三轮审计）。
    let t = c.vision.transcribe(&png, &prompt).inspect_err(|err| c.ledger.record_fail(&c.cfg.usage_key(), err, c.now))?;
    c.ledger.record_ok(&c.cfg.usage_key(), t.prompt_tokens, t.completion_tokens, c.now);
    // 行首标记兜底：几何没认出来（仍是正文）时按转写结果认，并剥掉标记——可能认出内容样式（Style）
    // 也可能认出结构性标记（### 小节），见 `notecore::marker::Marker`。
    let (marker, text) = if e.style == Style::Body { split_leading_marker(&t.text) } else { (None, t.text.clone()) };
    let draft = Draft { text: text.clone(), backend: c.vision.name().to_string(), at: c.now, hash: ink.hash.clone() };
    c.store.post_draft(uuid, &e.id, &draft, marker)?;
    Ok(Transcribed { prompt_tokens: t.prompt_tokens, completion_tokens: t.completion_tokens })
}

/// 跑一轮。`only` 给定 → 只做那一条（强制）。一轮报告由调用方记账（`ledger::record_if_new`）。
pub fn run_once(c: &Ctx<'_>, only: Option<Target<'_>>) -> RunReport {
    let mut r = RunReport { at: c.now, ..Default::default() };
    let books = match c.store.list_books() {
        Ok(b) => b,
        Err(e) => {
            r.note = e;
            return r;
        }
    };
    'books: for b in books {
        if let Some(t) = &only {
            if t.uuid != b.uuid {
                continue;
            }
        } else if b.pending == 0 {
            continue;
        }
        let book = match c.store.book(&b.uuid) {
            Ok(x) => x,
            Err(e) => {
                r.note = e;
                continue;
            }
        };
        for e in &book.entries {
            let forced = only.as_ref().map(|t| t.id == e.id).unwrap_or(false);
            if only.is_some() && !forced {
                continue;
            }
            if !forced && !e.needs_transcribe() {
                continue;
            }
            // 终态守卫（2026-09-09 审计补）：needs_transcribe() 已经排除了 Mined/Skipped/Revoked/
            // Archived，但那只挡自动扫描；forced（「重新转写」按钮强制指定一条）原来完全绕过这条
            // 检查，能让已"跳过/撤销/删除"的条目被继续转写、静默拉回 Draft。
            if forced && e.is_terminal() {
                r.note = "这条已跳过/撤销/删除，不能转写".into();
                continue;
            }
            r.scanned += 1;
            let hash = e.ink.as_ref().map(|i| i.hash.as_str()).unwrap_or("");
            if !forced && c.failures.exhausted(&b.uuid, &e.id, hash, c.cfg.max_attempts) {
                r.skipped += 1;
                continue;
            }
            if r.done + r.failed >= c.cfg.max_per_run {
                r.left += 1;
                continue;
            }
            match transcribe_entry(c, &b.uuid, e) {
                Ok(t) => {
                    r.done += 1;
                    r.prompt_tokens += t.prompt_tokens;
                    r.completion_tokens += t.completion_tokens;
                    c.failures.clear_one(&b.uuid, &e.id);
                }
                Err(err) => {
                    r.failed += 1;
                    c.failures.note(&b.uuid, &e.id, hash, &err, c.now);
                    if forced {
                        r.note = err;
                    }
                    // key 错 / 连不上 → 这一轮别再一条条撞
                    if r.failed >= 3 && r.done == 0 {
                        r.note = "连续失败，本轮停止（看 lastError）".into();
                        break 'books;
                    }
                }
            }
            if c.cfg.pause_ms > 0 && r.done + r.failed < c.cfg.max_per_run {
                std::thread::sleep(std::time::Duration::from_millis(c.cfg.pause_ms));
            }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Fixed;
    use crate::ink::BookBrief;
    use notecore::marker::Marker;
    use notecore::model::{Book, Entry, Ink, Quote, Status};

    struct Mem {
        book: Mutex<Book>,
        posted: Mutex<Vec<(String, Draft, Option<Marker>)>>,
        fail_post: std::sync::atomic::AtomicBool,
    }
    impl EntryStore for Mem {
        fn list_books(&self) -> Result<Vec<BookBrief>, String> {
            let b = self.book.lock().unwrap();
            Ok(vec![BookBrief { uuid: b.uuid.clone(), pending: b.entries.iter().filter(|e| e.needs_transcribe()).count() }])
        }
        fn book(&self, _uuid: &str) -> Result<Book, String> {
            Ok(self.book.lock().unwrap().clone())
        }
        fn crop(&self, _uuid: &str, file: &str) -> Result<Vec<u8>, String> {
            if file == "missing.png" { Err("没有这张裁图".into()) } else { Ok(b"\x89PNG".to_vec()) }
        }
        fn post_draft(&self, _uuid: &str, id: &str, draft: &Draft, marker: Option<Marker>) -> Result<(), String> {
            if self.fail_post.load(std::sync::atomic::Ordering::Relaxed) {
                return Err("ink-serve 连不上".into());
            }
            self.posted.lock().unwrap().push((id.into(), draft.clone(), marker.clone()));
            let mut b = self.book.lock().unwrap();
            if let Some(e) = b.entries.iter_mut().find(|e| e.id == id) {
                e.drafts.insert(0, draft.clone());
                e.status = Status::Draft;
                if let Some(Marker::Style(s)) = marker { e.style = s; }
            }
            Ok(())
        }
    }
    fn entry(id: &str, hash: &str, crop: &str, quote: Option<&str>) -> Entry {
        Entry { id: id.into(), page: "p".into(), page_index: 0, chapter: None, chapter_title: String::new(), subhead: None, quote: quote.map(|q| Quote { id: "q".into(), text: q.into(), color: "y".into(), rects: vec![] }), ink: Some(Ink { strokes: vec![], bbox: (0.0, 0.0, 1.0, 1.0), hash: hash.into(), crop: crop.into() }), drafts: vec![], text: None, style: Style::Body, ask_ai: false, question: None, answer: None, status: Status::Pending, destination: Default::default(), source: Default::default(), created: 0, updated: 0 }
    }
    fn mem(entries: Vec<Entry>) -> Mem {
        Mem { book: Mutex::new(Book { uuid: "u".into(), title: "t".into(), entries, ..Default::default() }), posted: Mutex::new(vec![]), fail_post: Default::default() }
    }
    fn cfg() -> TranscribeConfig {
        TranscribeConfig { pause_ms: 0, max_per_run: 2, max_attempts: 2, ..Default::default() }
    }

    #[test]
    fn transcribes_pending_writes_draft_and_style() {
        let t = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(&t.path().join("l.json"));
        let store = mem(vec![entry("a", "h1", "a.png", Some("梭罗")), entry("b", "h2", "b.png", None), entry("c", "h3", "c.png", None)]);
        let vision = Fixed("1. 背诵".into());
        let f = Failures::default();
        let c = Ctx { store: &store, vision: &vision, cfg: &cfg(), ledger: &ledger, failures: &f, now: 9 };
        let r = run_once(&c, None);
        assert_eq!((r.scanned, r.done, r.failed, r.left), (3, 2, 0, 1), "max_per_run=2 剩 1: {r:?}");
        assert_eq!((r.prompt_tokens, r.completion_tokens), (20, 4), "两次成功调用（各 10/2，见 backend::Fixed）累加，点「重转」弹出消耗要用这两个数");
        let posted = store.posted.lock().unwrap().clone();
        assert_eq!(posted[0].1, Draft { text: "背诵".into(), backend: "fixed".into(), at: 9, hash: "h1".into() });
        assert_eq!(posted[0].2, Some(Marker::Style(Style::Numbered)), "行首 1. → 有序，且标记剥掉");
        assert_eq!(ledger.snapshot().by_model[&cfg().usage_key()].ok, 2);
        // 第二轮：只剩 c；a/b 已有同指纹草稿不重做
        let r = run_once(&c, None);
        assert_eq!((r.scanned, r.done, r.left), (1, 1, 0));
        let r = run_once(&c, None);
        assert_eq!(r.scanned, 0, "全部有草稿后零调用");
    }

    #[test]
    fn forced_single_entry_run_reports_only_that_calls_tokens() {
        // 「重转」按钮走的就是这条路：uuid+id 强制指定一条，成功后返回的 promptTokens/completionTokens
        // 只是这一次调用的消耗，不是整本书/整轮累加的——书里另一条待转写的（entry "b"）不该被碰。
        let t = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(&t.path().join("l.json"));
        let store = mem(vec![entry("a", "h1", "a.png", None), entry("b", "h2", "b.png", None)]);
        let vision = Fixed("答案".into());
        let f = Failures::default();
        let c = Ctx { store: &store, vision: &vision, cfg: &cfg(), ledger: &ledger, failures: &f, now: 1 };
        let r = run_once(&c, Some(Target { uuid: "u", id: "a" }));
        assert_eq!((r.scanned, r.done, r.prompt_tokens, r.completion_tokens), (1, 1, 10, 2), "只强制转这一条，token 也只是这一条的: {r:?}");
        assert_eq!(store.posted.lock().unwrap().len(), 1, "entry b 没被顺带转写");
    }

    #[test]
    fn mined_and_skipped_entries_are_never_auto_transcribed() {
        // 2026-09-07 二期：ink-serve 摄取的初始态是 Mined（只是探测到），不是 Pending——
        // needs_transcribe() 只认真正被请求过的条目，自动转写不该碰它们，除非用户点了"转入笔记"。
        let t = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(&t.path().join("l.json"));
        let mut mined = entry("m", "h1", "m.png", None);
        mined.status = Status::Mined;
        let mut skipped = entry("s", "h2", "s.png", None);
        skipped.status = Status::Skipped;
        let store = mem(vec![mined, skipped]);
        let vision = Fixed("不该被调用".into());
        let f = Failures::default();
        let c = Ctx { store: &store, vision: &vision, cfg: &cfg(), ledger: &ledger, failures: &f, now: 9 };
        let r = run_once(&c, None);
        assert_eq!(r.scanned, 0, "Mined/Skipped 都不算 needs_transcribe，一个都不该扫到");
        assert!(store.posted.lock().unwrap().is_empty());

        // 转成 Pending（模拟用户点了"转入笔记"）之后才应该被扫到。
        store.book.lock().unwrap().entries[0].status = Status::Pending;
        let r = run_once(&c, None);
        assert_eq!(r.scanned, 1, "转成 Pending 后这条才进入扫描范围");
        assert_eq!(r.done, 1);
    }

    #[test]
    fn forced_retry_on_terminal_entry_is_rejected_not_silently_transcribed() {
        // 2026-09-09 审计补：forced（「重新转写」按钮强制指定一条）原来完全绕过 needs_transcribe()，
        // 能让已"跳过/撤销/删除"的条目被继续转写、静默拉回 Draft——终态条目该先恢复才能再操作。
        let t = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(&t.path().join("l.json"));
        let mut archived = entry("a", "h1", "a.png", None);
        archived.status = Status::Archived;
        let store = mem(vec![archived]);
        let vision = Fixed("不该被调用".into());
        let f = Failures::default();
        let c = Ctx { store: &store, vision: &vision, cfg: &cfg(), ledger: &ledger, failures: &f, now: 9 };
        let r = run_once(&c, Some(Target { uuid: "u", id: "a" }));
        assert_eq!((r.scanned, r.done, r.failed), (0, 0, 0), "终态条目不该被扫到，也不该调用模型");
        assert!(r.note.contains("跳过/撤销/删除"), "{}", r.note);
        assert!(store.posted.lock().unwrap().is_empty(), "没有草稿被写回");
    }

    #[test]
    fn failures_are_bounded_and_forced_retry_bypasses() {
        let t = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(&t.path().join("l.json"));
        let store = mem(vec![entry("a", "h1", "missing.png", None)]);
        let vision = Fixed("x".into());
        let f = Failures::default();
        let c = Ctx { store: &store, vision: &vision, cfg: &cfg(), ledger: &ledger, failures: &f, now: 1 };
        assert_eq!(run_once(&c, None).failed, 1);
        assert_eq!(run_once(&c, None).failed, 1);
        let r = run_once(&c, None);
        assert_eq!((r.failed, r.skipped), (0, 1), "两次后不再自动重试");
        assert_eq!(f.list()[0].attempts, 2);
        let r = run_once(&c, Some(Target { uuid: "u", id: "a" }));
        assert_eq!(r.failed, 1, "强制仍会试（并报错）");
        assert!(r.note.contains("裁图"));
        // 指纹变了 → 计数归零重试
        store.book.lock().unwrap().entries[0].ink.as_mut().unwrap().hash = "h9".into();
        assert_eq!(run_once(&c, None).failed, 1);
        f.clear();
        assert!(f.list().is_empty());
    }

    /// 账本只记模型调用：取不到裁图（没调模型）不记；模型成功但写回失败，token 照记为成功、不另记失败。
    #[test]
    fn ledger_counts_model_calls_only() {
        let t = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(&t.path().join("l.json"));
        let store = mem(vec![entry("a", "h1", "missing.png", None), entry("b", "h2", "b.png", None)]);
        store.fail_post.store(true, std::sync::atomic::Ordering::Relaxed);
        let vision = Fixed("x".into());
        let f = Failures::default();
        let c = Ctx { store: &store, vision: &vision, cfg: &cfg(), ledger: &ledger, failures: &f, now: 1 };
        let r = run_once(&c, None);
        assert_eq!((r.done, r.failed), (0, 2), "两条都没写成：一条缺裁图、一条写回失败");
        let m = &ledger.snapshot().by_model[&cfg().usage_key()];
        assert_eq!((m.calls, m.ok, m.failed, m.prompt_tokens), (1, 1, 0, 10), "只有 b 真调了模型，花的 token 记下");
        assert_eq!(f.list().len(), 2, "两条都进失败清单，按上限重试");
    }

    #[test]
    fn stops_after_consecutive_failures() {
        let t = tempfile::tempdir().unwrap();
        let ledger = Ledger::open(&t.path().join("l.json"));
        let store = mem((0..6).map(|i| entry(&format!("e{i}"), "h", "x.png", None)).collect());
        let vision = Fixed("!fail".into());
        let f = Failures::default();
        let big = TranscribeConfig { max_per_run: 50, ..cfg() };
        let c = Ctx { store: &store, vision: &vision, cfg: &big, ledger: &ledger, failures: &f, now: 1 };
        let r = run_once(&c, None);
        assert_eq!(r.failed, 3, "连续 3 次失败即停: {r:?}");
        assert!(r.note.contains("停止"));
        assert_eq!(ledger.snapshot().by_model[&big.usage_key()].last_error, "模拟失败");
    }
}
