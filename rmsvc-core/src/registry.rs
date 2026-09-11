//! 服务注册/发现（Registry）。每个领域服务启动时写 `<services_dir>/<name>.json`，退出时删除；
//! 网关/CLI 读目录即知"活着的服务"，缺哪个就隐藏该 tab / 回 404——拔插零配置。
//! 运行时目录重启即清，加上按 `/proc/<pid>` 清理陈旧条目，崩溃残留也不会误报。
use crate::paths::Paths;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 网关 UI 的一个 tab。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct UiTab {
    pub title: String,
    pub order: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ServiceInfo {
    pub name: String,
    pub port: u16,
    pub label: String,
    pub version: String,
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<UiTab>,
}

impl ServiceInfo {
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
}

/// 注册守卫：Drop 时删注册文件。
pub struct Registration {
    file: PathBuf,
}

impl Drop for Registration {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
    }
}

fn file_of(paths: &Paths, name: &str) -> PathBuf {
    paths.services_dir().join(format!("{name}.json"))
}

/// 写注册文件（原子：先写 .tmp 再 rename）。同名且 pid 仍活着的条目**拒绝覆盖**（防误起第二实例顶掉
/// 正在服务的那份——真机踩过：调试起了个 `--bind 127.0.0.1:1` 把 wallpaper-serve 注册顶没了）。
pub fn register(paths: &Paths, info: &ServiceInfo) -> std::io::Result<Registration> {
    std::fs::create_dir_all(paths.services_dir())?;
    let file = file_of(paths, &info.name);
    if let Some(existing) = std::fs::read_to_string(&file).ok().and_then(|t| serde_json::from_str::<ServiceInfo>(&t).ok()) {
        if existing.pid != info.pid && pid_alive(existing.pid) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} 已由 pid {} 注册（端口 {}）；先停掉它再起第二份", info.name, existing.pid, existing.port),
            ));
        }
    }
    crate::fs::write_atomic(&file, &serde_json::to_vec_pretty(info)?)?;
    Ok(Registration { file })
}

fn pid_alive(pid: u32) -> bool {
    if cfg!(target_os = "linux") {
        std::path::Path::new(&format!("/proc/{pid}")).exists()
    } else {
        true
    }
}

/// 列出活着的服务（按 ui.order 排序，无 tab 的在后）；陈旧条目（pid 已死）顺手删除。
pub fn list(paths: &Paths) -> Vec<ServiceInfo> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(paths.services_dir()) else { return out };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        let Ok(info) = serde_json::from_str::<ServiceInfo>(&text) else { continue };
        if !pid_alive(info.pid) {
            let _ = std::fs::remove_file(&p);
            continue;
        }
        out.push(info);
    }
    out.sort_by_key(|s| (s.ui.as_ref().map(|t| t.order).unwrap_or(u32::MAX), s.name.clone()));
    out
}

/// 按名找一个活着的服务。**O(1)**：直接读 `<name>.json`（注册文件名就是服务名，见 [`register`]），
/// 不再 `list` 全目录解析全部条目再过滤——网关每个代理请求、8 条事件订阅线程的重试都走这里。
/// 陈旧条目（pid 已死）同 `list` 一样顺手删除；名字不是单段文件名（含 `/`、`..`）一律当没有。
pub fn find(paths: &Paths, name: &str) -> Option<ServiceInfo> {
    crate::fs::plain_name(name).ok()?;
    let p = file_of(paths, name);
    let info = serde_json::from_str::<ServiceInfo>(&std::fs::read_to_string(&p).ok()?).ok()?;
    if !pid_alive(info.pid) {
        let _ = std::fs::remove_file(&p);
        return None;
    }
    (info.name == name).then_some(info)
}

/// 按注册表地址访问**另一个**服务的最小 HTTP 客户端骨架：查地址 → 带标准超时的 `ureq::Agent` →
/// GET/POST JSON。笔记线 mind/note/transcribe-serve 访问 ink-serve、note-serve 访问 book-serve 的
/// 回收站队列，此前各自把这层传输样板逐字节重写了一遍（2026-09-09 审计发现）——这里只抽这一层，
/// 各服务自己的 `EntryStore`/`TrashSink` 这类业务 trait 定义、方法签名、错误文案都不变，那是各自的
/// 关注点，合并了反而会把不同服务的语义耦合在一起。
pub struct SvcClient {
    paths: Paths,
    agent: ureq::Agent,
    service: &'static str,
}

impl SvcClient {
    pub fn new(paths: Paths, service: &'static str, timeout_secs: u64) -> SvcClient {
        SvcClient {
            paths,
            agent: ureq::AgentBuilder::new().timeout_connect(std::time::Duration::from_secs(3)).timeout(std::time::Duration::from_secs(timeout_secs)).build(),
            service,
        }
    }
    /// 目标服务当前的 base url；未运行时的错误文案统一成"<服务名> 未运行"。
    pub fn base(&self) -> Result<String, String> {
        find(&self.paths, self.service).map(|i| i.base_url()).ok_or_else(|| format!("{} 未运行", self.service))
    }
    /// 逃生舱：需要非 JSON 响应体（比如下载二进制裁图）时，调用方自己拼 URL、走这个 agent。
    pub fn agent(&self) -> &ureq::Agent {
        &self.agent
    }
    /// 服务返回非 2xx 时**读出对方的错误体**（`{"ok":false,"message":"..."}` 或 `{"error":"..."}`）：
    /// 此前只留 ureq 的 "status code 400"，对方写好的人话原因（"没有这本书的条目"之类）全丢了，
    /// 上层界面只能显示一句莫名其妙的状态码。其它传输错误（连不上/超时）照旧。
    fn to_svc_error(e: ureq::Error) -> SvcError {
        match e {
            ureq::Error::Status(code, resp) => {
                let msg = resp
                    .into_string()
                    .ok()
                    .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                    .and_then(|v| v.get("message").or_else(|| v.get("error")).and_then(|m| m.as_str()).map(str::to_string))
                    .filter(|m| !m.is_empty());
                SvcError { status: Some(code), message: msg.unwrap_or_else(|| format!("HTTP {code}")) }
            }
            e => SvcError { status: None, message: e.to_string() },
        }
    }
    fn prefixed(&self, verb: &str, path: &str, e: SvcError) -> String {
        format!("{} {verb} {path}: {}", self.service, e.message)
    }
    pub fn get_json(&self, path: &str) -> Result<serde_json::Value, String> {
        let service = self.service;
        let resp = self.agent.get(&format!("{}{path}", self.base()?)).call().map_err(|e| self.prefixed("GET", path, Self::to_svc_error(e)))?;
        serde_json::from_reader(resp.into_reader()).map_err(|e| format!("{service} {path} 应答不是 JSON: {e}"))
    }
    /// 响应体本身通常不需要（调用方要么忽略、要么自己按需再解析），失败时把状态错误带上。
    pub fn post_json(&self, path: &str, body: &serde_json::Value) -> Result<(), String> {
        self.post_json_value(path, body).map(|_| ())
    }
    /// POST JSON 并解析应答 JSON（应答体为空/不是 JSON 时给 `Null`，不当错误）；非 2xx 带对方错误体（错误文案带
    /// `<服务> POST <路径>:` 前缀，适合日志/开发者；要给用户看干净原因用 [`SvcClient::try_post_json`]）。
    pub fn post_json_value(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.try_post_json(path, body).map_err(|e| self.prefixed("POST", path, e))
    }
    /// 同 [`SvcClient::post_json_value`] 但返回结构化错误：`status` 是对方的 HTTP 状态码（传输层失败为 `None`），
    /// `message` 是对方错误体里的原因（没有则 `HTTP <码>`）或传输错误文本，不带任何前缀。
    pub fn try_post_json(&self, path: &str, body: &serde_json::Value) -> Result<serde_json::Value, SvcError> {
        let base = self.base().map_err(|m| SvcError { status: None, message: m })?;
        let resp = self.agent.post(&format!("{base}{path}")).set("Content-Type", "application/json").send_string(&body.to_string()).map_err(Self::to_svc_error)?;
        Ok(resp.into_string().ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(serde_json::Value::Null))
    }
}

/// 调另一个服务失败的结构化原因。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SvcError {
    /// 对方返回的 HTTP 状态码；连不上/超时等传输层错误为 `None`。
    pub status: Option<u16>,
    /// 对方错误体里的 `message`/`error`，没有则 `HTTP <码>`；传输错误为其文本。
    pub message: String,
}

/// URL 路径段编码（uuid/文件名这类需要转义的片段）。
pub fn enc(s: &str) -> String {
    crate::multipart::percent_encode(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(t: &tempfile::TempDir) -> Paths {
        let h = t.path().to_str().unwrap().to_string();
        Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None })
    }
    fn info(name: &str, order: u32, pid: u32) -> ServiceInfo {
        ServiceInfo {
            name: name.into(),
            port: 8790,
            label: name.into(),
            version: "0".into(),
            pid,
            ui: Some(UiTab { title: name.into(), order }),
        }
    }

    #[test]
    fn register_list_and_drop() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(&t);
        let me = std::process::id();
        let _a = register(&p, &info("b-svc", 2, me)).unwrap();
        let g = register(&p, &info("a-svc", 1, me)).unwrap();
        let names: Vec<String> = list(&p).into_iter().map(|s| s.name).collect();
        assert_eq!(names, vec!["a-svc", "b-svc"]);
        drop(g);
        assert_eq!(list(&p).len(), 1);
        assert!(find(&p, "b-svc").is_some());
        assert!(find(&p, "a-svc").is_none());
    }

    #[test]
    fn svc_client_base_url_uses_registry_and_errors_with_service_name_when_not_running() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(&t);
        let me = std::process::id();
        let _g = register(&p, &info("ink-serve", 1, me)).unwrap();

        let c = SvcClient::new(p.clone(), "ink-serve", 30);
        assert_eq!(c.base().unwrap(), "http://127.0.0.1:8790");

        let c2 = SvcClient::new(p, "book-serve", 10);
        let err = c2.base().unwrap_err();
        assert_eq!(err, "book-serve 未运行", "错误文案该点名是哪个服务没起来，不是裸的通用提示");
    }

    /// 起一个返回固定状态/体的一次性假服务，返回其端口。
    fn one_shot_server(status: u16, body: &'static str) -> u16 {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        std::thread::spawn(move || {
            for req in server.incoming_requests() {
                let _ = req.respond(tiny_http::Response::from_string(body).with_status_code(status));
            }
        });
        port
    }

    #[test]
    fn svc_client_keeps_server_error_message_and_parses_reply_json() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(&t);
        let me = std::process::id();
        let mut i = info("bad-svc", 1, me);
        i.port = one_shot_server(400, r#"{"ok":false,"message":"这本书正在处理中"}"#);
        let _g1 = register(&p, &i).unwrap();
        let c = SvcClient::new(p.clone(), "bad-svc", 5);
        let e = c.post_json_value("/x", &serde_json::json!({})).unwrap_err();
        assert!(e.contains("这本书正在处理中"), "服务端错误体里的 message 必须带出来: {e}");
        let se = c.try_post_json("/x", &serde_json::json!({})).unwrap_err();
        assert_eq!((se.status, se.message.as_str()), (Some(400), "这本书正在处理中"), "结构化错误不带前缀");
        assert!(!e.contains("status code"), "{e}");
        let e = c.get_json("/y").unwrap_err();
        assert!(e.contains("这本书正在处理中"), "{e}");
        let mut j = info("ok-svc", 2, me);
        j.port = one_shot_server(200, r#"{"queued":3}"#);
        let _g2 = register(&p, &j).unwrap();
        let c = SvcClient::new(p.clone(), "ok-svc", 5);
        assert_eq!(c.post_json_value("/x", &serde_json::json!({})).unwrap()["queued"], 3);
        let mut k = info("plain-svc", 3, me);
        k.port = one_shot_server(500, "boom");
        let _g3 = register(&p, &k).unwrap();
        let e = SvcClient::new(p, "plain-svc", 5).post_json("/x", &serde_json::json!({})).unwrap_err();
        assert!(e.contains("HTTP 500"), "对方体不是 JSON 时回落状态码: {e}");
    }

    #[test]
    fn enc_percent_encodes_path_segments() {
        assert_eq!(enc("a b"), crate::multipart::percent_encode("a b"), "就是 percent_encode 的薄封装，不重新发明编码规则");
    }

    #[test]
    fn find_reads_single_file_and_purges_stale() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(&t);
        let me = std::process::id();
        let _a = register(&p, &info("a-svc", 1, me)).unwrap();
        assert_eq!(find(&p, "a-svc").unwrap().pid, me);
        assert!(find(&p, "nope").is_none());
        assert!(find(&p, "../a-svc").is_none(), "非单段名一律当没有");
        let g = register(&p, &info("dead", 1, u32::MAX)).unwrap();
        std::mem::forget(g);
        assert!(find(&p, "dead").is_none());
        assert!(!p.services_dir().join("dead.json").exists(), "陈旧条目被顺手删");
    }

    #[test]
    fn second_live_instance_cannot_clobber() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(&t);
        let me = std::process::id();
        let _g = register(&p, &info("svc", 1, me)).unwrap();
        // 用 pid 1（init，必活）模拟"另一个活着的实例"
        let err = match register(&p, &info("svc", 1, 1)) {
            Err(e) => e,
            Ok(_) => panic!("同名活实例不该注册成功"),
        };
        assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(find(&p, "svc").unwrap().pid, me);
    }

    #[test]
    fn stale_pid_entries_are_purged() {
        let t = tempfile::tempdir().unwrap();
        let p = paths(&t);
        // pid 4294967295 不可能存活
        let g = register(&p, &info("dead", 1, u32::MAX)).unwrap();
        std::mem::forget(g); // 模拟崩溃：不删文件
        assert!(list(&p).is_empty());
        assert!(!p.services_dir().join("dead.json").exists());
    }
}
