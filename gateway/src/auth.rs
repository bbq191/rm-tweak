//! 网关登录策略（HTTP 层只见 [`Guard`]）：
//! - 公开：`/login`、`/logout`、`/ca.crt`、`/health`、`/favicon.ico`。
//! - 会话 Cookie `shelf_session`（HttpOnly；HTTPS 时 Secure；SameSite=Strict）——网页登录后凭它。
//! - `Authorization: Basic *:<密码>`（用户名任意）——CLI 用，免登录页。
//! - **首登必改**：`mustChangePassword` 时会话只能访问 `/password`（网页 303 过去、API 回 403）；
//!   Basic 同理只放行 `POST /password`（CLI `shelf passwd`）。
//! - 未登录：浏览器请求（Accept 含 text/html）303 → `/login?next=…`，其它 401 JSON。密码错延时 500ms。
use crate::config::GatewayConfig;
use rmsvc_core::auth::{parse_basic, parse_cookie, SessionStore};
use rmsvc_core::http::{ApiError, ApiResult, Guard, GuardRequest, Method, Reply, Request};
use rmsvc_core::paths::Paths;
use std::sync::{Arc, Mutex};

pub const COOKIE: &str = "shelf_session";

pub struct AuthState {
    pub cfg: Mutex<GatewayConfig>,
    pub sessions: SessionStore,
    pub paths: Paths,
    pub secure_cookie: bool,
}

pub type Shared = Arc<AuthState>;

fn wants_html(accept: Option<&str>) -> bool {
    accept.map(|a| a.contains("text/html")).unwrap_or(false)
}

fn is_public(method: Method, path: &str) -> bool {
    matches!((method, path), (Method::Get, "/login") | (Method::Post, "/login") | (Method::Post, "/logout") | (Method::Get, "/ca.crt") | (Method::Get, "/health") | (Method::Get, "/favicon.ico"))
}

/// 通过 Cookie 或 Basic 认出的身份。
enum Who {
    Session,
    Basic,
    Nobody,
}

impl AuthState {
    fn identify(&self, r: &GuardRequest) -> Who {
        if let Some(tok) = r.header("Cookie").and_then(|c| parse_cookie(c, COOKIE)) {
            if self.sessions.check(&tok) {
                return Who::Session;
            }
        }
        if let Some((_, pw)) = r.header("Authorization").and_then(parse_basic) {
            if self.cfg.lock().map(|c| c.verify(&pw)).unwrap_or(false) {
                return Who::Basic;
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
        Who::Nobody
    }

    pub fn must_change(&self) -> bool {
        self.cfg.lock().map(|c| c.must_change_password).unwrap_or(false)
    }

    pub fn guard(self: &Arc<Self>) -> Guard {
        let st = self.clone();
        Guard {
            check: Arc::new(move |r: &GuardRequest| {
                if is_public(r.method, &r.path) {
                    return None;
                }
                let html = wants_html(r.header("Accept"));
                match st.identify(r) {
                    Who::Nobody => Some(if html {
                        Reply::redirect(&format!("/login?next={}", rmsvc_core::multipart::percent_encode(&r.path)))
                    } else {
                        Reply::error(401, "需要登录（网页 /login；CLI 用 Basic 密码）").with_header("WWW-Authenticate", "Basic realm=\"shelf\", charset=\"UTF-8\"")
                    }),
                    Who::Session | Who::Basic if st.must_change() && !matches!((r.method, r.path.as_str()), (_, "/password") | (Method::Get, "/api/session")) => Some(if html {
                        Reply::redirect("/password")
                    } else {
                        Reply::error(403, "首次登录必须先改密码：网页打开 /password，或 CLI `shelf passwd`")
                    }),
                    _ => None,
                }
            }),
        }
    }

    fn cookie_header(&self, token: &str, max_age: u64) -> String {
        format!("{COOKIE}={token}; Path=/; Max-Age={max_age}; HttpOnly; SameSite=Strict{}", if self.secure_cookie { "; Secure" } else { "" })
    }

    /// `POST /login`（表单或 JSON `{password}`）。
    pub fn login(&self, req: &mut Request<'_>) -> ApiResult {
        let (pw, next, json) = read_password_body(req)?;
        let ok = self.cfg.lock().map(|c| c.verify(&pw)).unwrap_or(false);
        if !ok {
            std::thread::sleep(std::time::Duration::from_millis(500));
            return Ok(if json { Reply::error(401, "密码错误") } else { Reply::html(&crate::ui::login_page("密码错误", &next)).with_status(401) });
        }
        let tok = self.sessions.issue();
        let dest = if self.must_change() { "/password".to_string() } else if next.starts_with('/') && !next.starts_with("//") { next } else { "/".into() };
        let cookie = self.cookie_header(&tok, self.sessions.ttl().as_secs());
        Ok(if json { Reply::ok(&serde_json::json!({"ok": true, "mustChange": self.must_change(), "next": dest})) } else { Reply::redirect(&dest) }.with_header("Set-Cookie", &cookie))
    }

    /// `POST /logout`。
    pub fn logout(&self, req: &mut Request<'_>) -> ApiResult {
        if let Some(tok) = req.header("Cookie").and_then(|c| parse_cookie(c, COOKIE)) {
            self.sessions.revoke(&tok);
        }
        Ok(Reply::redirect("/login").with_header("Set-Cookie", &self.cookie_header("", 0)))
    }

    /// `POST /password`：`current` + `new` + `confirm`（表单）或 JSON `{current,new}`。
    pub fn change_password(&self, req: &mut Request<'_>) -> ApiResult {
        let json = req.content_type.starts_with("application/json");
        let (current, new, confirm) = if json {
            let v = req.json_body().map_err(ApiError::bad)?;
            let g = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            (g("current"), g("new"), g("new"))
        } else {
            let f = req.form_body().map_err(ApiError::bad)?;
            let g = |k: &str| f.get(k).cloned().unwrap_or_default();
            (g("current"), g("new"), g("confirm"))
        };
        let mut cfg = self.cfg.lock().map_err(|_| ApiError::internal("锁"))?;
        let forced = cfg.must_change_password;
        // 注意：持 cfg 锁期间不能再调 must_change()（std Mutex 不可重入，曾卡死测试）。
        let fail = |msg: &str, status: u16| -> ApiResult { Ok(if json { Reply::error(status, msg) } else { Reply::html(&crate::ui::password_page(msg, forced)).with_status(status) }) };
        // Basic 已证明持有当前密码；会话则必须再输一次当前密码。
        let via_basic = req.header("Authorization").and_then(parse_basic).map(|(_, p)| cfg.verify(&p)).unwrap_or(false);
        if !via_basic && !cfg.verify(&current) {
            drop(cfg);
            std::thread::sleep(std::time::Duration::from_millis(500));
            return fail("当前密码错误", 401);
        }
        if new != confirm {
            return fail("两次输入的新密码不一致", 400);
        }
        if let Err(e) = GatewayConfig::validate_new(&new) {
            return fail(&e, 400);
        }
        cfg.set_password(&self.paths, &new).map_err(ApiError::internal)?;
        drop(cfg);
        // 改密后踢掉其它设备的会话，本会话保留。
        let keep = req.header("Cookie").and_then(|c| parse_cookie(c, COOKIE)).unwrap_or_default();
        self.sessions.revoke_others(&keep);
        Ok(if json { Reply::ok(&serde_json::json!({"ok": true, "message": "密码已更新"})) } else { Reply::redirect("/") })
    }

    /// `GET /api/session`。
    pub fn session_info(&self) -> Reply {
        Reply::ok(&serde_json::json!({"ok": true, "mustChange": self.must_change()}))
    }
}

fn read_password_body(req: &mut Request<'_>) -> Result<(String, String, bool), ApiError> {
    if req.content_type.starts_with("application/json") {
        let v = req.json_body().map_err(ApiError::bad)?;
        Ok((v.get("password").and_then(|x| x.as_str()).unwrap_or("").to_string(), String::new(), true))
    } else {
        let f = req.form_body().map_err(ApiError::bad)?;
        Ok((f.get("password").cloned().unwrap_or_default(), f.get("next").cloned().unwrap_or_default(), false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn state(must_change: bool) -> Shared {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" { Some(h.clone()) } else { None });
        let mut cfg = GatewayConfig::default();
        cfg.ensure_password(&paths).unwrap();
        if !must_change {
            cfg.set_password(&paths, "secret1").unwrap();
        }
        std::mem::forget(t);
        Arc::new(AuthState { cfg: Mutex::new(cfg), sessions: SessionStore::new(std::time::Duration::from_secs(3600), 16), paths, secure_cookie: true })
    }
    fn gr(method: Method, path: &str, headers: &[(&str, &str)]) -> GuardRequest {
        GuardRequest { method, path: path.into(), headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect() }
    }
    fn req<'a>(method: Method, path: &str, ct: &str, headers: &[(&str, &str)], body: &'a mut &[u8]) -> Request<'a> {
        Request { method, path: path.into(), query: HashMap::new(), params: HashMap::new(), content_type: ct.into(), content_length: None, headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), body }
    }

    #[test]
    fn guard_public_redirect_and_basic() {
        let st = state(false);
        let g = st.guard();
        assert!((g.check)(&gr(Method::Get, "/login", &[])).is_none());
        assert!((g.check)(&gr(Method::Get, "/ca.crt", &[])).is_none());
        let r = (g.check)(&gr(Method::Get, "/", &[("Accept", "text/html,*/*")])).unwrap();
        assert_eq!(r.status, 303);
        assert!(r.headers.iter().any(|(k, v)| k == "Location" && v == "/login?next=%2F"));
        let r = (g.check)(&gr(Method::Get, "/api/services", &[])).unwrap();
        assert_eq!(r.status, 401);
        // Basic：用户名任意
        let b = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "whoever:secret1");
        assert!((g.check)(&gr(Method::Get, "/api/services", &[("Authorization", &format!("Basic {b}"))])).is_none());
        let bad = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "x:wrong");
        assert_eq!((g.check)(&gr(Method::Get, "/api/services", &[("Authorization", &format!("Basic {bad}"))])).unwrap().status, 401);
    }

    #[test]
    fn login_sets_cookie_and_must_change_flow() {
        let st = state(true);
        let g = st.guard();
        // 表单登录默认密码 → 303 /password + Set-Cookie
        let mut body: &[u8] = b"password=shelf&next=%2Fx";
        let mut r = req(Method::Post, "/login", "application/x-www-form-urlencoded", &[], &mut body);
        let rep = st.login(&mut r).unwrap();
        assert_eq!(rep.status, 303);
        let loc = rep.headers.iter().find(|(k, _)| k == "Location").unwrap().1.clone();
        assert_eq!(loc, "/password", "首登必改");
        let cookie = rep.headers.iter().find(|(k, _)| k == "Set-Cookie").unwrap().1.clone();
        assert!(cookie.contains("HttpOnly") && cookie.contains("Secure"));
        let tok = cookie.split(';').next().unwrap().to_string();
        // 带会话访问首页 → 被赶去 /password；API → 403
        let r = (g.check)(&gr(Method::Get, "/", &[("Cookie", &tok), ("Accept", "text/html")])).unwrap();
        assert_eq!(r.status, 303);
        assert_eq!((g.check)(&gr(Method::Post, "/api/books", &[("Cookie", &tok)])).unwrap().status, 403);
        assert!((g.check)(&gr(Method::Post, "/password", &[("Cookie", &tok)])).is_none());
        // 改密：不一致 / 太短 / 成功
        let mut b: &[u8] = b"current=shelf&new=abcdef1&confirm=zzz";
        assert_eq!(st.change_password(&mut req(Method::Post, "/password", "application/x-www-form-urlencoded", &[("Cookie", &tok)], &mut b)).unwrap().status, 400);
        let mut b: &[u8] = b"current=shelf&new=abcdef1&confirm=abcdef1";
        let rep = st.change_password(&mut req(Method::Post, "/password", "application/x-www-form-urlencoded", &[("Cookie", &tok)], &mut b)).unwrap();
        assert_eq!(rep.status, 303);
        assert!(!st.must_change());
        assert!((g.check)(&gr(Method::Get, "/", &[("Cookie", &tok)])).is_none(), "改完密码本会话仍有效");
        assert!(st.cfg.lock().unwrap().verify("abcdef1"));
        // 错密码登录 JSON → 401
        let mut b: &[u8] = br#"{"password":"nope"}"#;
        assert_eq!(st.login(&mut req(Method::Post, "/login", "application/json", &[], &mut b)).unwrap().status, 401);
        // 登出
        let rep = st.logout(&mut req(Method::Post, "/logout", "", &[("Cookie", &tok)], &mut (&b""[..]))).unwrap();
        assert_eq!(rep.status, 303);
        assert_eq!((g.check)(&gr(Method::Get, "/api/services", &[("Cookie", &tok)])).unwrap().status, 401);
    }

    #[test]
    fn basic_can_change_password_without_current_field() {
        let st = state(true);
        let b = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, "cli:shelf");
        let mut body: &[u8] = br#"{"new":"longer1"}"#;
        let rep = st.change_password(&mut req(Method::Post, "/password", "application/json", &[("Authorization", &format!("Basic {b}"))], &mut body)).unwrap();
        assert_eq!(rep.status, 200, "{}", String::from_utf8_lossy(&rep.body));
        assert!(!st.must_change());
    }
}
