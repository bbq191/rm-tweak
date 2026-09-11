//! 用量账本：调用/成功/失败次数、token 累计、最近一次错误，按模型分账（`by_model`，键是
//! `usage_key()`）。只记数不记内容（不存 key、不存转写文本/问答文本）。`transcribe-serve`/
//! `mind-serve` 落盘文件（`~/.local/state/notes/{transcribe,mind}.json`）几乎逐字段相同，之前是两份
//! 重复代码——唯一的结构性差异是 transcribe-serve 多一个"一轮批量转写的报告"（`last_run`，mind-serve
//! 没有批量轮次，每次调用都是独立的一问一答，用不上）。抽成 `UsageBook<Extra>` 泛型，`Extra` 是
//! "这个服务有没有额外的一轮报告"：transcribe-serve 用 `UsageBook<RunReport>`（自己定义 `RunReport`
//! 形状），mind-serve 用 `UsageBook<()>`（`last_run` 用 `skip_serializing_if` 跳过，磁盘上完全不出现
//! `lastRun` 键，跟它原来的 `Usage` 结构体压根没有这个字段效果一致）。
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelUsage {
    pub calls: u64,
    pub ok: u64,
    pub failed: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub last_at: u64,
    pub last_error: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default, bound(serialize = "Extra: Serialize", deserialize = "Extra: DeserializeOwned"))]
pub struct UsageBook<Extra = ()> {
    pub by_model: BTreeMap<String, ModelUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run: Option<Extra>,
}

impl<Extra> Default for UsageBook<Extra> {
    fn default() -> Self {
        UsageBook { by_model: BTreeMap::new(), last_run: None }
    }
}

pub struct Ledger<Extra = ()> {
    path: PathBuf,
    usage: Mutex<UsageBook<Extra>>,
}

impl<Extra: Clone + Serialize + DeserializeOwned> Ledger<Extra> {
    pub fn open(path: &Path) -> Ledger<Extra> {
        Ledger { path: path.to_path_buf(), usage: Mutex::new(rmsvc_core::config::load_or_default(path)) }
    }
    pub fn snapshot(&self) -> UsageBook<Extra> {
        self.usage.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
    fn edit(&self, f: impl FnOnce(&mut UsageBook<Extra>)) {
        let mut u = self.usage.lock().unwrap_or_else(|e| e.into_inner());
        f(&mut u);
        let _ = rmsvc_core::config::save(&self.path, &*u, None);
    }
    pub fn record_ok(&self, model_key: &str, prompt_tokens: u64, completion_tokens: u64, now: u64) {
        self.edit(|u| {
            let m = u.by_model.entry(model_key.to_string()).or_default();
            m.calls += 1;
            m.ok += 1;
            m.prompt_tokens += prompt_tokens;
            m.completion_tokens += completion_tokens;
            m.last_at = now;
        });
    }
    pub fn record_fail(&self, model_key: &str, err: &str, now: u64) {
        self.edit(|u| {
            let m = u.by_model.entry(model_key.to_string()).or_default();
            m.calls += 1;
            m.failed += 1;
            m.last_at = now;
            m.last_error = err.chars().take(200).collect();
        });
    }
    /// 只有维护"一轮批量报告"概念的服务（目前只有 transcribe-serve，`Extra=RunReport`）会调用；
    /// `Extra=()` 的服务（mind-serve）没有理由调用它，但方法本身对任何 `Extra` 都能用，不用专门拆
    /// 一个 trait 出来区分"有没有这个能力"——用不到就是不调用，比强行分两条继承链更简单。
    pub fn record_run(&self, r: Extra) {
        self.edit(|u| u.last_run = Some(r));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
    #[serde(rename_all = "camelCase", default)]
    struct FakeRun {
        done: usize,
    }

    #[test]
    fn persists_counts_per_model_with_run_report() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("transcribe.json");
        let l: Ledger<FakeRun> = Ledger::open(&p);
        l.record_ok("model-a", 100, 5, 1);
        l.record_fail("model-a", "HTTP 401：bad key", 2);
        l.record_ok("model-b", 50, 10, 3);
        l.record_run(FakeRun { done: 1 });
        let back: UsageBook<FakeRun> = Ledger::open(&p).snapshot();
        let a = &back.by_model["model-a"];
        assert_eq!((a.calls, a.ok, a.failed, a.prompt_tokens, a.completion_tokens), (2, 1, 1, 100, 5));
        assert_eq!(a.last_error, "HTTP 401：bad key");
        let b = &back.by_model["model-b"];
        assert_eq!((b.calls, b.ok, b.prompt_tokens), (1, 1, 50), "不同模型各算各的，不会混到一起");
        assert_eq!(back.last_run.unwrap().done, 1);
    }

    #[test]
    fn extra_unit_type_never_emits_last_run_key() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("mind.json");
        let l: Ledger<()> = Ledger::open(&p);
        l.record_ok("model-a", 10, 2, 1);
        let raw = std::fs::read_to_string(&p).unwrap();
        assert!(!raw.contains("lastRun"), "Extra=() 没有一轮报告概念，磁盘上不该出现 lastRun 键: {raw}");
        let back: UsageBook<()> = Ledger::open(&p).snapshot();
        assert_eq!(back.by_model["model-a"].calls, 1);
    }

    /// 真机 2026-09-08 实测采样的 mind.json 用量账本形状（数值脱敏，字段名/大小写原样）：只有
    /// `byModel`，没有 `lastRun`。确认泛型化后原样能读出来——这是这次重构最要紧的一条回归，真机上
    /// 已经有累计的真实用量数字，字段名/大小写差一点就会读不出来（悄悄退化成"账本是空的"）。
    #[test]
    fn reads_real_device_mind_ledger_shape_unchanged() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("mind.json");
        std::fs::write(&p, r#"{"byModel":{"qwen-plus":{"calls":1,"ok":0,"failed":1,"promptTokens":0,"completionTokens":0,"lastAt":1788853782,"lastError":"HTTP 403"}}}"#).unwrap();
        let back: UsageBook<()> = Ledger::open(&p).snapshot();
        let m = &back.by_model["qwen-plus"];
        assert_eq!((m.calls, m.ok, m.failed, m.last_at), (1, 0, 1, 1788853782));
        assert_eq!(m.last_error, "HTTP 403");
        assert!(back.last_run.is_none());
    }
}
