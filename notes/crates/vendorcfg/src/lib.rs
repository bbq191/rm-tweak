//! 笔记线两个"调 AI 厂商模型"的服务（`transcribe-serve` 视觉/`mind-serve` 文字）共享的核心逻辑：
//! 预置模型表 + key 按厂商分格存取（baseUrl 兜底认厂商）+ PUT /config 的 PATCH 语义 + 用量账本。
//! 两边此前是两份几乎逐行相同的代码（各自的 `config.rs`/`ledger.rs`），2026-09-08 抽出来。
//!
//! **只抽"行为"，不抽"数据结构"**：两个服务各自的 `TranscribeConfig`/`MindConfig`（字段不同——
//! transcribe 多一套节流参数 `max_per_run`/`pause_ms`/`auto`/`max_attempts`，mind 没有）和
//! `Usage`（transcribe 多一个 `last_run`）仍然各自定义、各自的 serde 落盘形状完全不变，不强行把
//! 两个服务的 Config/Usage 结构本身合并成一个泛型类型——真机上已经有用户配置好的
//! `~/.config/notes/{transcribe,mind}.json` 和用量账本文件，保住磁盘格式字节不变比"抽得更彻底"
//! 更重要（`preset`/`usage` 两个模块下的测试都拿真机 2026-09-08 实测采样的 JSON 形状做回归）。
//! **2026-09-15 补**：两边共有的只读派生方法（`provider()`/`model()`/`key()`/`public()` 等，
//! 之前两边各包一层同名转发、逐字节相同）收进 [`preset::VendorConfig`] trait——两个 Config
//! 结构体各自实现六个字段访问器即可拿到全部默认方法，字段/序列化形状仍然完全不动，跟上面这条
//! "只抽行为不抽数据结构"的原则一致，是同一条原则的延伸，不是推翻。
//!
//! **模型 id 是易变信息，不凭记忆写**：`DASHSCOPE`/`OPENAI`/`GEMINI`/`DEEPSEEK` 四个 baseUrl 常量、
//! 以及两个服务各自预置表里的型号字符串，都是 2026-09-08 当天过 WebSearch/WebFetch 核实官方文档页
//! 给出的（OpenAI `developers.openai.com/api/docs/models`、Gemini `ai.google.dev/gemini-api/docs/openai`、
//! DeepSeek `api-docs.deepseek.com/quick_start/pricing`）——这类字符串官方随时会改名，写死进代码本身
//! 就是权宜之计；真跑不通了首选去官方文档核对是不是又改了，而不是怀疑这段注释。豆包（火山方舟）
//! **没有**收进预置表：它的"模型"实际是账号自建的推理接入点 ID（`ep-xxxxxxxx`），不是一个所有用户
//! 通用的固定字符串，硬填一个占位模型名到预置表里反而是在提供一个保真不了的"已知能用"承诺——用户要
//! 接豆包，走"自定义"，`baseUrl` 填 `https://ark.cn-beijing.volces.com/api/v3`，`model` 填自己在方舟
//! 控制台建的 Endpoint ID。**花费不做官方定价表**：第三方 API 定价比模型 id 还易变，写死一份价格表
//! 更容易在用户不知情的情况下把"预估花费"做错——干脆不猜，改成让用户自己填单价（`prices`，缺省 0）。

pub mod preset;
pub mod usage;

pub use preset::{
    apply_common, key_masked, key_source, migrate_legacy, provider_for_base_url, public_json,
    resolve_base_url, resolve_key, resolve_model, resolve_provider, usage_key, KeySource, Preset,
    Price, VendorConfig, DASHSCOPE, DEEPSEEK, GEMINI, KEY_ENV, OPENAI,
};
pub use usage::{Ledger, ModelUsage, UsageBook};

/// 按字符数截断，超长加省略号；不 trim（调用方如果需要先 trim 自己处理，跟原样保留空白的场景区分
/// 开）。`mind-serve::backend`/`transcribe-serve::backend`/`mind-serve::prompt` 三处此前各自定义了
/// 一份几乎相同的截断函数（2026-09-09 审计发现），既然两个服务都已经依赖这个 crate，收进来一行改动量。
pub fn truncate_chars(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        s.chars().take(n).collect::<String>() + "…"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_chars_counts_unicode_scalars_not_bytes() {
        assert_eq!(truncate_chars("短", 5), "短", "没超长原样返回，不加省略号");
        assert_eq!(truncate_chars("一二三四五六", 3), "一二三…", "按字符数不是字节数截断，中文一个字不该被腰斩");
        assert_eq!(truncate_chars("  带前后空白  ", 20), "  带前后空白  ", "不 trim，原样保留");
    }
}
