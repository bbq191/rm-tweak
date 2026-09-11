//! 视觉后端（Strategy）：`Vision` 一个方法——给裁片 PNG 与提示词，回文本与用量。
//! 生产实现是 `vendorcfg::ChatClient`（2026-09-24 起取代本地 `OpenAiCompat` 壳）：`POST {base_url}/chat/completions` + `image_url` data URI，覆盖 DashScope（Qwen）与所有 OpenAI 兼容服务；
//! 换厂只改配置 baseUrl/model/key。测试用 `Fixed`。传输与应答解析（`post_chat`/`parse_chat_reply`）已收进
//! `vendorcfg::chat`（2026-09-20，跟 mind-serve 此前各抄一份）；这里只留带 `image_url` 的请求体与 `Vision` trait。
use base64::Engine;
use vendorcfg::ChatClient;

/// 一次转写的结果（文本 + token 用量）：与文字问答同形，共用 `vendorcfg::ChatReply`。
pub type Transcript = vendorcfg::ChatReply;

pub trait Vision: Send + Sync {
    fn name(&self) -> &str;
    fn transcribe(&self, png: &[u8], prompt: &str) -> Result<Transcript, String>;
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

/// 解析 chat/completions 应答（应答形状是 OpenAI 兼容口的通用约定，传输与解析共用 `vendorcfg::chat`）。
#[cfg(test)]
use vendorcfg::parse_chat_reply;

impl Vision for ChatClient {
    fn name(&self) -> &str {
        self.backend()
    }
    fn transcribe(&self, png: &[u8], prompt: &str) -> Result<Transcript, String> {
        self.post(&chat_request(self.model(), png, prompt))
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
