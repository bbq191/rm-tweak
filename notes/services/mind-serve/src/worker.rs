//! 按条目单发问答（纯逻辑，依赖全是 trait，可用内存桩单测）：取条目 → 拼提示词 → 文字模型 → 写回 `answer`。
//! 跟 transcribe-serve 的批量循环不同，这里没有"扫一轮待处理条目"这回事——每次调用只处理调用方指定的
//! 那一条，同步做完同步返回，不需要 `Failures`/`max_attempts` 那一套（失败了网页直接看到错误，重新点一次
//! 就是重试，不用服务端记账）。
use crate::backend::TextModel;
use crate::config::MindConfig;
use crate::ink::EntryStore;
use crate::ledger::Ledger;
use notecore::model::{Answer, Entry};

pub struct Ctx<'a> {
    pub store: &'a dyn EntryStore,
    pub model: &'a dyn TextModel,
    pub cfg: &'a MindConfig,
    pub ledger: &'a Ledger,
    pub now: u64,
}

/// 一次问答的结果：文本 + 这次调用花的 token（点「提问」弹出消耗要用，2026-09-08 第三轮反馈，跟
/// transcribe-serve 的 `Transcribed` 同一个理由）。
#[derive(Debug)]
pub struct Answered {
    pub text: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// 回答一条的问题并写回；返回回答文本 + token 消耗。要求条目勾了「问AI」且填了问题——两者都是网页写
/// 的字段，直接调这个端点绕过勾选框也会被拒（防止误触/脚本误调，语义上"问AI"这个开关就该管这件事）。
pub fn ask_entry(c: &Ctx<'_>, uuid: &str, e: &Entry) -> Result<Answered, String> {
    // 终态守卫（2026-09-09 审计补）：只检查 ask_ai/question 两个字段，不检查 status——一条已"跳过/
    // 撤销/删除"的条目只要 ask_ai 还留着 true 就能继续被问 AI、消耗 token，且对着一条用户认为已经
    // 处理完的条目回答没有意义。
    if e.is_terminal() {
        return Err("这条已跳过/撤销/删除，不能再问 AI".into());
    }
    if !e.ask_ai {
        return Err("这条没勾「问AI」".into());
    }
    let question = e.question.as_deref().map(str::trim).filter(|q| !q.is_empty()).ok_or("没有问题内容")?;
    let book = c.store.book(uuid)?;
    let ctx = crate::prompt::Context { book: &book.title, chapter: &e.chapter_title, quote: e.quote.as_ref().map(|q| q.text.as_str()), text: e.display_text(), question };
    let prompt = crate::prompt::build(&c.cfg.prompt, &ctx);
    let reply = match c.model.ask(&prompt) {
        Ok(r) => r,
        Err(err) => {
            c.ledger.record_fail(&c.cfg.usage_key(), &err, c.now);
            return Err(err);
        }
    };
    let answer = Answer { text: reply.text.clone(), backend: c.model.name().to_string(), at: c.now, brief: question.to_string() };
    c.store.post_answer(uuid, &e.id, &answer)?;
    c.ledger.record_ok(&c.cfg.usage_key(), reply.prompt_tokens, reply.completion_tokens, c.now);
    Ok(Answered { text: reply.text, prompt_tokens: reply.prompt_tokens, completion_tokens: reply.completion_tokens })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Fixed;
    use notecore::model::{Book, Ink, Quote, Status, Style};
    use std::sync::Mutex;

    fn entry(ask_ai: bool, question: Option<&str>) -> Entry {
        Entry {
            id: "e1".into(),
            page: "p".into(),
            page_index: 0,
            chapter: Some(0),
            chapter_title: "第一章".into(),
            subhead: None,
            quote: Some(Quote { id: "q1".into(), text: "原文".into(), color: "yellow".into(), rects: vec![] }),
            ink: Some(Ink { strokes: vec!["1:1".into()], bbox: (0.0, 0.0, 1.0, 1.0), hash: "h".into(), crop: "c.png".into() }),
            drafts: vec![],
            text: Some("这是谁".into()),
            style: Style::Body,
            ask_ai,
            question: question.map(str::to_string),
            answer: None,
            status: Status::Reviewed,
            destination: Default::default(),
            created: 0,
            updated: 0,
        }
    }

    struct Mem {
        book: Mutex<Book>,
        posted: Mutex<Vec<(String, Answer)>>,
    }
    impl EntryStore for Mem {
        fn book(&self, _uuid: &str) -> Result<Book, String> {
            Ok(self.book.lock().unwrap().clone())
        }
        fn post_answer(&self, _uuid: &str, id: &str, answer: &Answer) -> Result<(), String> {
            self.posted.lock().unwrap().push((id.into(), answer.clone()));
            Ok(())
        }
    }
    fn mem() -> Mem {
        Mem { book: Mutex::new(Book { uuid: "u".into(), title: "测试书".into(), ..Default::default() }), posted: Mutex::new(vec![]) }
    }
    fn cfg() -> MindConfig {
        MindConfig::default()
    }

    #[test]
    fn answers_and_writes_back_with_question_as_brief() {
        let store = mem();
        let model = Fixed("答案文本".into());
        let ledger = Ledger::open(&tempfile::tempdir().unwrap().path().join("mind.json"));
        let c = Ctx { store: &store, model: &model, cfg: &cfg(), ledger: &ledger, now: 42 };
        let out = ask_entry(&c, "u", &entry(true, Some("这是谁"))).unwrap();
        assert_eq!((out.text.as_str(), out.prompt_tokens, out.completion_tokens), ("答案文本", 10, 2), "点「提问」弹出的消耗就是这次调用的实际数字（见 backend::Fixed）");
        let posted = store.posted.lock().unwrap();
        assert_eq!(posted.len(), 1);
        assert_eq!((posted[0].0.as_str(), posted[0].1.text.as_str(), posted[0].1.brief.as_str(), posted[0].1.backend.as_str()), ("e1", "答案文本", "这是谁", "fixed"));
        let m = &ledger.snapshot().by_model[&cfg().usage_key()];
        assert_eq!((m.ok, m.calls), (1, 1));
    }

    #[test]
    fn refuses_when_ask_ai_is_off_or_question_is_empty() {
        let store = mem();
        let model = Fixed("x".into());
        let ledger = Ledger::open(&tempfile::tempdir().unwrap().path().join("mind.json"));
        let c = Ctx { store: &store, model: &model, cfg: &cfg(), ledger: &ledger, now: 1 };
        assert!(ask_entry(&c, "u", &entry(false, Some("问题"))).unwrap_err().contains("问AI"));
        assert!(ask_entry(&c, "u", &entry(true, None)).unwrap_err().contains("问题"));
        assert!(ask_entry(&c, "u", &entry(true, Some("  "))).unwrap_err().contains("问题"), "空白问题也算没有");
        assert!(store.posted.lock().unwrap().is_empty(), "拒绝的不该有任何写回");
    }

    #[test]
    fn refuses_terminal_entry_even_if_ask_ai_still_flagged() {
        // 2026-09-09 审计补：ask_ai/question 都还留着，但状态已经是终态（用户"不要了"）——不该被
        // 问 AI 消耗 token。
        let store = mem();
        let model = Fixed("不该被调用".into());
        let ledger = Ledger::open(&tempfile::tempdir().unwrap().path().join("mind.json"));
        let c = Ctx { store: &store, model: &model, cfg: &cfg(), ledger: &ledger, now: 1 };
        for s in [Status::Skipped, Status::Revoked, Status::Archived] {
            let mut e = entry(true, Some("问题"));
            e.status = s;
            let err = ask_entry(&c, "u", &e).unwrap_err();
            assert!(err.contains("跳过/撤销/删除"), "{s:?}: {err}");
        }
        assert!(store.posted.lock().unwrap().is_empty(), "终态条目一律不该有写回");
    }

    #[test]
    fn model_failure_is_recorded_and_nothing_is_written_back() {
        let store = mem();
        let model = Fixed("!fail".into());
        let ledger = Ledger::open(&tempfile::tempdir().unwrap().path().join("mind.json"));
        let c = Ctx { store: &store, model: &model, cfg: &cfg(), ledger: &ledger, now: 5 };
        let err = ask_entry(&c, "u", &entry(true, Some("问题"))).unwrap_err();
        assert_eq!(err, "模拟失败");
        assert!(store.posted.lock().unwrap().is_empty());
        let m = &ledger.snapshot().by_model[&cfg().usage_key()];
        assert_eq!((m.failed, m.last_error.as_str()), (1, "模拟失败"));
    }
}
