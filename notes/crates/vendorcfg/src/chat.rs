//! OpenAI 兼容 `POST {base_url}/chat/completions` 的传输与应答解析（`mind-serve` 文字问答 / `transcribe-serve`
//! 视觉转写共用）。两边此前各写了一份逐行相同的 `OpenAiCompat::ask/transcribe` 网络部分与 `parse_chat_reply`
//! （只有请求体不同：视觉多一个 `image_url` 段、温度不同），2026-09-20 收进这里。**只抽传输和解析**：
//! 请求体怎么拼（`chat_request`）、`Vision`/`TextModel` 这类业务 trait 仍是各服务自己的关注点。
use crate::truncate_chars as trunc;
use std::time::Duration;

/// 一次调用的结果：文本 + token 用量。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChatReply {
    pub text: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

/// 一个已配置好的调用端：后端标识 + baseUrl（去尾 `/`）+ 模型 + key + 带超时的 agent。两个服务此前各有一个字段、
/// 构造逐行相同的 `OpenAiCompat`（2026-09-24 第三轮审计收进来）；各服务的业务 trait（`Vision`/`TextModel`）直接
/// 实现在它身上，只管拼自己的请求体。`Debug` 故意不派生：结构里有 key，别让它有机会被 `{:?}` 打进日志。
pub struct ChatClient {
    backend: String,
    base_url: String,
    model: String,
    key: String,
    agent: ureq::Agent,
}

impl ChatClient {
    pub fn new(backend: &str, base_url: &str, model: &str, key: &str, timeout: Duration) -> ChatClient {
        ChatClient { backend: backend.into(), base_url: base_url.trim_end_matches('/').into(), model: model.into(), key: key.into(), agent: agent(timeout) }
    }
    /// 按当前配置建调用端；没 key 返回 `Err(missing_key)`（各服务的提示文案不同，调用方给）。
    pub fn from_config<C: crate::VendorConfig>(cfg: &C, backend: &str, timeout: Duration, missing_key: &str) -> Result<ChatClient, String> {
        let key = cfg.key().ok_or_else(|| missing_key.to_string())?;
        Ok(ChatClient::new(backend, cfg.base_url(), cfg.model(), &key, timeout))
    }
    /// 写进草稿/回答 `backend` 字段的后端标识。
    pub fn backend(&self) -> &str {
        &self.backend
    }
    pub fn model(&self) -> &str {
        &self.model
    }
    /// 发一次 `chat/completions`（请求体由调用方拼，见 [`post_chat`]）。
    pub fn post(&self, body: &serde_json::Value) -> Result<ChatReply, String> {
        post_chat(&self.agent, &self.base_url, &self.key, body)
    }
}

/// 建调用用的 agent：连接 10 秒超时 + 整个请求 `timeout`。
pub fn agent(timeout: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout(timeout).build()
}

/// 解析 chat/completions 应答：`choices[0].message.content` 可能是字符串或分段数组；`usage` 缺省 0。
pub fn parse_chat_reply(v: &serde_json::Value) -> Result<ChatReply, String> {
    let msg = v.pointer("/choices/0/message/content").ok_or_else(|| format!("应答无 choices[0].message.content：{}", trunc(&v.to_string(), 300)))?;
    let text = match msg {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(parts) => parts.iter().filter_map(|p| p.get("text").and_then(|t| t.as_str())).collect::<Vec<_>>().join(""),
        _ => return Err("content 形状不认识".into()),
    };
    let u = |k: &str| v.pointer(&format!("/usage/{k}")).and_then(|x| x.as_u64()).unwrap_or(0);
    Ok(ChatReply { text: text.trim().to_string(), prompt_tokens: u("prompt_tokens"), completion_tokens: u("completion_tokens") })
}

/// 发一次调用：`Authorization: Bearer <key>`；非 2xx 带状态码和截断到 300 字的应答体（错误文本可能回显请求内容，
/// 所以截断；key 从不出现在应答里）。`base_url` 末尾的 `/` 由调用方在构造时去掉。
pub fn post_chat(agent: &ureq::Agent, base_url: &str, key: &str, body: &serde_json::Value) -> Result<ChatReply, String> {
    let url = format!("{base_url}/chat/completions");
    let resp = agent.post(&url).set("Authorization", &format!("Bearer {key}")).set("Content-Type", "application/json").send_string(&body.to_string());
    let resp = match resp {
        Ok(r) => r,
        Err(ureq::Error::Status(c, r)) => return Err(format!("HTTP {c}：{}", trunc(&r.into_string().unwrap_or_default(), 300))),
        Err(e) => return Err(format!("连不上 {url}：{e}")),
    };
    let v: serde_json::Value = serde_json::from_reader(resp.into_reader()).map_err(|e| format!("应答不是 JSON：{e}"))?;
    parse_chat_reply(&v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_string_and_segmented_content_and_usage() {
        let r = parse_chat_reply(&serde_json::json!({"choices":[{"message":{"content":" 答案 \n"}}],"usage":{"prompt_tokens":50,"completion_tokens":8}})).unwrap();
        assert_eq!(r, ChatReply { text: "答案".into(), prompt_tokens: 50, completion_tokens: 8 });
        let r = parse_chat_reply(&serde_json::json!({"choices":[{"message":{"content":[{"type":"text","text":"甲"},{"type":"text","text":"乙"}]}}]})).unwrap();
        assert_eq!((r.text.as_str(), r.prompt_tokens), ("甲乙", 0), "usage 缺省 0");
        assert!(parse_chat_reply(&serde_json::json!({"error":"x"})).unwrap_err().contains("choices"));
        assert!(parse_chat_reply(&serde_json::json!({"choices":[{"message":{"content":5}}]})).is_err());
    }

    #[test]
    fn chat_client_trims_base_url_and_sends_bearer_key() {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let h = std::thread::spawn(move || {
            let req = server.recv().unwrap();
            let seen = (req.url().to_string(), req.headers().iter().find(|h| h.field.equiv("Authorization")).map(|h| h.value.to_string()));
            let _ = req.respond(tiny_http::Response::from_string(r#"{"choices":[{"message":{"content":"好"}}],"usage":{"prompt_tokens":3,"completion_tokens":1}}"#));
            seen
        });
        let c = ChatClient::new("qwen", &format!("http://127.0.0.1:{port}/v1/"), "m", "sk-1", Duration::from_secs(5));
        assert_eq!((c.backend(), c.model()), ("qwen", "m"));
        let r = c.post(&serde_json::json!({})).unwrap();
        assert_eq!((r.text.as_str(), r.prompt_tokens), ("好", 3));
        let (url, auth) = h.join().unwrap();
        assert_eq!(url, "/v1/chat/completions", "baseUrl 末尾的 / 去掉，不出现 //");
        assert_eq!(auth.as_deref(), Some("Bearer sk-1"));
    }

    #[test]
    fn post_chat_maps_status_and_transport_errors() {
        // 状态码错误：带状态码与应答体（截断）
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let _ = req.respond(tiny_http::Response::from_string("bad key").with_status_code(401));
            }
        });
        let a = agent(Duration::from_secs(5));
        let e = post_chat(&a, &format!("http://127.0.0.1:{port}"), "k", &serde_json::json!({})).unwrap_err();
        assert!(e.contains("HTTP 401") && e.contains("bad key"), "{e}");
        // 连不上
        let e = post_chat(&a, "http://127.0.0.1:1", "k", &serde_json::json!({})).unwrap_err();
        assert!(e.contains("连不上"), "{e}");
    }
}
