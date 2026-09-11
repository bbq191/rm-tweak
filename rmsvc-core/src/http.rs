//! HTTP 适配层（唯一碰 tiny_http 的地方）。领域模块只见 [`Request`]/[`Reply`] 两个纯数据类型。
//! 路由 = (方法, 路径模式) → 处理函数；路径模式支持尾部 `/*` 前缀匹配与单段 `{param}`。
use serde::Serialize;
use std::collections::HashMap;
use std::io::{Read, Write};
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
    /// 小 body（JSON 表单）整体读入，上限 1MB。
    pub fn read_small_body(&mut self) -> Result<Vec<u8>, String> {
        let mut v = Vec::new();
        self.body.take(1024 * 1024).read_to_end(&mut v).map_err(|e| e.to_string())?;
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
    /// 流式响应（SSE / 大文件边读边发）：不知长度，chunked。
    pub fn stream(content_type: &str, reader: Box<dyn Read + Send>) -> Reply {
        Reply { status: 200, content_type: content_type.into(), body: Vec::new(), headers: vec![], stream: Some(reader) }
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

/// 路径模式（分段；尾 `*` 前缀匹配；`{x}` 参数）。
struct Pattern {
    segs: Vec<String>,
    prefix: bool,
}

impl Pattern {
    fn parse(pattern: &str) -> Pattern {
        let mut segs: Vec<String> = pattern.trim_matches('/').split('/').filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
        let prefix = segs.last().map(|s| s == "*").unwrap_or(false);
        if prefix {
            segs.pop();
        }
        Pattern { segs, prefix }
    }

    /// 匹配则返回参数表（`{x}` 解码后；前缀模式额外给 `*`=余下路径）。
    fn matches(&self, path: &str) -> Option<HashMap<String, String>> {
        let segs: Vec<&str> = path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
        if self.prefix {
            if segs.len() < self.segs.len() {
                return None;
            }
        } else if segs.len() != self.segs.len() {
            return None;
        }
        let mut params = HashMap::new();
        for (pat, seg) in self.segs.iter().zip(segs.iter()) {
            if pat.starts_with('{') && pat.ends_with('}') {
                params.insert(pat[1..pat.len() - 1].to_string(), crate::multipart::percent_decode(seg));
            } else if pat != seg {
                return None;
            }
        }
        if self.prefix {
            params.insert("*".into(), segs[self.segs.len()..].join("/"));
        }
        Some(params)
    }
}

struct Route {
    method: Method,
    pattern: Pattern,
    handler: Handler,
}

#[derive(Default, Clone)]
pub struct Router {
    routes: Vec<Arc<Route>>,
}

impl Router {
    pub fn new() -> Router {
        Router::default()
    }
    pub fn route<F>(self, method: Method, pattern: &str, f: F) -> Router
    where
        F: Fn(&mut Request<'_>) -> ApiResult + Send + Sync + 'static,
    {
        self.any(&[method], pattern, f)
    }
    /// 同一处理函数挂到多个方法（反向代理这类"方法无关"的路由不必写四遍）。
    pub fn any<F>(mut self, methods: &[Method], pattern: &str, f: F) -> Router
    where
        F: Fn(&mut Request<'_>) -> ApiResult + Send + Sync + 'static,
    {
        let handler: Handler = Arc::new(f);
        for &method in methods {
            self.routes.push(Arc::new(Route { method, pattern: Pattern::parse(pattern), handler: handler.clone() }));
        }
        self
    }
    pub fn get<F>(self, p: &str, f: F) -> Router
    where
        F: Fn(&mut Request<'_>) -> ApiResult + Send + Sync + 'static,
    {
        self.route(Method::Get, p, f)
    }
    pub fn post<F>(self, p: &str, f: F) -> Router
    where
        F: Fn(&mut Request<'_>) -> ApiResult + Send + Sync + 'static,
    {
        self.route(Method::Post, p, f)
    }
    pub fn put<F>(self, p: &str, f: F) -> Router
    where
        F: Fn(&mut Request<'_>) -> ApiResult + Send + Sync + 'static,
    {
        self.route(Method::Put, p, f)
    }
    pub fn delete<F>(self, p: &str, f: F) -> Router
    where
        F: Fn(&mut Request<'_>) -> ApiResult + Send + Sync + 'static,
    {
        self.route(Method::Delete, p, f)
    }

    /// 追加另一组路由（保持各自顺序，self 的在前）。
    pub fn merge(mut self, other: Router) -> Router {
        self.routes.extend(other.routes);
        self
    }

    /// 分发（纯函数，可单测）。路径匹配但方法不对 → 405。
    pub fn dispatch(&self, req: &mut Request<'_>) -> Reply {
        let mut path_exists = false;
        for r in &self.routes {
            let Some(params) = r.pattern.matches(&req.path) else { continue };
            if r.method != req.method {
                path_exists = true;
                continue;
            }
            req.params = params;
            return match (r.handler)(req) {
                Ok(rep) => rep,
                Err(e) => e.into(),
            };
        }
        if req.method == Method::Options {
            return Reply { status: 204, content_type: "text/plain".into(), body: vec![], headers: vec![], stream: None };
        }
        if path_exists {
            Reply::error(405, "method not allowed")
        } else {
            Reply::not_found()
        }
    }
}

/// 解析查询串（百分号解码）。
pub fn parse_query(q: &str) -> HashMap<String, String> {
    q.split('&')
        .filter(|s| !s.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (crate::multipart::percent_decode(k), crate::multipart::percent_decode(v))
        })
        .collect()
}

/// 服务选项：TLS（PEM）与请求守卫（登录/密码策略由服务自己定义，HTTP 层只负责"先问守卫再分发"）。
#[derive(Default)]
pub struct ServeOpts {
    pub tls: Option<crate::tls::TlsPem>,
    pub guard: Option<Guard>,
}

/// 守卫：看到请求（方法/路径/头）后返回 `None`=放行，`Some(reply)`=拦下并直接回这个应答。
pub struct Guard {
    pub check: Arc<dyn Fn(&GuardRequest) -> Option<Reply> + Send + Sync>,
}

pub struct GuardRequest {
    pub method: Method,
    pub path: String,
    pub headers: Vec<(String, String)>,
}
impl GuardRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        header_of(&self.headers, name)
    }
}

const KEPT_HEADERS: &[&str] = &["Cookie", "Authorization", "Accept", "Host", "X-Forwarded-Proto", "User-Agent"];

fn reply_to_tiny(reply: Reply) -> tiny_http::Response<Box<dyn Read + Send>> {
    let mut headers = Vec::new();
    if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], reply.content_type.as_bytes()) {
        headers.push(h);
    }
    for (k, v) in reply.headers {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            headers.push(h);
        }
    }
    let len = reply.body.len();
    tiny_http::Response::new(tiny_http::StatusCode(reply.status), headers, Box::new(std::io::Cursor::new(reply.body)) as Box<dyn Read + Send>, Some(len), None)
}

/// 流式回执（SSE）：**不能**走 `respond`——tiny_http 的 chunked 编码器（chunked_transfer::Encoder）攒满 8 KB 才发、
/// 外面还套一层 1 KB BufWriter，小帧永远滞留（真机 curl 30 s 零字节）。改用 `Request::upgrade` 拿到裸 socket：
/// 先发一个只有头的 200（Content-Type: text/event-stream，无长度、`Connection: upgrade`——浏览器/curl 对 200 忽略它，
/// 按"读到连接关闭"处理），然后从 reader 读一帧写一帧、每帧 flush；客户端断开 → 写失败 → 退出，socket 随之关闭。
fn respond_stream(req: tiny_http::Request, status: u16, content_type: &str, extra: Vec<(String, String)>, mut reader: Box<dyn Read + Send>) {
    let mut resp = tiny_http::Response::empty(tiny_http::StatusCode(status));
    if let Ok(h) = tiny_http::Header::from_bytes(&b"Content-Type"[..], content_type.as_bytes()) {
        resp = resp.with_header(h);
    }
    for (k, v) in extra {
        if let Ok(h) = tiny_http::Header::from_bytes(k.as_bytes(), v.as_bytes()) {
            resp = resp.with_header(h);
        }
    }
    let mut sock = req.upgrade("sse", resp);
    let mut buf = [0u8; 4096];
    loop {
        let n = match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if sock.write_all(&buf[..n]).is_err() || sock.flush().is_err() {
            break;
        }
    }
}

/// 起阻塞服务器：每请求一线程（上传大文件不阻塞其它请求）。永不返回（bind 失败返回 Err）。
pub fn serve(bind: &str, router: Router) -> Result<(), String> {
    serve_with(bind, router, ServeOpts::default())
}

pub fn serve_with(bind: &str, router: Router, opts: ServeOpts) -> Result<(), String> {
    let server = match opts.tls {
        Some(pem) => tiny_http::Server::https(bind, tiny_http::SslConfig { certificate: pem.cert, private_key: pem.key }).map_err(|e| format!("绑定 {bind}（TLS）失败: {e}"))?,
        None => tiny_http::Server::http(bind).map_err(|e| format!("绑定 {bind} 失败: {e}"))?,
    };
    let router = Arc::new(router);
    let guard = opts.guard.map(Arc::new);
    for mut req in server.incoming_requests() {
        let router = router.clone();
        let guard = guard.clone();
        std::thread::spawn(move || {
            let url = req.url().to_string();
            let (path, query) = url.split_once('?').unwrap_or((&url, ""));
            let method = Method::from_tiny(req.method());
            let mut content_type = String::new();
            let mut content_length = None;
            let mut headers: Vec<(String, String)> = Vec::new();
            for h in req.headers() {
                if h.field.equiv("Content-Type") {
                    content_type = h.value.as_str().to_string();
                } else if h.field.equiv("Content-Length") {
                    content_length = h.value.as_str().parse().ok();
                } else if let Some(k) = KEPT_HEADERS.iter().find(|k| h.field.equiv(k)) {
                    headers.push((k.to_string(), h.value.as_str().to_string()));
                }
            }
            let path = path.to_string();
            if let Some(g) = &guard {
                if let Some(reply) = (g.check)(&GuardRequest { method, path: path.clone(), headers: headers.clone() }) {
                    let _ = req.respond(reply_to_tiny(reply));
                    return;
                }
            }
            let query = parse_query(query);
            let mut reply = {
                let mut body = req.as_reader();
                let mut r = Request { method, path, query, params: HashMap::new(), content_type, content_length, headers, body: &mut body };
                router.dispatch(&mut r)
            };
            if let Some(reader) = reply.stream.take() {
                respond_stream(req, reply.status, &reply.content_type, reply.headers, reader);
                return;
            }
            let _ = req.respond(reply_to_tiny(reply));
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(router: &Router, m: Method, path: &str, q: &str) -> (u16, String) {
        let mut empty: &[u8] = b"";
        let mut r = Request { method: m, path: path.into(), query: parse_query(q), params: HashMap::new(), content_type: String::new(), content_length: None, headers: vec![], body: &mut empty };
        let rep = router.dispatch(&mut r);
        (rep.status, String::from_utf8_lossy(&rep.body).to_string())
    }

    #[test]
    fn merge_keeps_front_routes_first() {
        let a = Router::new().get("/health", |_| Ok(Reply::ok(&serde_json::json!({"h": 1}))));
        let b = Router::new().get("/{name}", |r| Ok(Reply::ok(&serde_json::json!({"name": r.param("name")}))));
        let r = a.merge(b);
        assert_eq!(call(&r, Method::Get, "/health", "").1, r#"{"h":1}"#);
        assert_eq!(call(&r, Method::Get, "/x.png", "").1, r#"{"name":"x.png"}"#);
    }

    #[test]
    fn routes_params_prefix_and_errors() {
        let router = Router::new()
            .get("/api/fonts", |_| Ok(Reply::ok(&serde_json::json!({"n": 1}))))
            .delete("/api/fonts/{file}", |r| Ok(Reply::ok(&serde_json::json!({"file": r.param("file")}))))
            .get("/api/proxy/*", |r| Ok(Reply::ok(&serde_json::json!({"rest": r.param("*")}))))
            .post("/api/x", |r| Err(ApiError::bad(format!("q={}", r.q("t").unwrap_or("")))));
        assert_eq!(call(&router, Method::Get, "/api/fonts", "").0, 200);
        assert_eq!(call(&router, Method::Delete, "/api/fonts/%E5%AD%97.ttf", "").1, r#"{"file":"字.ttf"}"#);
        assert_eq!(call(&router, Method::Get, "/api/proxy/a/b/c", "").1, r#"{"rest":"a/b/c"}"#);
        assert_eq!(call(&router, Method::Post, "/api/x", "t=1%202"), (400, r#"{"message":"q=1 2","ok":false}"#.into()));
        assert_eq!(call(&router, Method::Post, "/api/fonts", "").0, 405);
        assert_eq!(call(&router, Method::Get, "/nope", "").0, 404);
        assert_eq!(call(&router, Method::Options, "/whatever", "").0, 204);
    }

    #[test]
    fn any_binds_one_handler_to_many_methods_and_bind_shares_state() {
        let st = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let router = Router::new().any(&[Method::Get, Method::Post], "/n", bind(&st, |st, _| {
            let n = st.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            Ok(Reply::ok(&serde_json::json!({"n": n})))
        }));
        assert_eq!(call(&router, Method::Get, "/n", "").1, r#"{"n":1}"#);
        assert_eq!(call(&router, Method::Post, "/n", "").1, r#"{"n":2}"#);
        assert_eq!(call(&router, Method::Put, "/n", "").0, 405);
    }

    #[test]
    fn json_body_accessors() {
        let b = JsonBody(serde_json::json!({"name": "x", "folder": "  ", "keep": false}));
        assert_eq!(b.str("name").unwrap(), "x");
        assert_eq!(b.str("nope").unwrap_err().message, "缺 nope");
        assert_eq!(b.str_or("folder", "lib"), "lib", "空白当缺省");
        assert!(!b.bool_or("keep", true) && b.bool_or("other", true));
    }
}
