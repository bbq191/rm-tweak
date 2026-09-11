use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const KEY_ENV: &str = "DASHSCOPE_API_KEY";

pub const DASHSCOPE: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1";
pub const OPENAI: &str = "https://api.openai.com/v1";
pub const GEMINI: &str = "https://generativelanguage.googleapis.com/v1beta/openai/";
pub const DEEPSEEK: &str = "https://api.deepseek.com/v1";

/// baseUrl 兜底认厂商：一个厂商的 key 对它旗下所有模型都通用，不该按"这个具体型号在不在预置表里"来
/// 分格——2026-09-08 真机在 mind-serve 那边踩过这个坑（那边预置表是文字模型，没收视觉模型
/// `qwen3-vl-plus`，老配置迁移时判成"没匹配上"落进 `custom` 格，切到同样是 DashScope 的 `qwen-plus`
/// 预置后就找不到那把明明是同一账号的 key 了）。`resolve_provider()`/`migrate_legacy()` 都用它兜底：
/// baseUrl 匹配上四家已知厂商之一，不管选的是预置表里的型号还是"自定义"填的同一个地址，都能找到同一
/// 把 key。
pub fn provider_for_base_url(base_url: &str) -> Option<&'static str> {
    // 两侧都先去掉尾部 `/` 再比较（2026-09-09 审计修）：GEMINI 常量本身带尾斜杠（官方 OpenAI 兼容
    // 端点就是这个形状），是四家里唯一一个；而 apply_common() 保存用户手填的 custom_base_url 时会
    // `trim_end_matches('/')`。如果直接用 `==` 精确匹配，用户手填 Gemini 端点当"自定义"填会被判成
    // 认不出厂商、落进 custom 桶存 key，之后切到 Gemini 预置又找不到这把 key——复现过 §03w 那次
    // provider 隔离 bug 的同款体验。两侧一起 trim 后比较，不管常量以后是否再改是否带斜杠都稳。
    let b = base_url.trim_end_matches('/');
    if b == DASHSCOPE.trim_end_matches('/') {
        Some("dashscope")
    } else if b == OPENAI.trim_end_matches('/') {
        Some("openai")
    } else if b == GEMINI.trim_end_matches('/') {
        Some("gemini")
    } else if b == DEEPSEEK.trim_end_matches('/') {
        Some("deepseek")
    } else {
        None
    }
}

/// 一个预置模型选项：网页下拉给的都是"已知能用"的组合，不需要用户自己填 baseUrl。`provider` 决定这条
/// 预置的 key 存哪一格——同厂商换模型不用重新粘贴 key。视觉/文字两张预置表各自的型号不同，各服务自己
/// 的 `const PRESETS: &[Preset]` 定义在各自 `config.rs` 里，这里只是共享的表项形状。
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Preset {
    pub id: &'static str,
    pub label: &'static str,
    pub model: &'static str,
    pub base_url: &'static str,
    pub provider: &'static str,
}

/// 用户自填的每千 token 单价（缺省都是 0＝不计费）。分输入/输出两档是因为大多数厂商这两档价格不同。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Price {
    pub input_per1k: f64,
    pub output_per1k: f64,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum KeySource {
    Config,
    Env,
    None,
}

pub fn active_preset<'a>(presets: &'a [Preset], preset_id: &str) -> Option<&'a Preset> {
    presets.iter().find(|p| p.id == preset_id)
}

pub fn resolve_provider(presets: &[Preset], preset_id: &str, custom_base_url: &str) -> String {
    active_preset(presets, preset_id)
        .map(|p| p.provider.to_string())
        .or_else(|| provider_for_base_url(custom_base_url).map(str::to_string))
        .unwrap_or_else(|| "custom".to_string())
}

pub fn resolve_model<'a>(presets: &'a [Preset], preset_id: &str, custom_model: &'a str) -> &'a str {
    active_preset(presets, preset_id).map(|p| p.model).unwrap_or(custom_model)
}

pub fn resolve_base_url<'a>(presets: &'a [Preset], preset_id: &str, custom_base_url: &'a str) -> &'a str {
    active_preset(presets, preset_id).map(|p| p.base_url).unwrap_or(custom_base_url)
}

/// 解析出可用的 key（不打印、不落日志）——按 `provider` 去 `keys` 里找；环境变量只在缺省的 DashScope
/// 组合下兜底（不该让 DashScope 的环境变量被误当成其它厂商的 key）。
pub fn resolve_key(keys: &BTreeMap<String, String>, provider: &str, env: Option<String>) -> Option<String> {
    if let Some(k) = keys.get(provider).map(|s| s.trim()).filter(|s| !s.is_empty()) {
        return Some(k.to_string());
    }
    if provider == "dashscope" {
        return env.map(|e| e.trim().to_string()).filter(|e| !e.is_empty());
    }
    None
}

pub fn key_source(keys: &BTreeMap<String, String>, provider: &str) -> KeySource {
    if keys.get(provider).map(|k| !k.trim().is_empty()).unwrap_or(false) {
        KeySource::Config
    } else if provider == "dashscope" && std::env::var(KEY_ENV).map(|e| !e.trim().is_empty()).unwrap_or(false) {
        KeySource::Env
    } else {
        KeySource::None
    }
}

/// 脱敏预览：只回最后 4 位（如 `...ab12`），服务端算，绝不整串回显。
pub fn key_masked(key: &str) -> String {
    let n = key.chars().count();
    if n <= 4 {
        return "*".repeat(n);
    }
    let tail: String = key.chars().skip(n - 4).collect();
    format!("...{tail}")
}

/// 用量记账的分组键——按预置 id 分；自定义模型按 `"custom:<model>"` 分（不同自定义地址/模型各算各的）。
pub fn usage_key(preset_id: &str, custom_model: &str) -> String {
    if preset_id == "custom" { format!("custom:{custom_model}") } else { preset_id.to_string() }
}

/// 老配置文件（重做预置表之前落盘的：单一 `model`/`baseUrl`/`apiKey` 三个平铺字段，没有
/// `preset`/`keys`）搬进新形状——只在启动加载时调用一次。判据：`keys` 还空着，且老三件套至少有一个
/// 非空。已经是新形状的文件（`keys` 非空）直接 no-op，不重复迁移、不改落盘文件本身（下一次 PUT
/// /config 保存就会是新形状，旧字段因为调用方结构体上的 `skip_serializing` 自然消失）。
/// 返回值：是否真的迁移了（目前两个调用方都没用这个返回值，留着给以后可能要做的"迁移后立刻落盘一次"
/// 之类的需求，不算过度设计——签名已经定了，不用的信息不强制调用方处理，`Result`/`bool` 都比"静默"更
/// 诚实）。
/// 8 个参数确实超过 clippy 默认阈值——都是"迁移要读的老三件套"+"迁移要写的新四件套"，硬拆成一个
/// 结构体只是把参数列表换个包装位置，调用方（两个服务各自的 `migrate()`）反而要多一层组装/解包，
/// 不是真的更清楚，权衡后选择留作自由函数 + 显式 allow。
#[allow(clippy::too_many_arguments)]
pub fn migrate_legacy(
    presets: &[Preset],
    preset: &mut String,
    custom_model: &mut String,
    custom_base_url: &mut String,
    keys: &mut BTreeMap<String, String>,
    legacy_model: &str,
    legacy_base_url: &str,
    legacy_api_key: &str,
) -> bool {
    if !keys.is_empty() {
        return false;
    }
    if legacy_api_key.is_empty() && legacy_model.is_empty() && legacy_base_url.is_empty() {
        return false;
    }
    match presets.iter().find(|p| p.model == legacy_model && p.base_url == legacy_base_url) {
        Some(p) => *preset = p.id.to_string(),
        None => {
            *preset = "custom".to_string();
            *custom_model = legacy_model.to_string();
            *custom_base_url = legacy_base_url.to_string();
        }
    }
    if !legacy_api_key.is_empty() {
        // 用 resolve_provider()（按 baseUrl 兜底认厂商，不只是精确匹配预置型号），不要在这里重复一遍
        // "匹配不上就落 custom"的逻辑，保证迁移存 key 的位置永远跟运行时查 key 的位置一致。
        let provider = resolve_provider(presets, preset, custom_base_url);
        keys.insert(provider, legacy_api_key.to_string());
    }
    true
}

/// PUT /config 的 PATCH 语义里"预置选择 + 自定义 model/baseUrl + key + 价格"这一段两个服务一字不差；
/// 节流字段（`maxPerRun` 等，只有 transcribe-serve 有）和 `backend`/`prompt` 这类各服务自己按需处理的
/// 单字段留给调用方在这个函数前后自己补，不塞进这里的签名——不然参数表会无限膨胀，得不偿失。
pub fn apply_common(
    presets: &[Preset],
    j: &serde_json::Value,
    preset: &mut String,
    custom_model: &mut String,
    custom_base_url: &mut String,
    keys: &mut BTreeMap<String, String>,
    prices: &mut BTreeMap<String, Price>,
) -> Result<(), String> {
    let s = |k: &str| j.get(k).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    if let Some(v) = s("preset") {
        if v != "custom" && !presets.iter().any(|p| p.id == v) {
            return Err(format!("未知的模型预置：{v}"));
        }
        *preset = v;
    }
    if *preset == "custom" {
        if let Some(v) = s("model") {
            *custom_model = v;
        }
        if let Some(v) = s("baseUrl") {
            if !v.starts_with("http://") && !v.starts_with("https://") {
                return Err("baseUrl 要以 http(s):// 开头".into());
            }
            *custom_base_url = v.trim_end_matches('/').to_string();
        }
    }
    let provider = resolve_provider(presets, preset, custom_base_url);
    if let Some(v) = s("apiKey") {
        keys.insert(provider.clone(), v);
    }
    if j.get("clearKey").and_then(|v| v.as_bool()).unwrap_or(false) {
        keys.remove(&provider);
    }
    if let Some(price) = j.get("price") {
        let mut p = prices.get(preset.as_str()).copied().unwrap_or_default();
        if let Some(x) = price.get("input").and_then(|v| v.as_f64()) {
            p.input_per1k = x.max(0.0);
        }
        if let Some(x) = price.get("output").and_then(|v| v.as_f64()) {
            p.output_per1k = x.max(0.0);
        }
        prices.insert(preset.clone(), p);
    }
    Ok(())
}

/// 对外视图（`GET /config`）共用的 JSON 整形：`raw` 是调用方已经 `serde_json::to_value(self)` 好的
/// 完整配置对象（含各服务自己独有的字段，原样保留），这里只去掉不该回显的内部字段、补上派生字段。
#[allow(clippy::too_many_arguments)]
pub fn public_json(
    mut raw: serde_json::Value,
    presets: &[Preset],
    active_preset_id: &str,
    model: &str,
    base_url: &str,
    provider: &str,
    has_key: bool,
    source: KeySource,
    masked: Option<String>,
    price: Price,
) -> serde_json::Value {
    if let Some(o) = raw.as_object_mut() {
        o.remove("keys");
        o.remove("model");
        o.remove("baseUrl");
        o.remove("apiKey");
        o.insert("model".into(), serde_json::Value::String(model.to_string()));
        o.insert("baseUrl".into(), serde_json::Value::String(base_url.to_string()));
        o.insert("provider".into(), serde_json::Value::String(provider.to_string()));
        o.insert("hasKey".into(), serde_json::Value::Bool(has_key));
        o.insert("keySource".into(), serde_json::to_value(source).unwrap_or_default());
        o.insert("keyMasked".into(), serde_json::to_value(masked).unwrap_or(serde_json::Value::Null));
        o.insert("presets".into(), serde_json::to_value(presets).unwrap_or_default());
        o.insert("activePreset".into(), serde_json::Value::String(active_preset_id.to_string()));
        o.insert("price".into(), serde_json::to_value(price).unwrap_or_default());
    }
    raw
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESETS: &[Preset] = &[
        Preset { id: "a", label: "A", model: "model-a", base_url: DASHSCOPE, provider: "dashscope" },
        Preset { id: "b", label: "B", model: "model-b", base_url: OPENAI, provider: "openai" },
    ];

    #[test]
    fn resolve_provider_prefers_preset_then_base_url_then_custom() {
        assert_eq!(resolve_provider(PRESETS, "a", ""), "dashscope");
        assert_eq!(resolve_provider(PRESETS, "custom", DEEPSEEK), "deepseek", "预置里没有，但 baseUrl 认得出来");
        assert_eq!(resolve_provider(PRESETS, "custom", "https://x.example/v1"), "custom", "两边都认不出来才落 custom");
    }

    #[test]
    fn gemini_base_url_recognized_regardless_of_trailing_slash() {
        // 2026-09-09 审计修：GEMINI 常量自带尾斜杠，apply_common() 保存自定义 baseUrl 时会 trim 掉，
        // 修前用户手填 Gemini 端点会被判成认不出厂商、落进 custom 桶——两侧都该被视为同一个厂商。
        assert_eq!(provider_for_base_url(GEMINI), Some("gemini"), "常量本身（带尾斜杠）");
        assert_eq!(provider_for_base_url(GEMINI.trim_end_matches('/')), Some("gemini"), "去掉尾斜杠（apply_common trim 之后的形状）");
        assert_eq!(provider_for_base_url(&format!("{GEMINI}/")), Some("gemini"), "多打一个斜杠也该认得出来");
        assert_eq!(resolve_provider(PRESETS, "custom", GEMINI.trim_end_matches('/')), "gemini", "手填 Gemini 端点当自定义，也该归到 gemini provider 存 key");
    }

    #[test]
    fn resolve_key_env_fallback_only_for_dashscope_and_only_when_unset() {
        let mut keys = BTreeMap::new();
        assert_eq!(resolve_key(&keys, "dashscope", Some("env-k".into())).as_deref(), Some("env-k"));
        keys.insert("dashscope".into(), "file-k".into());
        assert_eq!(resolve_key(&keys, "dashscope", Some("env-k".into())).as_deref(), Some("file-k"), "文件优先");
        assert_eq!(resolve_key(&keys, "openai", Some("env-k".into())), None, "非 dashscope 不借用环境变量");
    }

    #[test]
    fn key_masked_keeps_last_four_chars_only() {
        assert_eq!(key_masked("sk-abcd1234"), "...1234");
        assert_eq!(key_masked("ab"), "**");
    }

    #[test]
    fn usage_key_groups_by_preset_or_custom_model() {
        assert_eq!(usage_key("a", ""), "a");
        assert_eq!(usage_key("custom", "foo"), "custom:foo");
    }

    #[test]
    fn migrate_legacy_matches_known_preset_then_falls_back_to_custom() {
        let (mut preset, mut cm, mut cb) = (String::new(), String::new(), String::new());
        let mut keys = BTreeMap::new();
        let changed = migrate_legacy(PRESETS, &mut preset, &mut cm, &mut cb, &mut keys, "model-a", DASHSCOPE, "k1");
        assert!(changed);
        assert_eq!(preset, "a");
        assert_eq!(keys.get("dashscope").map(String::as_str), Some("k1"));

        let (mut preset2, mut cm2, mut cb2) = (String::new(), String::new(), String::new());
        let mut keys2 = BTreeMap::new();
        migrate_legacy(PRESETS, &mut preset2, &mut cm2, &mut cb2, &mut keys2, "unlisted-model", DEEPSEEK, "k2");
        assert_eq!(preset2, "custom");
        assert_eq!((cm2.as_str(), cb2.as_str()), ("unlisted-model", DEEPSEEK));
        assert_eq!(keys2.get("deepseek").map(String::as_str), Some("k2"), "baseUrl 兜底认出厂商，没有落进字面 custom 格");
    }

    #[test]
    fn migrate_legacy_is_noop_when_keys_already_present_or_nothing_to_migrate() {
        let (mut preset, mut cm, mut cb) = ("x".to_string(), String::new(), String::new());
        let mut keys = BTreeMap::from([("dashscope".to_string(), "already-here".to_string())]);
        assert!(!migrate_legacy(PRESETS, &mut preset, &mut cm, &mut cb, &mut keys, "model-a", DASHSCOPE, "ignored"), "keys 非空就不迁移");
        assert_eq!(preset, "x", "没被动过");

        let (mut preset2, mut cm2, mut cb2) = ("y".to_string(), String::new(), String::new());
        let mut keys2 = BTreeMap::new();
        assert!(!migrate_legacy(PRESETS, &mut preset2, &mut cm2, &mut cb2, &mut keys2, "", "", ""), "全新安装没有老数据");
    }

    #[test]
    fn apply_common_rejects_unknown_preset_and_scopes_key_by_provider() {
        let (mut preset, mut cm, mut cb) = ("a".to_string(), String::new(), String::new());
        let mut keys = BTreeMap::new();
        let mut prices = BTreeMap::new();
        let err = apply_common(PRESETS, &serde_json::json!({"preset":"no-such"}), &mut preset, &mut cm, &mut cb, &mut keys, &mut prices).unwrap_err();
        assert!(err.contains("未知的模型预置"), "{err}");

        apply_common(PRESETS, &serde_json::json!({"apiKey":"k1"}), &mut preset, &mut cm, &mut cb, &mut keys, &mut prices).unwrap();
        assert_eq!(keys.get("dashscope").map(String::as_str), Some("k1"));

        apply_common(PRESETS, &serde_json::json!({"preset":"b"}), &mut preset, &mut cm, &mut cb, &mut keys, &mut prices).unwrap();
        apply_common(PRESETS, &serde_json::json!({"clearKey":true}), &mut preset, &mut cm, &mut cb, &mut keys, &mut prices).unwrap();
        assert!(!keys.contains_key("openai"), "clearKey 只清当前 provider 那把，不动 dashscope 那把");
        assert_eq!(keys.get("dashscope").map(String::as_str), Some("k1"), "换预置不影响别的厂商已存的 key");
    }

    #[test]
    fn apply_common_rejects_custom_base_url_without_scheme() {
        let (mut preset, mut cm, mut cb) = ("custom".to_string(), String::new(), String::new());
        let mut keys = BTreeMap::new();
        let mut prices = BTreeMap::new();
        let err = apply_common(PRESETS, &serde_json::json!({"baseUrl":"dashscope"}), &mut preset, &mut cm, &mut cb, &mut keys, &mut prices).unwrap_err();
        assert!(err.contains("http"), "{err}");
    }

    #[test]
    fn public_json_strips_secrets_and_adds_derived_fields() {
        let raw = serde_json::json!({"backend":"qwen","preset":"a","keys":{"dashscope":"secret"},"maxPerRun":20});
        let v = public_json(raw, PRESETS, "a", "model-a", DASHSCOPE, "dashscope", true, KeySource::Config, Some("...cret".into()), Price { input_per1k: 0.1, output_per1k: 0.2 });
        assert!(v.get("keys").is_none(), "不回显 keys 表: {v}");
        assert_eq!(v["model"], "model-a");
        assert_eq!(v["hasKey"], true);
        assert_eq!(v["keyMasked"], "...cret");
        assert_eq!(v["maxPerRun"], 20, "调用方自己的字段原样透传");
        assert_eq!(v["presets"].as_array().unwrap().len(), 2);
    }
}
