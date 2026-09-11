//! 用量账本 `~/.local/state/notes/transcribe.json`：调用/成功/失败次数、token 累计、最近一次错误与一轮报告。
//! 只记数不记内容（不存 key、不存转写文本）。
//! **存取逻辑共享给 mind-serve**（`vendorcfg::usage`，2026-09-08 抽出来，之前两边各抄一遍）——这里只
//! 定义这条服务独有的东西：`RunReport`（一轮批量转写的结果，mind-serve 没有批量轮次，不需要它）。
//! `Usage`/`Ledger` 是 `vendorcfg` 泛型在 `RunReport` 上的具体实例化。
//! **第二轮整理区反馈（2026-09-08，点 2）**：按模型分账（`by_model`，键是 `TranscribeConfig::usage_key()`——
//! 预置 id 或 `custom:<model>`）——同一个服务现在能在多家厂商之间切换预置，"各个模型的用量花费 profile"
//! 要求每个用过的模型各算各的，不能只有一份全局聚合数字（不然切个模型历史用量就混一起分不清了）。
use serde::{Deserialize, Serialize};

/// 一轮转写的结果（网页状态区显示）。
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct RunReport {
    pub at: u64,
    pub scanned: usize,
    pub done: usize,
    pub failed: usize,
    pub skipped: usize,
    pub left: usize,
    /// 这一轮成功调用累计花的 token（点「重转」弹出消耗要用，2026-09-08 第三轮反馈）——强制单条时
    /// 这轮只有一次成功调用，这两个数就是那一次调用的实际消耗；批量跑一轮时是整轮的累计。
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub note: String,
}

pub type Usage = vendorcfg::UsageBook<RunReport>;
pub type Ledger = vendorcfg::Ledger<RunReport>;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persists_counts_per_model() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("transcribe.json");
        let l = Ledger::open(&p);
        l.record_ok("qwen3-vl-plus", 100, 5, 1);
        l.record_fail("qwen3-vl-plus", "HTTP 401：bad key", 2);
        l.record_ok("gpt-5.6-terra", 50, 10, 3);
        l.record_run(RunReport { at: 2, scanned: 2, done: 1, failed: 1, ..Default::default() });
        let back = Ledger::open(&p).snapshot();
        let qwen = &back.by_model["qwen3-vl-plus"];
        assert_eq!((qwen.calls, qwen.ok, qwen.failed, qwen.prompt_tokens, qwen.completion_tokens), (2, 1, 1, 100, 5));
        assert_eq!(qwen.last_error, "HTTP 401：bad key");
        let gpt = &back.by_model["gpt-5.6-terra"];
        assert_eq!((gpt.calls, gpt.ok, gpt.prompt_tokens), (1, 1, 50), "不同模型各算各的，不会混到一起");
        assert_eq!(back.last_run.unwrap().done, 1);
    }

    /// 真机 2026-09-08 实测采样的 transcribe.json 用量账本形状（数值原样，非敏感）：`byModel` +
    /// `lastRun` 都要原样读出来——这是重构最要紧的一条回归，真机上已经有累计的真实用量数字。
    #[test]
    fn reads_real_device_ledger_shape_unchanged() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("transcribe.json");
        std::fs::write(&p, r#"{"byModel":{"qwen3-vl-plus":{"calls":8,"ok":8,"failed":0,"promptTokens":2528,"completionTokens":104,"lastAt":1788853946,"lastError":""}},"lastRun":{"at":1788853946,"scanned":1,"done":1,"failed":0,"skipped":0,"left":0,"promptTokens":316,"completionTokens":13,"note":""}}"#).unwrap();
        let back = Ledger::open(&p).snapshot();
        let m = &back.by_model["qwen3-vl-plus"];
        assert_eq!((m.calls, m.ok, m.prompt_tokens, m.completion_tokens), (8, 8, 2528, 104));
        assert_eq!(back.last_run.unwrap().prompt_tokens, 316);
    }
}
