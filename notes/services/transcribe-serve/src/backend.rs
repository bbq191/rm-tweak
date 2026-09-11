//! 视觉后端（Strategy）：`Vision` 一个方法——给裁片 PNG 与提示词，回文本与用量。
//! 生产实现 `OpenAiCompat`：`POST {base_url}/chat/completions` + `image_url` data URI，覆盖 DashScope（Qwen）与所有 OpenAI 兼容服务；
//! 换厂只改配置 baseUrl/model/key。测试用 `Fixed`。应答解析独立成纯函数（`parse_chat_reply`）可单测。
use base64::Engine;
use std::time::Duration;
use vendorcfg::truncate_chars as trunc;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Transcript {
    pub text: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

pub trait Vision: Send + Sync {
    fn name(&self) -> &str;
    fn transcribe(&self, png: &[u8], prompt: &str) -> Result<Transcript, String>;
}

pub struct OpenAiCompat {
    pub backend: String,
    pub base_url: String,
    pub model: String,
    pub key: String,
    pub agent: ureq::Agent,
}

impl OpenAiCompat {
    pub fn new(backend: &str, base_url: &str, model: &str, key: &str, timeout: Duration) -> OpenAiCompat {
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(timeout).build();
        OpenAiCompat { backend: backend.into(), base_url: base_url.trim_end_matches('/').into(), model: model.into(), key: key.into(), agent }
    }
}

/// 请求体（纯函数，便于核对形状）。temperature 0：转写要稳定，不要发挥。
pub fn chat_request(model: &str, png: &[u8], prompt: &str) -> serde_json::Value {
    let b64 = base64::engine::general_purpose::STANDARD.encode(png);
    serde_json::json!({
        "model": model,
        "temperature": 0,
        "messages": [{"role": "user", "content": [
            {"type": "text", "text": prompt},
            {"type": "image_url", "image_url": {"url": format!("data:image/png;base64,{b64}")}}
        ]}]
    })
}

/// 解析 chat/completions 应答：`choices[0].message.content` 可能是字符串或分段数组；`usage` 缺省 0。
pub fn parse_chat_reply(v: &serde_json::Value) -> Result<Transcript, String> {
    let msg = v.pointer("/choices/0/message/content").ok_or_else(|| format!("应答无 choices[0].message.content：{}", trunc(&v.to_string(), 300)))?;
    let text = match msg {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(""),
        _ => return Err("content 形状不认识".into()),
    };
    let u = |k: &str| v.pointer(&format!("/usage/{k}")).and_then(|x| x.as_u64()).unwrap_or(0);
    Ok(Transcript { text: text.trim().to_string(), prompt_tokens: u("prompt_tokens"), completion_tokens: u("completion_tokens") })
}

impl Vision for OpenAiCompat {
    fn name(&self) -> &str {
        &self.backend
    }
    fn transcribe(&self, png: &[u8], prompt: &str) -> Result<Transcript, String> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = chat_request(&self.model, png, prompt);
        let resp = self.agent.post(&url).set("Authorization", &format!("Bearer {}", self.key)).set("Content-Type", "application/json").send_string(&body.to_string());
        let resp = match resp {
            Ok(r) => r,
            // 错误文本可能带回显，只截前 300 字，且 key 从不在应答里
            Err(ureq::Error::Status(c, r)) => return Err(format!("HTTP {c}：{}", trunc(&r.into_string().unwrap_or_default(), 300))),
            Err(e) => return Err(format!("连不上 {url}：{e}")),
        };
        let v: serde_json::Value = serde_json::from_reader(resp.into_reader()).map_err(|e| format!("应答不是 JSON：{e}"))?;
        parse_chat_reply(&v)
    }
}

/// 测试桩：固定回文（`!fail` = 模拟失败）。
#[cfg(test)]
pub struct Fixed(pub String);
#[cfg(test)]
impl Vision for Fixed {
    fn name(&self) -> &str {
        "fixed"
    }
    fn transcribe(&self, _png: &[u8], _prompt: &str) -> Result<Transcript, String> {
        if self.0 == "!fail" { Err("模拟失败".into()) } else { Ok(Transcript { text: self.0.clone(), prompt_tokens: 10, completion_tokens: 2 }) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shape_and_reply_parsing() {
        let r = chat_request("qwen3-vl-plus", b"\x89PNG", "转写");
        assert_eq!(r["model"], "qwen3-vl-plus");
        assert_eq!(r["messages"][0]["content"][0]["text"], "转写");
        assert!(r["messages"][0]["content"][1]["image_url"]["url"].as_str().unwrap().starts_with("data:image/png;base64,iVBORw"));
        let t = parse_chat_reply(&serde_json::json!({"choices":[{"message":{"content":" 查作者 \n"}}],"usage":{"prompt_tokens":123,"completion_tokens":4}})).unwrap();
        assert_eq!(t, Transcript { text: "查作者".into(), prompt_tokens: 123, completion_tokens: 4 });
        let t = parse_chat_reply(&serde_json::json!({"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}]})).unwrap();
        assert_eq!(t.text, "ab");
        assert!(parse_chat_reply(&serde_json::json!({"error":{"message":"invalid api key"}})).unwrap_err().contains("invalid api key"));
    }
}
