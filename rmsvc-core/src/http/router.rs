//! 路由：路径模式（尾 `/*` 前缀匹配、单段 `{param}`）+ 最具体优先的分发 + 查询串解析。纯数据结构，不碰 socket。
use super::{ApiResult, Handler, Method, Reply, Request};
use std::collections::HashMap;
use std::sync::Arc;

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
    /// 具体程度：(字面段个数, 是否精确匹配)。值大者更具体。
    fn specificity(&self) -> (usize, bool) {
        (self.segs.iter().filter(|s| !(s.starts_with('{') && s.ends_with('}'))).count(), !self.prefix)
    }

    fn matches(&self, segs: &[&str]) -> Option<HashMap<String, String>> {
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
                params.insert(pat[1..pat.len() - 1].to_string(), crate::multipart::percent_decode_path(seg));
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

/// 分发时的候选：(具体程度, 路由, 解出的路径参数)。
type Candidate<'a> = ((usize, bool), &'a Arc<Route>, HashMap<String, String>);

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
    /// **最具体的路由优先**（字面段个数多者胜，同数时精确匹配胜过尾部 `/*`，仍相同才按注册先后）：
    /// 此前是"注册顺序第一个匹配者胜"，`GET /{name}` 这类通配路由只要注册在字面路由前面就会把
    /// `/events`、`/health` 抢走（真机 wallpaper-serve 踩过，靠"必须先注册字面路由"的口头纪律避免）。
    pub fn dispatch(&self, req: &mut Request<'_>) -> Reply {
        let mut path_exists = false;
        let mut best: Option<Candidate<'_>> = None;
        // 路径只切一次：此前每条路由各自切分+分配一遍（网关几十条路由、每个请求都过一轮）。
        let segs: Vec<&str> = req.path.trim_matches('/').split('/').filter(|s| !s.is_empty()).collect();
        for r in &self.routes {
            let Some(params) = r.pattern.matches(&segs) else { continue };
            if r.method != req.method {
                path_exists = true;
                continue;
            }
            let score = r.pattern.specificity();
            if best.as_ref().map(|(b, _, _)| score > *b).unwrap_or(true) {
                best = Some((score, r, params));
            }
        }
        if let Some((_, r, params)) = best {
            req.params = params;
            // `?ka=<秒>`：让本请求里创建的 SSE 流用指定心跳（见 `events::parse_keepalive_param`）。
            let _ka = crate::events::enter_request(crate::events::parse_keepalive_param(req.q("ka")));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::{bind, ApiError};

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

    /// 回归：字面路由不因注册在通配路由之后而被抢走（原来靠注册顺序纪律）。

    #[test]
    fn literal_route_beats_param_route_regardless_of_registration_order() {
        let router = Router::new()
            .get("/{name}", |r| Ok(Reply::ok(&serde_json::json!({"wild": r.param("name")}))))
            .get("/events", |_| Ok(Reply::ok(&serde_json::json!({"lit": "events"}))))
            .get("/books/{id}", |r| Ok(Reply::ok(&serde_json::json!({"id": r.param("id")}))))
            .get("/books/adopt", |_| Ok(Reply::ok(&serde_json::json!({"lit": "adopt"}))))
            .get("/api/*", |_| Ok(Reply::ok(&serde_json::json!({"prefix": true}))))
            .get("/api/{x}", |r| Ok(Reply::ok(&serde_json::json!({"exact": r.param("x")}))));
        assert_eq!(call(&router, Method::Get, "/events", "").1, r#"{"lit":"events"}"#);
        assert_eq!(call(&router, Method::Get, "/other", "").1, r#"{"wild":"other"}"#);
        assert_eq!(call(&router, Method::Get, "/books/adopt", "").1, r#"{"lit":"adopt"}"#);
        assert_eq!(call(&router, Method::Get, "/books/7", "").1, r#"{"id":"7"}"#);
        assert_eq!(call(&router, Method::Get, "/api/v", "").1, r#"{"exact":"v"}"#, "同字面段数时精确匹配胜过尾部通配");
        assert_eq!(call(&router, Method::Get, "/api/v/w", "").1, r#"{"prefix":true}"#);
        assert_eq!(call(&router, Method::Post, "/events", "").0, 405);
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
        assert_eq!(call(&router, Method::Delete, "/api/fonts/C++.ttf", "").1, r#"{"file":"C++.ttf"}"#, "路径参数里的 + 不当空格");
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
}
