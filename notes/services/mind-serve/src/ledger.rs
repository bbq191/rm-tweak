//! 用量账本 `~/.local/state/notes/mind.json`：调用/成功/失败次数、token 累计、最近一次错误。
//! 只记数不记内容（不存 key、不存问题/回答文本）。没有 transcribe-serve 那个 `last_run`（一轮批量的
//! 报告）——mind-serve 没有批量轮次，每次调用都是独立的一问一答，`Usage`/`Ledger` 是 `vendorcfg` 泛型
//! 在 `Extra=()` 上的具体实例化，磁盘上完全不会出现 `lastRun` 键。
//! **存取逻辑跟 transcribe-serve 共享**（`vendorcfg::usage`，2026-09-08 抽出来，之前两边各抄一遍）。
//! **第二轮整理区反馈（2026-09-08，点 2）**：按模型分账（`by_model`，见 `transcribe-serve::ledger` 同样
//! 的设计理由）。
pub type Ledger = vendorcfg::Ledger;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persists_counts_per_model() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("mind.json");
        let l = Ledger::open(&p);
        l.record_ok("qwen-plus", 80, 20, 1);
        l.record_fail("qwen-plus", "HTTP 401：bad key", 2);
        l.record_ok("deepseek-v4-flash", 30, 5, 3);
        let back = Ledger::open(&p).snapshot();
        let qwen = &back.by_model["qwen-plus"];
        assert_eq!((qwen.calls, qwen.ok, qwen.failed, qwen.prompt_tokens, qwen.completion_tokens), (2, 1, 1, 80, 20));
        assert_eq!(qwen.last_error, "HTTP 401：bad key");
        assert_eq!(qwen.last_at, 2);
        assert_eq!(back.by_model["deepseek-v4-flash"].calls, 1, "不同模型各算各的");
    }

    /// 真机 2026-09-08 实测采样的 mind.json 用量账本形状：只有 `byModel`，没有 `lastRun`——确认
    /// 泛型化后（`Extra=()`）原样读得出来，也确认存的时候依然不会多出 `lastRun` 键。
    #[test]
    fn reads_real_device_ledger_shape_unchanged() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("mind.json");
        std::fs::write(&p, r#"{"byModel":{"custom:qwen3-vl-plus":{"calls":1,"ok":1,"failed":0,"promptTokens":111,"completionTokens":75,"lastAt":1788853934,"lastError":""},"qwen-plus":{"calls":1,"ok":0,"failed":1,"promptTokens":0,"completionTokens":0,"lastAt":1788853782,"lastError":"HTTP 403"}}}"#).unwrap();
        let back = Ledger::open(&p).snapshot();
        assert_eq!(back.by_model["custom:qwen3-vl-plus"].prompt_tokens, 111);
        assert_eq!(back.by_model["qwen-plus"].last_error, "HTTP 403");
        l_never_emits_last_run(&p);
    }

    fn l_never_emits_last_run(p: &std::path::Path) {
        let l = Ledger::open(p);
        l.record_ok("x", 1, 1, 9);
        let raw = std::fs::read_to_string(p).unwrap();
        assert!(!raw.contains("lastRun"), "mind-serve 没有一轮报告概念，写回去也不该多出这个键: {raw}");
    }
}
