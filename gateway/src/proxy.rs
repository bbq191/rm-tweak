//! 反向代理（Facade）：把 `/api/<seg>[/<rest>]` 转给注册表里的服务（剥掉 `<seg>`），状态码/JSON 原样回。
//! **只有请求方向真的流式**（`send(&mut *req.body)` 直接转发原始请求体读取器，上传大文件不额外占内存）；
//! **响应方向整体缓冲进内存**（`into_reader().read_to_end(...)`，2026-09-09 审计发现文档写的"body 流式
//! 透传"跟实现不符，这里改成如实描述）——`rmsvc_core::http::Reply::stream` 现有的流式响应通道是给 SSE
//! 用的，底层走 `tiny_http` 的 `upgrade()` 直接接管裸 socket（不走常规的 Content-Length/chunked 头协商），
//! 拿来复用给任意大小的代理下载响应需要先确认这套机制对非 SSE 场景是否语义正确，评估下来风险和这条
//! 低优先级审计项本身的收益不成比例，这次只改注释，没有改行为。
use rmsvc_core::http::{ApiError, ApiResult, Method, Reply, Request};
use rmsvc_core::multipart::percent_encode as enc;
use rmsvc_core::paths::Paths;
use rmsvc_core::registry;
use std::io::Read;

/// `/api/{svc}/*` → 按 URL 段查目录表找服务名再转发（段不在表里 404）。
pub fn forward(paths: &Paths, req: &mut Request<'_>) -> ApiResult {
    let Some(name) = crate::manage::service_of(req.param("svc")) else { return Err(ApiError::not_found("未知服务")) };
    let Some(info) = registry::find(paths, name) else {
        return Err(ApiError { status: 404, message: format!("{name} 未安装或未运行") });
    };
    // 剥掉服务段：`/api/fonts/x` → 后端 `/x`，`/api/fonts` → 后端 `/`。后端直连（SSH 调试）与经网关同一套路由。
    let rest = req.param("*").to_string();
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
    if let Some(n) = req.content_length {
        r = r.set("Content-Length", &n.to_string());
    }
    let resp = if matches!(req.method, Method::Get | Method::Delete) { r.call() } else { r.send(&mut *req.body) };
    let (status, resp) = match resp {
        Ok(r) => (r.status(), r),
        Err(ureq::Error::Status(c, r)) => (c, r),
        Err(e) => return Err(ApiError { status: 502, message: format!("{name} 无响应: {e}") }),
    };
    let ctype = resp.header("Content-Type").unwrap_or("application/octet-stream").to_string();
    // 只转发这一个头：后端服务想让浏览器"下载保存"而不是原地展示/跳转时设它（如 md/zip 导出、CA 证书下载，
    // 见 gateway::main 的证书下载同款用法）；别的头一律不转发，不给后端服务借这条通道夹带别的东西。
    let disposition = resp.header("Content-Disposition").map(str::to_string);
    let mut body = Vec::new();
    resp.into_reader().read_to_end(&mut body).map_err(|e| ApiError::internal(e.to_string()))?;
    let mut reply = Reply { status, content_type: ctype, body, headers: vec![], stream: None };
    if let Some(v) = disposition {
        reply = reply.with_header("Content-Disposition", &v);
    }
    Ok(reply)
}
