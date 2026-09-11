//! 配置 `~/.config/notes/mind.json`（0600：含 key）。
//! **第二轮整理区反馈（2026-09-08，点 2「模型管理彻底重做」）**：一个服务不再只认一家厂商——预置表现在
//! 横跨 DashScope/OpenAI/Gemini/DeepSeek 四家，切换预置只是换"这次用哪个模型"，key 按厂商（`provider`）
//! 分开存（`keys` 表），不会出现"切到 OpenAI 却把 DashScope 的 key 发过去"这种事，也不用每切一次模型
//! 就重新粘贴 key。`custom` 转义阀单独占一格 key。
//! **预置选择/key 存取/迁移/PATCH 的核心逻辑跟 `transcribe-serve::config` 共享**（`vendorcfg` crate，
//! 2026-09-08 抽出来，之前两边各抄一遍）——这里是文字模型表，跟视觉模型表分开维护；模型 id 核实来源/
//! 豆包为什么不进预置表/花费为什么不做官方定价表，见 `vendorcfg` crate 文档，理由完全一样，不重复写。
//! **没有 `maxPerRun`/`pauseMs`/`auto`/`maxAttempts` 这些节流字段**——mind-serve 不跑批量循环，纯粹是
//! "问一条答一条"，见 `worker.rs`/`main.rs` 文档。
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use vendorcfg::{Preset, Price, VendorConfig, DASHSCOPE, DEEPSEEK, GEMINI, OPENAI};

/// 文字模型预置表（换厂商/加型号在这加一行，网页自动出现新选项；核实来源见 `vendorcfg` crate 文档）。
pub const PRESETS: &[Preset] = &[
    Preset { id: "qwen-plus", label: "Qwen-Plus（推荐）", model: "qwen-plus", base_url: DASHSCOPE, provider: "dashscope" },
    Preset { id: "qwen-max", label: "Qwen-Max（更强，更贵）", model: "qwen-max", base_url: DASHSCOPE, provider: "dashscope" },
    Preset { id: "qwen-turbo", label: "Qwen-Turbo（更快更便宜）", model: "qwen-turbo", base_url: DASHSCOPE, provider: "dashscope" },
    Preset { id: "gpt-5.6-luna", label: "GPT-5.6 Luna（OpenAI，便宜量大）", model: "gpt-5.6-luna", base_url: OPENAI, provider: "openai" },
    Preset { id: "gpt-5.6-terra", label: "GPT-5.6 Terra（OpenAI，性价比）", model: "gpt-5.6-terra", base_url: OPENAI, provider: "openai" },
    Preset { id: "gemini-3.8-flash", label: "Gemini 3.8 Flash（Google）", model: "gemini-3.8-flash", base_url: GEMINI, provider: "gemini" },
    Preset { id: "deepseek-v4-flash", label: "DeepSeek V4 Flash（快省）", model: "deepseek-v4-flash", base_url: DEEPSEEK, provider: "deepseek" },
    Preset { id: "deepseek-v4-pro", label: "DeepSeek V4 Pro（更强）", model: "deepseek-v4-pro", base_url: DEEPSEEK, provider: "deepseek" },
];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct MindConfig {
    /// 后端标识（写进 `Answer.backend`）。
    pub backend: String,
    /// 当前选中的预置 id；`"custom"` 走下面两个手填字段。
    pub preset: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub custom_model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub custom_base_url: String,
    /// 厂商（`Preset.provider`，或 `"custom"`）→ key。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
    /// 预置 id（或 `"custom"`）→ 用户自填单价。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub prices: BTreeMap<String, Price>,
    /// 单次请求超时（秒）。
    pub timeout_secs: u64,
    /// 自定义提示词前缀（空 = 内置）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,

    /// 迁移专用（见 `transcribe-serve::config` 同名字段的说明，只在反序列化时读一次）。
    #[serde(skip_serializing)]
    pub model: String,
    #[serde(skip_serializing)]
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: String,
}

impl Default for MindConfig {
    fn default() -> Self {
        MindConfig {
            backend: "qwen".into(),
            preset: PRESETS[0].id.to_string(),
            custom_model: String::new(),
            custom_base_url: String::new(),
            keys: BTreeMap::new(),
            prices: BTreeMap::new(),
            timeout_secs: 60,
            prompt: String::new(),
            model: String::new(),
            base_url: String::new(),
            api_key: String::new(),
        }
    }
}

impl VendorConfig for MindConfig {
    fn presets() -> &'static [Preset] {
        PRESETS
    }
    fn preset(&self) -> &str {
        &self.preset
    }
    fn custom_model(&self) -> &str {
        &self.custom_model
    }
    fn custom_base_url(&self) -> &str {
        &self.custom_base_url
    }
    fn keys(&self) -> &BTreeMap<String, String> {
        &self.keys
    }
    fn prices(&self) -> &BTreeMap<String, Price> {
        &self.prices
    }
}

impl MindConfig {
    /// 老配置文件搬进新形状——只在启动加载时调用一次，见 `transcribe-serve::config::migrate` 的说明。
    pub fn migrate(mut self) -> Self {
        vendorcfg::migrate_legacy(PRESETS, &mut self.preset, &mut self.custom_model, &mut self.custom_base_url, &mut self.keys, &self.model, &self.base_url, &self.api_key);
        self
    }
    /// 套用 PUT /config 的 JSON（同 `transcribe-serve::config::apply` 的规则，少了节流字段）。
    pub fn apply(&mut self, j: &serde_json::Value) -> Result<(), String> {
        if let Some(v) = j.get("backend").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
            self.backend = v.to_string();
        }
        vendorcfg::apply_common(PRESETS, j, &mut self.preset, &mut self.custom_model, &mut self.custom_base_url, &mut self.keys, &mut self.prices)?;
        if let Some(v) = j.get("timeoutSecs").and_then(|v| v.as_u64()) { self.timeout_secs = v.clamp(5, 600); }
        if let Some(v) = j.get("prompt") {
            self.prompt = v.as_str().unwrap_or("").trim().to_string();
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_priority_and_masking_scoped_by_provider() {
        let mut c = MindConfig::default();
        assert_eq!(c.key_with_env(None), None);
        assert_eq!(c.key_with_env(Some(" env-k ".into())).as_deref(), Some("env-k"));
        c.keys.insert("dashscope".into(), "file-k".into());
        assert_eq!(c.key_with_env(Some("env-k".into())).as_deref(), Some("file-k"), "文件优先");
        let p = c.public();
        assert!(p.get("apiKey").is_none() && p.get("keys").is_none(), "对外不回显 key/keys 表: {p}");
        assert_eq!(p["hasKey"], true);
        assert_eq!(p["model"], "qwen-plus");
        assert_eq!(p["keyMasked"], "...le-k", "只露最后 4 位，不整串回显");
        c.keys.remove("dashscope");
        assert_eq!(c.public()["keyMasked"], serde_json::Value::Null, "没 key 时是 null");
    }

    #[test]
    fn switching_provider_does_not_leak_other_providers_key() {
        let mut c = MindConfig::default();
        c.keys.insert("dashscope".into(), "ds-key".into());
        c.apply(&serde_json::json!({"preset": "gpt-5.6-luna"})).unwrap();
        assert_eq!(c.key(), None, "切到 OpenAI，还没存过它的 key");
        c.apply(&serde_json::json!({"apiKey": "oa-key"})).unwrap();
        assert_eq!(c.key(), Some("oa-key".into()));
        c.apply(&serde_json::json!({"preset": "qwen-max"})).unwrap();
        assert_eq!(c.key(), Some("ds-key".into()), "切回 DashScope 系预置，之前存的 key 还在");
    }

    #[test]
    fn apply_updates_only_given_fields() {
        let mut c = MindConfig::default();
        c.apply(&serde_json::json!({"apiKey": "k1", "model": "m2"})).unwrap();
        assert_eq!(c.key().as_deref(), Some("k1"), "preset 缺省不是 custom，model 字段不生效");
        c.apply(&serde_json::json!({"apiKey": ""})).unwrap();
        assert_eq!(c.key().as_deref(), Some("k1"), "空 apiKey 不改");
        c.apply(&serde_json::json!({"clearKey": true})).unwrap();
        assert!(c.key().is_none());
        assert_eq!(c.timeout_secs, 60, "没给的字段不动");
    }

    #[test]
    fn preset_selection_sets_model_and_base_url_atomically_and_rejects_unknown() {
        let mut c = MindConfig::default();
        assert_eq!(c.preset, "qwen-plus", "缺省值就是第一个预置");
        c.apply(&serde_json::json!({"preset": "qwen-max"})).unwrap();
        assert_eq!((c.model(), c.base_url()), ("qwen-max", "https://dashscope.aliyuncs.com/compatible-mode/v1"));
        let err = c.apply(&serde_json::json!({"preset": "gpt-4o"})).unwrap_err();
        assert!(err.contains("未知的模型预置"), "{err}");
        assert_eq!(c.preset, "qwen-max", "拒绝后不改动");
    }

    #[test]
    fn custom_preset_leaves_model_and_base_url_to_the_old_manual_fields() {
        let mut c = MindConfig::default();
        c.apply(&serde_json::json!({"preset": "custom", "model": "my-model", "baseUrl": "https://x.example/v1"})).unwrap();
        assert_eq!((c.model(), c.base_url()), ("my-model", "https://x.example/v1"));
    }

    #[test]
    fn public_exposes_presets_and_active_preset() {
        let c = MindConfig::default();
        let p = c.public();
        assert_eq!(p["activePreset"], "qwen-plus");
        assert!(p["presets"].as_array().unwrap().len() >= 4, "四家厂商都要出现在下拉里");
        assert_eq!(p["presets"][0]["id"], "qwen-plus");
    }

    #[test]
    fn price_is_user_entered_per_model_and_defaults_to_zero() {
        let mut c = MindConfig::default();
        assert_eq!(c.price(), Price::default());
        c.apply(&serde_json::json!({"price": {"input": 0.02, "output": 0.06}})).unwrap();
        assert_eq!(c.price(), Price { input_per1k: 0.02, output_per1k: 0.06 });
    }

    #[test]
    fn migrate_carries_forward_legacy_flat_model_and_key_without_losing_it() {
        let old = serde_json::json!({"backend":"qwen","baseUrl":"https://dashscope.aliyuncs.com/compatible-mode/v1","model":"qwen-max","apiKey":"real-device-key","timeoutSecs":60});
        let c: MindConfig = serde_json::from_value(old).unwrap();
        let c = c.migrate();
        assert_eq!(c.preset, "qwen-max");
        assert_eq!(c.key().as_deref(), Some("real-device-key"));
        assert_eq!(c.clone().migrate(), c, "已经迁移过是 no-op");
    }

    /// 真机 2026-09-08 踩过的坑：老配置的 `model` 是 `qwen3-vl-plus`（视觉模型，这条服务的文字预置表
    /// 压根没收），迁移时"精确匹配预置"这条路必然落空、判成 `custom`；如果 key 也存进字面意义的
    /// `custom` 格，用户后来把预置切到同样是 DashScope 的 `qwen-plus`，新预置在 `dashscope` 格找不到
    /// 那把明明是同一账号的 key——`provider()` 加了 baseUrl 兜底之后，这两步都应该找到同一把 key。
    #[test]
    fn migrate_unmatched_model_but_known_provider_base_url_shares_key_with_real_presets_of_that_provider() {
        let old = serde_json::json!({"model":"qwen3-vl-plus","baseUrl":"https://dashscope.aliyuncs.com/compatible-mode/v1","apiKey":"real-device-key"});
        let c: MindConfig = serde_json::from_value(old).unwrap();
        let mut c = c.migrate();
        assert_eq!(c.preset, "custom", "qwen3-vl-plus 不在文字预置表里，还是落 custom");
        assert_eq!(c.provider(), "dashscope", "但 baseUrl 认出来是 DashScope，key 该存这一格");
        assert_eq!(c.key().as_deref(), Some("real-device-key"));
        c.apply(&serde_json::json!({"preset": "qwen-plus"})).unwrap();
        assert_eq!(c.key().as_deref(), Some("real-device-key"), "切到同厂商的真实预置，key 还在，不用重新粘贴");
    }

    #[test]
    fn migrate_is_noop_for_fresh_install_with_no_legacy_data() {
        let c = MindConfig::default().migrate();
        assert_eq!(c, MindConfig::default());
    }

    /// 真机 2026-09-08 实测采样的 mind.json 形状（key 值脱敏，字段名/大小写原样，含 customModel/
    /// customBaseUrl——真机上这台设备的预置停在 custom 但 keys 里已经按 baseUrl 兜底认对了厂商）：
    /// 确认核心逻辑挪进 `vendorcfg` 之后原样读得出来、`key()` 给出跟改之前一致的结果。
    #[test]
    fn reads_real_device_config_shape_unchanged() {
        let raw = r#"{"backend":"qwen","preset":"qwen-plus","customModel":"qwen3-vl-plus","customBaseUrl":"https://dashscope.aliyuncs.com/compatible-mode/v1","keys":{"custom":"REDACTED-KEY","dashscope":"REDACTED-KEY"},"timeoutSecs":60}"#;
        let c: MindConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(c.model(), "qwen-plus", "preset 已经切到真实预置，model() 走预置表不走 customModel");
        assert_eq!(c.provider(), "dashscope");
        assert_eq!(c.key().as_deref(), Some("REDACTED-KEY"), "dashscope 格的 key 能正常解出来");
    }
}
