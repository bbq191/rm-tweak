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
    /// 读账本；文件损坏时从零记起（下一次记账会覆盖它），但先另存一份 `.corrupt` 副本（已有就不重复拷）——
    /// 累计用量是用户看花费的依据，不该被静默清零（2026-09-24 第三轮审计，跟 `ConfigCell`/条目库同一纪律）。
    pub fn open(path: &Path) -> Ledger<Extra> {
        if rmsvc_core::config::is_corrupt::<UsageBook<Extra>>(path) {
            let bak = path.with_extension("json.corrupt");
            if !bak.exists() {
                let _ = std::fs::copy(path, &bak);
            }
            eprintln!("[vendorcfg] 用量账本 {} 解析失败，从零记起（原内容另存 {}）", path.display(), bak.display());
        }
        Ledger { path: path.to_path_buf(), usage: Mutex::new(rmsvc_core::config::load_or_default(path)) }
    }
    pub fn snapshot(&self) -> UsageBook<Extra> {
        rmsvc_core::sync::lock(&self.usage).clone()
    }
    fn edit(&self, f: impl FnOnce(&mut UsageBook<Extra>)) {
        let mut u = rmsvc_core::sync::lock(&self.usage);
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

/// 「各个模型的用量花费 profile」（网页「模型」面板的用量表）：预置表里的每个模型都出一行（哪怕还没调用过、
/// 用量全 0——方便用户先把价格填上）；账本里出现过但不在预置表里的（比如用过的自定义模型）也补进来。
/// 花费只在用户填过单价（`prices`，缺省 0）时才算，没填就是 `null`，网页只显示 token 数不显示金额——理由见
/// crate 头注"花费不做官方定价表"。`transcribe-serve`/`mind-serve` 此前各写一份逐行相同的版本。
pub fn usage_profile<C: crate::VendorConfig, E>(cfg: &C, presets: &[crate::Preset], usage: &UsageBook<E>) -> serde_json::Value {
    let mut keys: Vec<String> = presets.iter().map(|p| p.id.to_string()).collect();
    for k in usage.by_model.keys() {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    let active = cfg.usage_key();
    let rows: Vec<serde_json::Value> = keys
        .into_iter()
        .map(|k| {
            let label = presets.iter().find(|p| p.id == k).map(|p| p.label.to_string()).unwrap_or_else(|| k.clone());
            let m = usage.by_model.get(&k).cloned().unwrap_or_default();
            let price = cfg.prices().get(&k).copied().unwrap_or_default();
            let cost = if price.input_per1k > 0.0 || price.output_per1k > 0.0 {
                Some((m.prompt_tokens as f64 / 1000.0) * price.input_per1k + (m.completion_tokens as f64 / 1000.0) * price.output_per1k)
            } else {
                None
            };
            serde_json::json!({"id": k, "label": label, "active": k == active,
                "calls": m.calls, "ok": m.ok, "failed": m.failed,
                "promptTokens": m.prompt_tokens, "completionTokens": m.completion_tokens,
                "lastError": m.last_error, "lastAt": m.last_at,
                "price": price, "costEstimate": cost})
        })
        .collect();
    serde_json::Value::Array(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Serialize)]
    struct TestCfg {
        preset: String,
        keys: BTreeMap<String, String>,
        prices: BTreeMap<String, crate::Price>,
    }
    const TEST_PRESETS: &[crate::Preset] = &[
        crate::Preset { id: "m1", label: "模型一", model: "m1", base_url: crate::DASHSCOPE, provider: "dashscope" },
        crate::Preset { id: "m2", label: "模型二", model: "m2", base_url: crate::DASHSCOPE, provider: "dashscope" },
    ];
    impl crate::VendorConfig for TestCfg {
        fn presets() -> &'static [crate::Preset] {
            TEST_PRESETS
        }
        fn preset(&self) -> &str {
            &self.preset
        }
        fn custom_model(&self) -> &str {
            ""
        }
        fn custom_base_url(&self) -> &str {
            ""
        }
        fn keys(&self) -> &BTreeMap<String, String> {
            &self.keys
        }
        fn prices(&self) -> &BTreeMap<String, crate::Price> {
            &self.prices
        }
    }

    #[test]
    fn usage_profile_lists_every_preset_plus_unknown_used_models_and_costs_only_with_price() {
        let mut prices = BTreeMap::new();
        prices.insert("m1".to_string(), crate::Price { input_per1k: 0.5, output_per1k: 1.0 });
        let cfg = TestCfg { preset: "m1".into(), keys: BTreeMap::new(), prices };
        let mut book: UsageBook = UsageBook::default();
        book.by_model.insert("m1".into(), ModelUsage { calls: 2, ok: 2, prompt_tokens: 2000, completion_tokens: 1000, ..Default::default() });
        book.by_model.insert("custom:x".into(), ModelUsage { calls: 1, ok: 1, prompt_tokens: 10, ..Default::default() });
        let v = usage_profile(&cfg, TEST_PRESETS, &book);
        let rows = v.as_array().unwrap();
        assert_eq!(rows.iter().map(|r| r["id"].as_str().unwrap()).collect::<Vec<_>>(), ["m1", "m2", "custom:x"], "预置全列（没用过的用量 0），账本里的自定义模型补在后面");
        assert_eq!(rows[0]["active"], true);
        assert_eq!(rows[0]["costEstimate"], 2.0, "2 千入 × 0.5 + 1 千出 × 1.0");
        assert!(rows[1]["costEstimate"].is_null() && rows[2]["costEstimate"].is_null(), "没填单价不估算");
        assert_eq!(rows[2]["label"], "custom:x", "不在预置表里的用 id 当标签");
    }


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
    fn corrupt_ledger_is_backed_up_before_being_overwritten() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("mind.json");
        std::fs::write(&p, b"{\"byModel\": {broken").unwrap();
        let l: Ledger<()> = Ledger::open(&p);
        assert!(l.snapshot().by_model.is_empty());
        l.record_ok("m", 1, 1, 1);
        assert_eq!(std::fs::read(p.with_extension("json.corrupt")).unwrap(), b"{\"byModel\": {broken", "原内容留了副本");
        assert_eq!(Ledger::<()>::open(&p).snapshot().by_model["m"].calls, 1);
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
