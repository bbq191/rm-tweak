//! HTTP 适配层（唯一碰 tiny_http 的地方）。领域模块只见 [`Request`]/[`Reply`] 两个纯数据类型。
//! 路由 = (方法, 路径模式) → 处理函数；路径模式支持尾部 `/*` 前缀匹配与单段 `{param}`。
mod router;
mod server;
pub use router::{parse_query, Router};
pub use server::{serve, serve_with, Guard, GuardFn, GuardRequest, ServeOpts, DEFAULT_MAX_CONCURRENT};

use serde::Serialize;
use std::collections::HashMap;
use std::io::Read;
use std::sync::Arc;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Method {
    Get,
    Post,
    Put,
    Delete,
    Options,
    Other,
}

impl Method {
    fn from_tiny(m: &tiny_http::Method) -> Method {
        match m {
            tiny_http::Method::Get => Method::Get,
            tiny_http::Method::Post => Method::Post,
            tiny_http::Method::Put => Method::Put,
            tiny_http::Method::Delete => Method::Delete,
            tiny_http::Method::Options => Method::Options,
            _ => Method::Other,
        }
    }
}

/// 进入领域的请求视图：body 是流（大文件不读进内存）。
pub struct Request<'a> {
    pub method: Method,
    pub path: String,
    pub query: HashMap<String, String>,
    pub params: HashMap<String, String>,
    pub content_type: String,
    pub content_length: Option<usize>,
    /// 少量请求头（Cookie/Authorization/Accept/Host/X-Forwarded-Proto），登录/守卫用。
    pub headers: Vec<(String, String)>,
    pub body: &'a mut dyn Read,
}

/// 服务器写入的内部头：TCP 对端 IP。**只由服务器写**——客户端发来的同名头不在保留白名单里、进不来，
/// 所以处理函数读到的一定是真实对端地址（不是 `X-Forwarded-For` 这类可伪造的值）。
pub const REMOTE_IP_HEADER: &str = "X-Rmsvc-Remote-Ip";

/// [`Request::read_small_body`] 的上限（1MB）：JSON 表单、KOReader 配置补丁这类小请求体。
pub const SMALL_BODY_MAX: u64 = 1024 * 1024;

/// 按名取头（不区分大小写）——[`Request`] 与 [`GuardRequest`] 共用。
fn header_of<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
}

/// JSON 请求体的取值门面：缺字段 / 类型不对统一变 400 `ApiError`，各服务不再各写一遍
/// `j.get("name").and_then(|v| v.as_str()).ok_or_else(|| ApiError::bad("缺 name"))`。
pub struct JsonBody(pub serde_json::Value);

impl JsonBody {
    /// 必填字符串字段。
    pub fn str(&self, key: &str) -> Result<&str, ApiError> {
        self.0.get(key).and_then(|v| v.as_str()).ok_or_else(|| ApiError::bad(format!("缺 {key}")))
    }
    /// 可选字符串字段（去首尾空白；缺 / 空白 → `default`）。
    pub fn str_or<'a>(&'a self, key: &str, default: &'a str) -> &'a str {
        self.0.get(key).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty()).unwrap_or(default)
    }
    pub fn bool_or(&self, key: &str, default: bool) -> bool {
        self.0.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
    }
}

impl Request<'_> {
    /// 按名取头（不区分大小写）。
    pub fn header(&self, name: &str) -> Option<&str> {
        header_of(&self.headers, name)
    }
    /// TCP 对端 IP（见 [`REMOTE_IP_HEADER`]）；测试里手工构造、没填这个头时为 `None`。
    pub fn remote_ip(&self) -> Option<std::net::IpAddr> {
        self.header(REMOTE_IP_HEADER).and_then(|v| v.parse().ok())
    }
    /// 查询参数是否为真（`1` / `true`）。
    pub fn q_flag(&self, k: &str) -> bool {
        matches!(self.q(k), Some("1") | Some("true"))
    }
    /// `application/x-www-form-urlencoded` 表单 → map（小 body）。
    pub fn form_body(&mut self) -> Result<HashMap<String, String>, String> {
        let b = self.read_small_body()?;
        Ok(parse_query(&String::from_utf8_lossy(&b).replace('+', " ")))
    }
    pub fn q(&self, k: &str) -> Option<&str> {
        self.query.get(k).map(|s| s.as_str())
    }
    pub fn param(&self, k: &str) -> &str {
        self.params.get(k).map(|s| s.as_str()).unwrap_or("")
    }
    /// 小 body（JSON 表单 / 配置补丁）整体读入，上限 [`SMALL_BODY_MAX`]。**超限报错**而不是截断：此前
    /// `take(1MB)` 静默截断，超长的 KOReader 配置补丁会被切成半截再交给合并脚本、JSON 报一句莫名的解析错
    /// （2026-09-24 审计）。多读 1 字节即可判定超限，不必读完整个超长 body。
    pub fn read_small_body(&mut self) -> Result<Vec<u8>, String> {
        let too_big = || format!("请求体超过 {} KB 上限", SMALL_BODY_MAX / 1024);
        if self.content_length.is_some_and(|n| n as u64 > SMALL_BODY_MAX) {
            return Err(too_big());
        }
        let mut v = Vec::new();
        self.body.take(SMALL_BODY_MAX + 1).read_to_end(&mut v).map_err(|e| e.to_string())?;
        if v.len() as u64 > SMALL_BODY_MAX {
            return Err(too_big());
        }
        Ok(v)
    }
    pub fn json_body(&mut self) -> Result<serde_json::Value, String> {
        let b = self.read_small_body()?;
        serde_json::from_slice(&b).map_err(|e| format!("JSON 解析失败: {e}"))
    }
    /// JSON body → [`JsonBody`]（解析失败 400）。
    pub fn json(&mut self) -> Result<JsonBody, ApiError> {
        self.json_body().map(JsonBody).map_err(ApiError::bad)
    }
    /// multipart 请求的 boundary；非 multipart → 400。
    pub fn multipart_boundary(&self) -> Result<String, ApiError> {
        crate::multipart::boundary_of(&self.content_type).ok_or_else(|| ApiError::bad("需要 multipart/form-data"))
    }
}

pub struct Reply {
    pub status: u16,
    pub content_type: String,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
    /// 流式响应体（SSE 等）：Some 时忽略 `body`，chunked 边读边发，直到 reader 返回 0 或客户端断开。
    pub stream: Option<Box<dyn Read + Send>>,
}

impl Reply {
    pub fn json<T: Serialize>(status: u16, v: &T) -> Reply {
        Reply { status, content_type: "application/json; charset=utf-8".into(), body: serde_json::to_vec(v).unwrap_or_default(), headers: vec![], stream: None }
    }
    pub fn ok<T: Serialize>(v: &T) -> Reply {
        Reply::json(200, v)
    }
    pub fn error(status: u16, message: impl Into<String>) -> Reply {
        Reply::json(status, &serde_json::json!({"ok": false, "message": message.into()}))
    }
    pub fn html(body: &str) -> Reply {
        Reply { status: 200, content_type: "text/html; charset=utf-8".into(), body: body.as_bytes().to_vec(), headers: vec![], stream: None }
    }
    pub fn bytes(content_type: &str, body: Vec<u8>) -> Reply {
        Reply { status: 200, content_type: content_type.into(), body, headers: vec![], stream: None }
    }
    /// 流式响应（SSE）：不知长度，读到 reader 结束或客户端断开为止。
    pub fn stream(content_type: &str, reader: Box<dyn Read + Send>) -> Reply {
        Reply { status: 200, content_type: content_type.into(), body: Vec::new(), headers: vec![], stream: Some(reader) }
    }
    /// 已知长度的流（文件下载）：带 `Content-Length`，服务器按定长响应边读边发，发完即结束（不走 SSE 那条路）。
    pub fn sized_stream(content_type: &str, reader: Box<dyn Read + Send>, len: u64) -> Reply {
        Reply::stream(content_type, reader).with_header("Content-Length", &len.to_string())
    }
    pub fn not_found() -> Reply {
        Reply::error(404, "not found")
    }
    /// 303 跳转（表单提交后用 303 避免重复提交）。
    pub fn redirect(location: &str) -> Reply {
        Reply { status: 303, content_type: "text/plain; charset=utf-8".into(), body: Vec::new(), headers: vec![("Location".into(), location.into())], stream: None }
    }
    pub fn with_header(mut self, k: &str, v: &str) -> Reply {
        self.headers.push((k.into(), v.into()));
        self
    }
    pub fn with_status(mut self, status: u16) -> Reply {
        self.status = status;
        self
    }
}

/// 领域错误 → 回执：`Err(ApiError)` 统一变 JSON。
#[derive(Debug)]
pub struct ApiError {
    pub status: u16,
    pub message: String,
}
impl ApiError {
    pub fn bad(m: impl Into<String>) -> ApiError {
        ApiError { status: 400, message: m.into() }
    }
    pub fn internal(m: impl Into<String>) -> ApiError {
        ApiError { status: 500, message: m.into() }
    }
    pub fn not_found(m: impl Into<String>) -> ApiError {
        ApiError { status: 404, message: m.into() }
    }
}
impl From<String> for ApiError {
    fn from(m: String) -> Self {
        ApiError::internal(m)
    }
}
impl From<ApiError> for Reply {
    fn from(e: ApiError) -> Reply {
        Reply::error(e.status, e.message)
    }
}
pub type ApiResult = Result<Reply, ApiError>;

pub type Handler = Arc<dyn Fn(&mut Request<'_>) -> ApiResult + Send + Sync>;

/// 把共享状态绑进处理函数：`router.get("/x", bind(&st, |st, r| …))`。
/// 替代各服务 main 里 `let (s1, s2, s3, …) = (st.clone(), …)` 的手工克隆串。
pub fn bind<S, F>(state: &Arc<S>, f: F) -> impl Fn(&mut Request<'_>) -> ApiResult + Send + Sync + 'static
where
    S: Send + Sync + 'static,
    F: Fn(&S, &mut Request<'_>) -> ApiResult + Send + Sync + 'static,
{
    let st = state.clone();
    move |r| f(&st, r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_body_accessors() {
        let b = JsonBody(serde_json::json!({"name": "x", "folder": "  ", "keep": false}));
        assert_eq!(b.str("name").unwrap(), "x");
        assert_eq!(b.str("nope").unwrap_err().message, "缺 nope");
        assert_eq!(b.str_or("folder", "lib"), "lib", "空白当缺省");
        assert!(!b.bool_or("keep", true) && b.bool_or("other", true));
    }

    fn req_with<'a>(body: &'a mut &[u8], content_length: Option<usize>) -> Request<'a> {
        Request { method: Method::Post, path: "/".into(), query: HashMap::new(), params: HashMap::new(), content_type: String::new(), content_length, headers: vec![], body }
    }

    /// 回归：超过上限的小 body 报错，而不是静默截断成半截内容交给调用方。
    #[test]
    fn small_body_rejects_oversize_instead_of_truncating() {
        let max = SMALL_BODY_MAX as usize;
        let exact = vec![b'a'; max];
        let mut r: &[u8] = &exact;
        assert_eq!(req_with(&mut r, None).read_small_body().unwrap().len(), max, "恰好上限照收");
        let over = vec![b'a'; max + 1];
        let mut r: &[u8] = &over;
        assert!(req_with(&mut r, None).read_small_body().unwrap_err().contains("上限"), "没有 Content-Length（chunked）也按实际字节判");
        let mut r: &[u8] = b"{}";
        assert!(req_with(&mut r, Some(max + 1)).read_small_body().is_err(), "声明长度超限直接拒，不读 body");
        let mut r: &[u8] = b"{\"a\":1}";
        assert_eq!(req_with(&mut r, Some(7)).json().unwrap().0["a"], 1, "正常小 body 不受影响");
    }
}
