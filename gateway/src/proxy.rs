//! 反向代理（Facade）：把 `/api/<seg>[/<rest>]` 转给注册表里的服务（剥掉 `<seg>`），状态码/JSON 原样回。
//! **请求方向流式**（`send(&mut *req.body)` 直接转发原始请求体读取器，上传大文件不额外占内存）。
//! **响应方向**：后端给了 `Content-Length` 的 200 应答，若是下载（带 `Content-Disposition`）或体积超过
//! [`STREAM_MIN_BYTES`]（壁纸原图等），走 `Reply::sized_stream` 按定长边读边发，不整个读进网关内存；
//! 其余（JSON 等小应答、没有长度的应答）读完再回——没有长度的流只能走 SSE 那条"读到连接关闭"的通道，
//! 不适合普通下载（见 [`stream_len`]）。
//!
//! **并发/内存预算闸门**（2026-09-19）：`优化`/`加入xochitl`/`加入KOReader` 这三个操作在这里统一
//! 拦一道——真机测出漫画 optimize/超限分卷投递内存峰值 ≈ 处理的文件体积本身，`book-serve`/
//! `koreader-serve` 的忙锁都是按书名分别加的、点不同的书互不阻塞，同时点几本大部头会线性叠加内存。
//! `gateway` 是这三个操作物理上唯一必经的转发关口（三个服务是独立进程，互不共享内存），闸门放这里
//! 不需要任何跨进程锁，详见 `budget.rs` 文档注释。只有这三条命中路由才会额外读一次 body（几十字节
//! 的小 JSON，`Request::read_small_body` 本来就有 1MB 上限）+ 查一次文件体积，其余请求（含真正的
//! 大文件上传）完全不受影响、维持原有纯流式转发。
//!
//! 第四条（2026-09-25）：**抓网文勾了「同步优化」**（`POST /api/books/staging/fetch-article`，`optimize:true`）
//! 也过闸门——它在一次 HTTP 请求里同步地"抓取→组 EPUB→落母版库→跑 `optimize()`"，此前既不占 book-serve 的忙锁
//! 也不占这里的名额。书名在请求时还不知道（要等抓完才有标题），闸门键用 `抓网文 <url>`，固定小档（网文通常几十 KB）；
//! 同步操作，响应回来＝真正做完，名额随函数返回释放（同 `KoreaderAdopt`）。没勾同步优化的抓取不过闸门。
use rmsvc_core::http::{ApiError, ApiResult, JsonBody, Method, Reply, Request};
use rmsvc_core::multipart::percent_encode as enc;
use rmsvc_core::paths::Paths;
use rmsvc_core::registry::{self, SvcClient};
use std::io::Read;
use std::time::{Duration, Instant};

/// 等"book-serve 有新事件"的兜底超时：正常靠 [`crate::events::books_wake`] 事件唤醒（忙态结束 book-serve 会发 `books`
/// 事件），这里只防事件丢了/订阅线程重连空窗——所以从原来的 5 秒轮询放宽到 30 秒（整个优化期间唤醒降到 1/6）。
const POLL_FALLBACK: Duration = Duration::from_secs(30);
/// 轮询等一个异步任务（优化/落库）真正跑完的上限——不是永久卡死，服务崩溃/重启导致侦测不到
/// 结果时，超时后如实放弃、让名额自然释放，不为一个查不到结果的任务永久占着并发档位。
const SETTLE_POLL_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum GatedOp {
    /// `book-serve` 的"优化"——异步：HTTP 响应几乎立即回"已开始"，真正处理在后台线程跑。
    Optimize,
    /// `book-serve` 的"加入 xochitl"——同上，异步。
    Deliver,
    /// `koreader-serve` 的"加入 KOReader"——同步：`fs::copy`+`fs::rename`，HTTP 响应返回=真正做完。
    KoreaderAdopt,
    /// `book-serve` 的"抓网文"——同步：抓取 + 组包 + （勾了才有的）同步优化都在这次请求里做完。
    FetchArticle,
}

/// 抓网文在闸门里的占位名前缀（后接 URL）。书名要抓完才知道，只能拿 URL 当键；带前缀不会跟母版库书名撞，
/// 网页的"排队/处理中"计数照样把它算进去。
const ARTICLE_GATE_PREFIX: &str = "抓网文 ";

/// 这次请求在闸门里要占的名额：键（书名或 `抓网文 <url>`）+ 档位。`Ok(None)`＝这次不用过闸门（抓网文没勾同步优化）。
/// 体积由 `size_of` 注入，便于离线测。
fn gate_target(kind: GatedOp, body: &JsonBody, size_of: impl Fn(&str) -> u64) -> Result<Option<(String, crate::budget::Tier)>, ApiError> {
    if kind == GatedOp::FetchArticle {
        // 与 book-serve 同口径：`optimize` 缺省 false（`bool_or("optimize", false)`）。
        if !body.bool_or("optimize", false) {
            return Ok(None);
        }
        return Ok(Some((format!("{ARTICLE_GATE_PREFIX}{}", body.str("url")?), crate::budget::Tier::Small)));
    }
    let name = body.str("name")?.to_string();
    let tier = crate::budget::tier_of(size_of(&name));
    Ok(Some((name, tier)))
}

/// 这个请求是不是命中要限流的三个操作之一。`service_name` 是解析过的后端服务名
/// （`book-serve`/`koreader-serve`，不是 URL 段 `books`/`koreader`）。
fn gated_operation(service_name: &str, rest: &str, method: Method) -> Option<GatedOp> {
    if method != Method::Post {
        return None;
    }
    match (service_name, rest) {
        ("book-serve", "staging/optimize") => Some(GatedOp::Optimize),
        ("book-serve", "staging/deliver") => Some(GatedOp::Deliver),
        ("koreader-serve", "books/adopt") => Some(GatedOp::KoreaderAdopt),
        ("book-serve", "staging/fetch-article") => Some(GatedOp::FetchArticle),
        _ => None,
    }
}

/// 不带 `Content-Disposition` 的应答超过这个体积也流式转发（壁纸原图、裁图等图片；JSON 列表远小于它）。
const STREAM_MIN_BYTES: u64 = 256 * 1024;

/// 这条后端应答该不该流式转发；该 → 返回定长。**只流式转发有 `Content-Length` 的 200**：定长流由 tiny_http
/// 按长度发完即结束；没有长度的流只能走 `Reply::stream`（SSE 用的"升级成裸 socket、读到连接关闭"），拿来做
/// 普通下载会让客户端等不到结束（2026-09-24 真机下载卡住就是这一类），所以宁可读完再回。
fn stream_len(status: u16, download: bool, len: Option<u64>) -> Option<u64> {
    let n = len?;
    (status == 200 && (download || n > STREAM_MIN_BYTES)).then_some(n)
}

/// `/api/{svc}/*` → 按 URL 段查目录表找服务名再转发（段不在表里 404）。
pub fn forward(paths: &Paths, req: &mut Request<'_>) -> ApiResult {
    let Some(name) = crate::manage::service_of(req.param("svc")) else { return Err(ApiError::not_found("未知服务")) };
    let Some(info) = registry::find(paths, name) else {
        return Err(ApiError { status: 404, message: format!("{name} 未安装或未运行") });
    };
    // 剥掉服务段：`/api/fonts/x` → 后端 `/x`，`/api/fonts` → 后端 `/`。后端直连（SSH 调试）与经网关同一套路由。
    let rest = req.param("*").to_string();
    let gated = gated_operation(name, &rest, req.method);

    // 命中限流操作才读 body 拿书名、过闸门；其余请求原样走下面已有的流式转发，不碰这段。
    let mut body_override: Option<Vec<u8>> = None;
    let mut slot: Option<crate::budget::Slot<'static>> = None;
    let mut book_name = String::new();
    if let Some(kind) = gated {
        // 读一次 body 拿书名、过闸门后还要原样转发给后端，所以先读成字节再解析（`req.json()` 会把流读空）。
        let buf = req.read_small_body().map_err(ApiError::bad)?;
        let parsed: serde_json::Value = serde_json::from_slice(&buf).map_err(|e| ApiError::bad(format!("请求不是 JSON: {e}")))?;
        let staging = paths.staging_dir();
        if let Some((key, tier)) = gate_target(kind, &JsonBody(parsed), |n| staging.join(n).metadata().map(|m| m.len()).unwrap_or(0))? {
            slot = Some(crate::budget::global().admit(tier, &key).map_err(|e| ApiError { status: e.status(), message: e.message() })?);
            book_name = key;
        }
        body_override = Some(buf);
    }

    let mut url = format!("{}/{}", info.base_url(), rest);
    if !req.query.is_empty() {
        let q: Vec<String> = req.query.iter().map(|(k, v)| format!("{}={}", enc(k), enc(v))).collect();
        url.push('?');
        url.push_str(&q.join("&"));
    }
    let method = match req.method {
        Method::Get => "GET",
        Method::Post => "POST",
        Method::Put => "PUT",
        Method::Delete => "DELETE",
        _ => return Err(ApiError::bad("unsupported method")),
    };
    let agent = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(900)).build();
    let mut r = agent.request(method, &url);
    if !req.content_type.is_empty() {
        r = r.set("Content-Type", &req.content_type);
    }
    // 只在真的转发请求体时带长度：GET/DELETE 走 `call()` 不发 body，若照抄客户端的 Content-Length（带 body 的 DELETE），
    // 后端会一直等那几个永远不来的字节直到超时；闸门那条路重发的是读出来的字节，长度以它为准。
    let has_body = !matches!(req.method, Method::Get | Method::Delete);
    if let Some(n) = body_override.as_ref().map(Vec::len).or(req.content_length).filter(|_| has_body) {
        r = r.set("Content-Length", &n.to_string());
    }
    let resp = if !has_body {
        r.call()
    } else if let Some(buf) = body_override {
        r.send(&mut std::io::Cursor::new(buf))
    } else {
        r.send(&mut *req.body)
    };
    let (status, resp) = match resp {
        Ok(r) => (r.status(), r),
        Err(ureq::Error::Status(c, r)) => (c, r),
        Err(e) => return Err(ApiError { status: 502, message: format!("{name} 无响应: {e}") }),
    };
    let ctype = resp.header("Content-Type").unwrap_or("application/octet-stream").to_string();
    // 只转发这一个头：后端服务想让浏览器"下载保存"而不是原地展示/跳转时设它（如 md/zip 导出、CA 证书下载，
    // 见 gateway::main 的证书下载同款用法）；别的头一律不转发，不给后端服务借这条通道夹带别的东西。
    let disposition = resp.header("Content-Disposition").map(str::to_string);
    // 下载（母版库原件可达上百 MB）与大应答：按定长边读边发，不整个读进网关内存；其余照旧读完再回。
    let len = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok());
    let mut reply = if let Some(n) = stream_len(status, disposition.is_some(), len) {
        Reply::sized_stream(&ctype, Box::new(resp.into_reader()), n).with_status(status)
    } else {
        let mut body = Vec::new();
        resp.into_reader().read_to_end(&mut body).map_err(|e| ApiError::internal(e.to_string()))?;
        Reply { status, content_type: ctype, body, headers: vec![], stream: None }
    };
    if let Some(v) = disposition {
        reply = reply.with_header("Content-Disposition", &v);
    }

    // 名额释放时机：`KoreaderAdopt`/`FetchArticle` 是同步操作，走到这里真正的复制/优化已经做完，`slot` 出函数作用域
    // 自然 Drop 释放，不用特殊处理。`Optimize`/`Deliver` 是异步的，HTTP 响应此刻只代表"已经开始"，
    // 真正的内存开销在后台线程里继续——把 `slot` 转移进一个监控线程，轮询该服务自己的 `/staging`
    // 列表直到这本书不再 busy（或条目已经不在了，比如漫画→PDF 改名），`slot` 才出那个线程的作用域
    // 释放；轮询/线程本身跟这次 HTTP 响应完全解耦，不影响这次请求的返回时间。
    if let Some(kind) = gated {
        if matches!(kind, GatedOp::Optimize | GatedOp::Deliver) {
            if let Some(slot) = slot.take() {
                let client = SvcClient::new(paths.clone(), "book-serve", 10);
                std::thread::spawn(move || {
                    poll_until_settled(&client, &book_name);
                    drop(slot);
                });
            }
        }
    }

    Ok(reply)
}

/// 查 book-serve 的 `/staging`（直连后端服务，不经网关自己这层转发，避免自己调自己）直到
/// [`crate::budget::is_settled`] 判定这本书已经不再忙，或等到 [`SETTLE_POLL_TIMEOUT`] 放弃。
/// **事件驱动**：每次查完就阻塞等 [`crate::events::books_wake`]（book-serve 有事件才醒），至多 [`POLL_FALLBACK`] 兜底一次；
/// 先取代数再查，查询期间到达的事件不会漏。
/// 查询失败要**连续** [`MAX_POLL_FAILURES`] 次才放弃（每次隔 [`FAILURE_RETRY`]）：此前一次失败就放名额，
/// 而大书优化时 book-serve 正忙、10 秒查询超时恰恰最容易撞上，于是第二本大书被放进来、内存照样叠加——
/// 闸门在最该起作用的时候失效（2026-09-24 审查）。连续失败才说明服务真的挂了/重启了（任务随之没了），
/// 这时再放名额，不让侦测本身不可靠把并发档位永久卡住。
pub(crate) fn poll_until_settled(client: &SvcClient, name: &str) {
    let wake = crate::events::books_wake();
    wait_settled(|| client.get_json("/staging"), name, Instant::now() + SETTLE_POLL_TIMEOUT, || wake.generation(), |seen, d| { wake.wait_change(seen, d); });
}

/// 连续几次查询失败才认定"服务已不可达、任务没了"。
const MAX_POLL_FAILURES: u32 = 6;
/// 查询失败后至多隔多久重试（服务卡住时事件多半不来，不能只等事件）。
const FAILURE_RETRY: Duration = Duration::from_secs(5);

/// [`poll_until_settled`] 的循环本体，查询/代数/等待都由调用方注入，便于离线测试。
fn wait_settled(
    mut query: impl FnMut() -> Result<serde_json::Value, String>,
    name: &str,
    deadline: Instant,
    generation: impl Fn() -> u64,
    wait: impl Fn(u64, Duration),
) {
    let mut failures = 0u32;
    loop {
        let seen = generation();
        let failed = match query() {
            Ok(json) if crate::budget::is_settled(&json, name) => return,
            Ok(_) => false, // 还在忙，继续轮询
            Err(_) => true,
        };
        failures = if failed { failures + 1 } else { 0 };
        if failures >= MAX_POLL_FAILURES || Instant::now() >= deadline {
            return;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if failed {
            wait(generation(), FAILURE_RETRY.min(left)); // 等下一次事件，至多 FAILURE_RETRY
        } else {
            wait(seen, POLL_FALLBACK.min(left));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gated_operation_matches_exactly_three_routes() {
        assert_eq!(gated_operation("book-serve", "staging/optimize", Method::Post), Some(GatedOp::Optimize));
        assert_eq!(gated_operation("book-serve", "staging/deliver", Method::Post), Some(GatedOp::Deliver));
        assert_eq!(gated_operation("koreader-serve", "books/adopt", Method::Post), Some(GatedOp::KoreaderAdopt));
        assert_eq!(gated_operation("book-serve", "staging/fetch-article", Method::Post), Some(GatedOp::FetchArticle));
    }

    /// 抓网文：勾了同步优化才占名额（小档、键带前缀不跟书名撞），没勾不过闸门；普通优化仍按书名 + 体积分档。
    #[test]
    fn gate_target_for_fetch_article_and_staged_books() {
        use crate::budget::Tier;
        let j = JsonBody;
        let big = |_: &str| crate::budget::LARGE_THRESHOLD_BYTES + 1;
        let got = gate_target(GatedOp::FetchArticle, &j(serde_json::json!({"url": "https://a.b/c", "optimize": true})), big).unwrap();
        assert_eq!(got, Some(("抓网文 https://a.b/c".to_string(), Tier::Small)), "网文固定小档，不查母版库体积");
        assert_eq!(gate_target(GatedOp::FetchArticle, &j(serde_json::json!({"url": "https://a.b/c", "optimize": false})), big).unwrap(), None);
        assert_eq!(gate_target(GatedOp::FetchArticle, &j(serde_json::json!({"url": "https://a.b/c"})), big).unwrap(), None, "缺省不优化＝不过闸门");
        assert!(gate_target(GatedOp::FetchArticle, &j(serde_json::json!({"optimize": true})), big).is_err(), "缺 url 直接 400");
        assert_eq!(gate_target(GatedOp::Optimize, &j(serde_json::json!({"name": "x.epub"})), big).unwrap(), Some(("x.epub".to_string(), Tier::Large)));
    }

    /// 回归（2026-09-25）：抓网文同步优化经 [`forward`] 真的占一个名额、响应回来就还——起一个假 book-serve，
    /// 在它处理请求的那一刻查闸门快照，确认名额在用；请求返回后快照里没有它。
    #[test]
    fn fetch_article_with_optimize_holds_and_returns_a_slot() {
        use std::io::Write;
        use std::sync::atomic::{AtomicBool, Ordering};
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_string_lossy().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k.starts_with("XDG_") { Some(h.clone()) } else { None });
        let url = "https://example.invalid/slot-test";
        let key = format!("{ARTICLE_GATE_PREFIX}{url}");
        let seen_active = std::sync::Arc::new(AtomicBool::new(false));
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let (seen2, key2) = (seen_active.clone(), key.clone());
        let backend = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            // 读到请求头结束 + 按 Content-Length 读完 body（小 JSON），再回一个最小 200。
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            loop {
                let n = sock.read(&mut chunk).unwrap();
                buf.extend_from_slice(&chunk[..n]);
                if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..end]).to_ascii_lowercase();
                    let len: usize = head.lines().find_map(|l| l.strip_prefix("content-length:").map(|v| v.trim().parse().unwrap())).unwrap_or(0);
                    if buf.len() >= end + 4 + len || n == 0 {
                        break;
                    }
                }
            }
            seen2.store(crate::budget::global().snapshot().1.contains(&key2), Ordering::SeqCst);
            let body = b"{\"ok\":true}";
            write!(sock, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            sock.write_all(body).unwrap();
        });
        let info = registry::ServiceInfo { name: "book-serve".into(), port, label: String::new(), version: String::new(), pid: std::process::id(), ui: None };
        let _reg = registry::register(&paths, &info).unwrap();
        let body = serde_json::json!({"url": url, "optimize": true}).to_string().into_bytes();
        let mut rd = std::io::Cursor::new(body.clone());
        let params = [("svc", "books"), ("*", "staging/fetch-article")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let mut req = Request {
            method: Method::Post,
            path: "/api/books/staging/fetch-article".into(),
            query: Default::default(),
            params,
            content_type: "application/json".into(),
            content_length: Some(body.len()),
            headers: vec![],
            body: &mut rd,
        };
        let reply = forward(&paths, &mut req).unwrap();
        backend.join().unwrap();
        assert_eq!(reply.status, 200);
        assert!(seen_active.load(Ordering::SeqCst), "后端处理期间闸门里应有这次抓网文的名额");
        assert!(!crate::budget::global().snapshot().1.contains(&key), "同步请求返回后名额应已归还");
    }

    /// 回归：客户端发了带 `Content-Length` 的 DELETE，网关用 `call()` 不转发 body，也就不能转发这个长度——否则后端会
    /// 干等那几个字节。假后端只读请求头，记下有没有 `content-length`，然后立刻回 200。
    #[test]
    fn delete_forwards_no_content_length() {
        use std::io::Write;
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_string_lossy().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k.starts_with("XDG_") { Some(h.clone()) } else { None });
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let backend = std::thread::spawn(move || {
            let (mut sock, _) = listener.accept().unwrap();
            let mut buf = Vec::new();
            let mut chunk = [0u8; 1024];
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut chunk).unwrap();
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let body = b"{\"ok\":true}";
            write!(sock, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).unwrap();
            sock.write_all(body).unwrap();
            String::from_utf8_lossy(&buf).to_ascii_lowercase()
        });
        let info = registry::ServiceInfo { name: "font-serve".into(), port, label: String::new(), version: String::new(), pid: std::process::id(), ui: None };
        let _reg = registry::register(&paths, &info).unwrap();
        let mut rd: &[u8] = b"12345";
        let params = [("svc", "fonts"), ("*", "x.ttf")].iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let mut req = Request { method: Method::Delete, path: "/api/fonts/x.ttf".into(), query: Default::default(), params, content_type: String::new(), content_length: Some(5), headers: vec![], body: &mut rd };
        let reply = forward(&paths, &mut req).unwrap();
        let head = backend.join().unwrap();
        assert_eq!(reply.status, 200);
        assert!(head.starts_with("delete /x.ttf"), "{head}");
        assert!(!head.contains("content-length"), "DELETE 不转发 body，也不能带长度：{head}");
    }

    #[test]
    fn gated_operation_ignores_everything_else() {
        assert_eq!(gated_operation("book-serve", "staging", Method::Post), None, "落库入库本身走多文件上传，不该被拦下来读 body");
        assert_eq!(gated_operation("book-serve", "staging", Method::Get), None, "列表查询不限流");
        assert_eq!(gated_operation("book-serve", "staging/optimize", Method::Get), None, "方法不对不该命中");
        assert_eq!(gated_operation("koreader-serve", "books", Method::Get), None);
        assert_eq!(gated_operation("font-serve", "staging/optimize", Method::Post), None, "服务名对不上不该误命中");
    }

    #[test]
    fn streams_only_sized_downloads_or_big_bodies() {
        assert_eq!(stream_len(200, true, Some(10)), Some(10), "带长度的下载：流式");
        assert_eq!(stream_len(200, false, Some(STREAM_MIN_BYTES + 1)), Some(STREAM_MIN_BYTES + 1), "大图片：流式");
        assert_eq!(stream_len(200, false, Some(1000)), None, "小 JSON：读完再回");
        assert_eq!(stream_len(200, true, None), None, "没有长度的下载不能走读到关闭的流");
        assert_eq!(stream_len(404, true, Some(10)), None, "错误应答读完再回");
    }

    /// 回归：一次查询失败（大书优化时 book-serve 忙、查询超时）不能提前放名额，要等它真的不忙。
    #[test]
    fn transient_query_failure_does_not_release_slot_early() {
        use std::cell::Cell;
        let busy = serde_json::json!({"items": [{"name": "big.epub", "busy": true}]});
        let idle = serde_json::json!({"items": [{"name": "big.epub", "busy": false}]});
        let script: Vec<Result<serde_json::Value, String>> = vec![Ok(busy.clone()), Err("timeout".into()), Err("timeout".into()), Ok(busy), Err("timeout".into()), Ok(idle)];
        let calls = Cell::new(0usize);
        wait_settled(|| { let i = calls.get(); calls.set(i + 1); script[i].clone() }, "big.epub", Instant::now() + Duration::from_secs(3600), || 0, |_, _| {});
        assert_eq!(calls.get(), 6, "应一直等到真正不忙（第 6 次查询）才返回");
    }

    /// 连续失败到上限 → 放弃（服务真挂了，任务已随之消失）。
    #[test]
    fn persistent_failure_gives_up_after_limit() {
        use std::cell::Cell;
        let calls = Cell::new(0u32);
        wait_settled(|| { calls.set(calls.get() + 1); Err("down".into()) }, "x", Instant::now() + Duration::from_secs(3600), || 0, |_, _| {});
        assert_eq!(calls.get(), MAX_POLL_FAILURES);
    }
}
