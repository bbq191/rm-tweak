//! 文字后端（Strategy）：`TextModel` 一个方法——给拼好的提示词，回文本与用量。
//! 生产实现 `OpenAiCompat`：`POST {base_url}/chat/completions`，纯文本消息，覆盖 DashScope（Qwen）与所有 OpenAI 兼容服务；
//! 换厂只改配置 baseUrl/model/key。跟 transcribe-serve 的同名结构同一个模式，区别只是没有 `image_url`——
//! 两边各自成文件，没有共享 crate：都是几十行胶水，抽公共 crate 不值当（shelf 工程原则"专项专用"）——
//! 这条评估过没变，唯独字符截断这一个小工具函数（`trunc`）2026-09-09 审计发现三处（这里/
//! transcribe-serve 同名函数/`mind-serve::prompt::take`）几乎逐字节重复，两边都已经依赖 `vendorcfg`，
//! 收进去一行改动量，跟"不共享 OpenAiCompat 本体"这个决定不矛盾——只是复用一个通用字符串工具。
use std::time::Duration;
use vendorcfg::truncate_chars as trunc;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Reply {
    pub text: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

pub trait TextModel: Send + Sync {
    fn name(&self) -> &str;
    fn ask(&self, prompt: &str) -> Result<Reply, String>;
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

/// 请求体（纯函数，便于核对形状）。temperature 给点余地（0.3）——问答不是转写，允许组织语言，但别太发挥。
pub fn chat_request(model: &str, prompt: &str) -> serde_json::Value {
    serde_json::json!({"model": model, "temperature": 0.3, "messages": [{"role": "user", "content": prompt}]})
}

/// 解析 chat/completions 应答：`choices[0].message.content` 可能是字符串或分段数组；`usage` 缺省 0。
/// 跟 transcribe-serve::backend::parse_chat_reply 逻辑一样（应答形状是 OpenAI 兼容口的通用约定，不是转写特有的）。
pub fn parse_chat_reply(v: &serde_json::Value) -> Result<Reply, String> {
    let msg = v.pointer("/choices/0/message/content").ok_or_else(|| format!("应答无 choices[0].message.content：{}", trunc(&v.to_string(), 300)))?;
    let text = match msg {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(""),
        _ => return Err("content 形状不认识".into()),
    };
    let u = |k: &str| v.pointer(&format!("/usage/{k}")).and_then(|x| x.as_u64()).unwrap_or(0);
    Ok(Reply { text: text.trim().to_string(), prompt_tokens: u("prompt_tokens"), completion_tokens: u("completion_tokens") })
}

impl TextModel for OpenAiCompat {
    fn name(&self) -> &str {
        &self.backend
    }
    fn ask(&self, prompt: &str) -> Result<Reply, String> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = chat_request(&self.model, prompt);
        let resp = self.agent.post(&url).set("Authorization", &format!("Bearer {}", self.key)).set("Content-Type", "application/json").send_string(&body.to_string());
        let resp = match resp {
            Ok(r) => r,
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
impl TextModel for Fixed {
    fn name(&self) -> &str {
        "fixed"
    }
    fn ask(&self, _prompt: &str) -> Result<Reply, String> {
        if self.0 == "!fail" { Err("模拟失败".into()) } else { Ok(Reply { text: self.0.clone(), prompt_tokens: 10, completion_tokens: 2 }) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_shape_and_reply_parsing() {
        let r = chat_request("qwen-plus", "问一下");
        assert_eq!(r["model"], "qwen-plus");
        assert_eq!(r["messages"][0]["content"], "问一下");
        assert!(r["messages"][0]["content"].is_string(), "纯文本消息，不是分段数组，不带图（跟 transcribe-serve 的 chat_request 不一样）");
        let t = parse_chat_reply(&serde_json::json!({"choices":[{"message":{"content":" 答案 \n"}}],"usage":{"prompt_tokens":50,"completion_tokens":8}})).unwrap();
        assert_eq!(t, Reply { text: "答案".into(), prompt_tokens: 50, completion_tokens: 8 });
        let t = parse_chat_reply(&serde_json::json!({"choices":[{"message":{"content":[{"type":"text","text":"a"},{"type":"text","text":"b"}]}}]})).unwrap();
        assert_eq!(t.text, "ab");
        assert!(parse_chat_reply(&serde_json::json!({"error":{"message":"invalid api key"}})).unwrap_err().contains("invalid api key"));
    }
}
