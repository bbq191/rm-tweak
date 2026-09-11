//! 配置 `~/.config/notes/transcribe.json`（0600：含 key）。
//! **第二轮整理区反馈（2026-09-08，点 2「模型管理彻底重做」）**：一个服务不再只认一家厂商——预置表现在
//! 横跨 DashScope/OpenAI/Gemini/DeepSeek 四家，切换预置只是换"这次用哪个模型"，key 按厂商（`provider`）
//! 分开存（`keys` 表），不会出现"切到 OpenAI 却把 DashScope 的 key 发过去"这种事，也不用每切一次模型
//! 就重新粘贴 key。`custom` 转义阀单独占一格 key（跟四个已知厂商都不共用，防止乱填的地址意外收到
//! 别家的密钥）。
//! 对外（GET /config）永远只报 `hasKey`/`keySource`，**不回显 key**；写入走 PUT /config 的 `apiKey` 字段
//! （空串=不改，`clearKey`=清当前预置所属厂商的那把）。
//! **预置选择/key 存取/迁移/PATCH 的核心逻辑现在共享给 mind-serve**（`vendorcfg` crate，2026-09-08 抽
//! 出来），这里只留：① 这条服务自己的视觉模型预置表（型号跟 mind-serve 的文字表不同）；② 这条服务
//! 独有的字段（`max_per_run`/`pause_ms`/`auto`/`max_attempts` 这套节流参数，mind-serve 没有，它不跑
//! 批量循环）。模型 id 核实来源、豆包为什么不进预置表、花费为什么不做官方定价表，这几条设计理由见
//! `vendorcfg` 的 crate 文档，不在这重复。
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use vendorcfg::{Preset, Price, VendorConfig, DASHSCOPE, DEEPSEEK, GEMINI, OPENAI};

/// 视觉模型预置表（换厂商/加型号在这加一行，网页自动出现新选项；核实来源见 `vendorcfg` crate 文档）。
pub const PRESETS: &[Preset] = &[
    Preset { id: "qwen3-vl-plus", label: "Qwen3-VL-Plus（推荐，速度快）", model: "qwen3-vl-plus", base_url: DASHSCOPE, provider: "dashscope" },
    Preset { id: "qwen-vl-max", label: "Qwen-VL-Max（更准，稍慢）", model: "qwen-vl-max", base_url: DASHSCOPE, provider: "dashscope" },
    Preset { id: "qwen-vl-plus", label: "Qwen-VL-Plus（旧一代视觉模型）", model: "qwen-vl-plus", base_url: DASHSCOPE, provider: "dashscope" },
    Preset { id: "gpt-5.6-terra", label: "GPT-5.6 Terra（OpenAI，性价比）", model: "gpt-5.6-terra", base_url: OPENAI, provider: "openai" },
    Preset { id: "gpt-6-astra", label: "GPT-6 Astra（OpenAI，旗舰更贵）", model: "gpt-6-astra", base_url: OPENAI, provider: "openai" },
    Preset { id: "gemini-3.8-flash", label: "Gemini 3.8 Flash（Google）", model: "gemini-3.8-flash", base_url: GEMINI, provider: "gemini" },
    Preset { id: "deepseek-flash", label: "DeepSeek V4.1 Flash（原生多模态）", model: "deepseek-flash", base_url: DEEPSEEK, provider: "deepseek" },
];

/// 官方已下线的预置 id → 现行 id（老配置里存的选择自动迁过去）。
const RETIRED_PRESETS: &[(&str, &str)] = &[("deepseek-v4-flash-vision-exp", "deepseek-flash")];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct TranscribeConfig {
    /// 后端标识（写进草稿 `backend` 字段，便于区分不同模型的建议）。
    pub backend: String,
    /// 当前选中的预置 id；`"custom"` 走下面两个手填字段。
    pub preset: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub custom_model: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub custom_base_url: String,
    /// 厂商（`Preset.provider`，或 `"custom"`）→ key。同厂商多个预置共用一把，切换预置不用重新粘贴。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub keys: BTreeMap<String, String>,
    /// 预置 id（或 `"custom"`）→ 用户自填单价，见 `vendorcfg` 模块文档"花费不做官方定价表"。
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub prices: BTreeMap<String, Price>,
    /// 单次请求超时（秒）。设备走自己的 WiFi 直连，国内 API 通常 5–15 s。
    pub timeout_secs: u64,
    /// 一轮最多转写多少条（防一次合书几十条把费用打爆；剩下的下一轮接着来）。
    pub max_per_run: usize,
    /// 两次请求间歇（毫秒），给限流留余量。
    pub pause_ms: u64,
    /// 条目库有新条目时自动转写；关掉则只在网页点「转写」时跑。
    pub auto: bool,
    /// 同一条目（同指纹）失败几次后不再自动重试（网页可手动重来）。
    pub max_attempts: u32,
    /// 自定义提示词（空 = 内置）。
    #[serde(skip_serializing_if = "String::is_empty")]
    pub prompt: String,

    /// 迁移专用（第二轮反馈重做前的老配置文件形状：单一 `model`/`baseUrl`/`apiKey` 三个平铺字段）——
    /// 只在反序列化时读一次，见 `migrate()`；新配置永远不再写这三个字段（`skip_serializing`）。
    #[serde(skip_serializing)]
    pub model: String,
    #[serde(skip_serializing)]
    pub base_url: String,
    #[serde(skip_serializing)]
    pub api_key: String,
}

impl Default for TranscribeConfig {
    fn default() -> Self {
        TranscribeConfig {
            backend: "qwen".into(),
            preset: PRESETS[0].id.to_string(),
            custom_model: String::new(),
            custom_base_url: String::new(),
            keys: BTreeMap::new(),
            prices: BTreeMap::new(),
            timeout_secs: 60,
            max_per_run: 20,
            pause_ms: 300,
            auto: true,
            max_attempts: 3,
            prompt: String::new(),
            model: String::new(),
            base_url: String::new(),
            api_key: String::new(),
        }
    }
}

impl VendorConfig for TranscribeConfig {
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

impl TranscribeConfig {
    /// 老配置文件（重做预置表之前，2026-09-08 上午之前落盘的）搬进新形状——只在启动加载时调用一次。
    pub fn migrate(mut self) -> Self {
        vendorcfg::migrate_legacy(PRESETS, &mut self.preset, &mut self.custom_model, &mut self.custom_base_url, &mut self.keys, &self.model, &self.base_url, &self.api_key);
        vendorcfg::remap_retired_preset(&mut self.preset, &mut self.prices, RETIRED_PRESETS);
        self
    }
    /// 套用 PUT /config 的 JSON：可改字段逐个覆盖；`apiKey` 非空才改（存到当前厂商名下）；
    /// `clearKey:true` 清当前厂商那把。`preset` 切换预置（未知预置名拒绝）；`preset:"custom"` 时
    /// `model`/`baseUrl` 才生效，写进 `customModel`/`customBaseUrl`。`price:{input,output}` 存到当前
    /// 预置名下。共享部分见 `vendorcfg::apply_common`，这里只补这条服务独有的字段
    /// （`backend`/节流四件套/`prompt`）。
    pub fn apply(&mut self, j: &serde_json::Value) -> Result<(), String> {
        if let Some(v) = j.get("backend").and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()) {
            self.backend = v.to_string();
        }
        vendorcfg::apply_common(PRESETS, j, &mut self.preset, &mut self.custom_model, &mut self.custom_base_url, &mut self.keys, &mut self.prices)?;
        if let Some(v) = j.get("timeoutSecs").and_then(|v| v.as_u64()) { self.timeout_secs = v.clamp(5, 600); }
        if let Some(v) = j.get("maxPerRun").and_then(|v| v.as_u64()) { self.max_per_run = (v as usize).clamp(1, 500); }
        if let Some(v) = j.get("pauseMs").and_then(|v| v.as_u64()) { self.pause_ms = v.min(60_000); }
        if let Some(v) = j.get("auto").and_then(|v| v.as_bool()) { self.auto = v; }
        if let Some(v) = j.get("maxAttempts").and_then(|v| v.as_u64()) { self.max_attempts = (v as u32).clamp(1, 20); }
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
        let mut c = TranscribeConfig::default();
        assert_eq!(c.key_with_env(None), None);
        assert_eq!(c.key_with_env(Some(" env-k ".into())).as_deref(), Some("env-k"), "缺省预置是 DashScope，环境变量兜底生效");
        c.keys.insert("dashscope".into(), "file-k".into());
        assert_eq!(c.key_with_env(Some("env-k".into())).as_deref(), Some("file-k"), "文件优先");
        let p = c.public();
        assert!(p.get("apiKey").is_none() && p.get("keys").is_none(), "对外不回显 key/keys 表: {p}");
        assert_eq!(p["hasKey"], true);
        assert_eq!(p["model"], "qwen3-vl-plus");
        assert_eq!(p["keyMasked"], "...le-k", "只露最后 4 位，不整串回显");
        c.keys.remove("dashscope");
        assert_eq!(c.public()["keyMasked"], serde_json::Value::Null, "没 key 时是 null");
    }

    #[test]
    fn switching_provider_does_not_leak_other_providers_key_and_env_fallback_is_dashscope_only() {
        let mut c = TranscribeConfig::default();
        c.keys.insert("dashscope".into(), "ds-key".into());
        c.apply(&serde_json::json!({"preset": "gpt-5.6-terra"})).unwrap();
        assert_eq!(c.provider(), "openai");
        assert_eq!(c.key_with_env(Some("env-should-not-apply".into())), None, "切到 OpenAI 后既没有 openai 的 key，也不该借用 DashScope 的环境变量兜底");
        c.apply(&serde_json::json!({"apiKey": "oa-key"})).unwrap();
        assert_eq!(c.key(), Some("oa-key".into()));
        c.apply(&serde_json::json!({"preset": "qwen-vl-max"})).unwrap();
        assert_eq!(c.key(), Some("ds-key".into()), "切回 DashScope 系预置，之前存的 key 还在，不用重新粘贴");
    }

    #[test]
    fn apply_updates_only_given_fields() {
        let mut c = TranscribeConfig::default();
        c.apply(&serde_json::json!({"apiKey": "k1", "maxPerRun": 5})).unwrap();
        assert_eq!((c.key().as_deref(), c.max_per_run), (Some("k1"), 5));
        c.apply(&serde_json::json!({"apiKey": ""})).unwrap();
        assert_eq!(c.key().as_deref(), Some("k1"), "空 apiKey 不改");
        c.apply(&serde_json::json!({"clearKey": true})).unwrap();
        assert!(c.key().is_none());
        assert_eq!(c.timeout_secs, 60, "没给的字段不动");
    }

    #[test]
    fn preset_selection_sets_model_and_base_url_atomically_and_rejects_unknown() {
        let mut c = TranscribeConfig::default();
        assert_eq!(c.preset, "qwen3-vl-plus", "缺省值就是第一个预置");
        c.apply(&serde_json::json!({"preset": "qwen-vl-max"})).unwrap();
        assert_eq!((c.model(), c.base_url()), ("qwen-vl-max", "https://dashscope.aliyuncs.com/compatible-mode/v1"));
        let err = c.apply(&serde_json::json!({"preset": "gpt-4o"})).unwrap_err();
        assert!(err.contains("未知的模型预置"), "{err}");
        assert_eq!(c.preset, "qwen-vl-max", "拒绝后不改动");
    }

    #[test]
    fn custom_preset_leaves_model_and_base_url_to_the_old_manual_fields() {
        let mut c = TranscribeConfig::default();
        c.apply(&serde_json::json!({"preset": "custom", "model": "my-model", "baseUrl": "https://x.example/v1"})).unwrap();
        assert_eq!((c.model(), c.base_url()), ("my-model", "https://x.example/v1"));
        assert!(c.apply(&serde_json::json!({"baseUrl": "dashscope"})).is_err());
    }

    #[test]
    fn public_exposes_presets_and_active_preset() {
        let c = TranscribeConfig::default();
        let p = c.public();
        assert_eq!(p["activePreset"], "qwen3-vl-plus");
        assert!(p["presets"].as_array().unwrap().len() >= 4, "四家厂商都要出现在下拉里");
        assert_eq!(p["presets"][0]["id"], "qwen3-vl-plus");
        assert_eq!(p["presets"][0]["provider"], "dashscope");
    }

    #[test]
    fn price_is_user_entered_per_model_and_defaults_to_zero() {
        let mut c = TranscribeConfig::default();
        assert_eq!(c.price(), Price::default(), "没填过就是 0，不计费");
        c.apply(&serde_json::json!({"price": {"input": 0.01, "output": 0.03}})).unwrap();
        assert_eq!(c.price(), Price { input_per1k: 0.01, output_per1k: 0.03 });
        c.apply(&serde_json::json!({"preset": "gpt-5.6-terra"})).unwrap();
        assert_eq!(c.price(), Price::default(), "换了个模型，价格各记各的，不共用");
    }

    #[test]
    fn usage_key_groups_by_preset_or_custom_model() {
        let mut c = TranscribeConfig::default();
        assert_eq!(c.usage_key(), "qwen3-vl-plus");
        c.apply(&serde_json::json!({"preset": "custom", "model": "foo", "baseUrl": "https://x.example/v1"})).unwrap();
        assert_eq!(c.usage_key(), "custom:foo");
    }

    #[test]
    fn migrate_carries_forward_legacy_flat_model_and_key_without_losing_it() {
        // 老形状（2026-09-08 之前）：apiKey/model/baseUrl 平铺，没有 preset/keys 字段。
        let old = serde_json::json!({"backend":"qwen","baseUrl":"https://dashscope.aliyuncs.com/compatible-mode/v1","model":"qwen-vl-max","apiKey":"real-device-key","timeoutSecs":60,"maxPerRun":20,"pauseMs":300,"auto":true,"maxAttempts":3});
        let c: TranscribeConfig = serde_json::from_value(old).unwrap();
        assert!(c.keys.is_empty(), "反序列化本身不做迁移，只是老字段读进了兼容字段");
        let c = c.migrate();
        assert_eq!(c.preset, "qwen-vl-max", "按老 model+baseUrl 匹配回对应预置");
        assert_eq!(c.key().as_deref(), Some("real-device-key"), "真机已保存的 key 没有因为升级配置形状而丢");
        // 已经是新形状（keys 非空）的文件，迁移是 no-op。
        let migrated_twice = c.clone().migrate();
        assert_eq!(migrated_twice, c);
    }

    #[test]
    fn migrate_remaps_retired_deepseek_vision_preset() {
        let c: TranscribeConfig = serde_json::from_value(serde_json::json!({"preset":"deepseek-v4-flash-vision-exp","keys":{"deepseek":"k"}})).unwrap();
        let c = c.migrate();
        assert_eq!(c.preset, "deepseek-flash");
        assert_eq!(c.model(), "deepseek-flash");
        assert_eq!(c.key().as_deref(), Some("k"));
    }

    #[test]
    fn migrate_unmatched_legacy_combo_falls_back_to_custom() {
        let old = serde_json::json!({"apiKey":"k","model":"some-unlisted-model","baseUrl":"https://x.example/v1"});
        let c: TranscribeConfig = serde_json::from_value(old).unwrap();
        let c = c.migrate();
        assert_eq!(c.preset, "custom");
        assert_eq!((c.custom_model.as_str(), c.custom_base_url.as_str()), ("some-unlisted-model", "https://x.example/v1"));
        assert_eq!(c.key().as_deref(), Some("k"));
    }

    #[test]
    fn migrate_is_noop_for_fresh_install_with_no_legacy_data() {
        let c = TranscribeConfig::default().migrate();
        assert_eq!(c, TranscribeConfig::default(), "全新安装没有老字段，迁移不该改任何东西");
    }

    /// 真机 2026-09-08 实测采样的 transcribe.json 形状（key 值脱敏，字段名/大小写原样）：确认这次把
    /// 核心逻辑挪进 `vendorcfg` 之后，原本已经落盘的真实配置文件还能原样读出来、`key()`/`model()` 等
    /// 派生方法给出跟改之前一致的结果——这是这次重构最要紧的一条回归，真机上已经有用户配置好的 key。
    #[test]
    fn reads_real_device_config_shape_unchanged() {
        let raw = r#"{"backend":"qwen","preset":"qwen3-vl-plus","keys":{"dashscope":"REDACTED-KEY"},"prices":{"qwen3-vl-plus":{"inputPer1k":0.0,"outputPer1k":0.0}},"timeoutSecs":60,"maxPerRun":20,"pauseMs":300,"auto":false,"maxAttempts":3}"#;
        let c: TranscribeConfig = serde_json::from_str(raw).unwrap();
        assert_eq!(c.model(), "qwen3-vl-plus");
        assert_eq!(c.provider(), "dashscope");
        assert_eq!(c.key().as_deref(), Some("REDACTED-KEY"));
        assert!(!c.auto, "真机上这轮采样时自动转写是关的");
    }
}
