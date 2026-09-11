//! 文字后端（Strategy）：`TextModel` 一个方法——给拼好的提示词，回文本与用量。
//! 生产实现是 `vendorcfg::ChatClient`（2026-09-24 起取代本地 `OpenAiCompat` 壳）：`POST {base_url}/chat/completions`，纯文本消息，覆盖 DashScope（Qwen）与所有 OpenAI 兼容服务；
//! 换厂只改配置 baseUrl/model/key。跟 transcribe-serve 的同名结构同一个模式，区别只是没有 `image_url`。
//! **传输（POST + 错误截断）与应答解析已收进 `vendorcfg::chat`（2026-09-20，两边此前各抄一份）**——这里只留
//! 请求体（纯文本消息、温度 0.3）与 `TextModel` 这个业务 trait。
use vendorcfg::ChatClient;

/// 一次问答的结果（文本 + token 用量）：与视觉转写同形，共用 `vendorcfg::ChatReply`。
pub type Reply = vendorcfg::ChatReply;

pub trait TextModel: Send + Sync {
    fn name(&self) -> &str;
    fn ask(&self, prompt: &str) -> Result<Reply, String>;
}

/// 请求体（纯函数，便于核对形状）。temperature 给点余地（0.3）——问答不是转写，允许组织语言，但别太发挥。
pub fn chat_request(model: &str, prompt: &str) -> serde_json::Value {
    serde_json::json!({"model": model, "temperature": 0.3, "messages": [{"role": "user", "content": prompt}]})
}

/// 解析 chat/completions 应答（传输与解析共用 `vendorcfg::chat`，跟 transcribe-serve 是同一份）。
#[cfg(test)]
use vendorcfg::parse_chat_reply;

impl TextModel for ChatClient {
    fn name(&self) -> &str {
        self.backend()
    }
    fn ask(&self, prompt: &str) -> Result<Reply, String> {
        self.post(&chat_request(self.model(), prompt))
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
