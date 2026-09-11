//! 网关配置 `$XDG_CONFIG_HOME/shelf/gateway.json`：HTTPS 开关、密码哈希、首登必改标志、mDNS 名、额外 SAN。
//! **首次默认密码** [`DEFAULT_PASSWORD`]：首启写入哈希并置 `mustChangePassword=true`，网页登录后强制改密才能进；
//! 忘记密码：设备上 `gateway reset-password`（回到默认并再次强制改）或 `gateway passwd <新密码>`。
use serde::{Deserialize, Serialize};
use rmsvc_core::auth;
use rmsvc_core::paths::Paths;
use std::path::PathBuf;

pub const DEFAULT_PASSWORD: &str = "shelf";
pub const MIN_PASSWORD_LEN: usize = 6;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct GatewayConfig {
    pub https: bool,
    pub auth: bool,
    pub password_hash: String,
    pub must_change_password: bool,
    /// mDNS 主机名（不含 .local）；空=不起 mDNS。
    pub mdns_name: String,
    /// 追加进证书 SAN 的名字/IP（如 host 热点 dnsmasq 里配的别名 `shelf.rm`）。
    pub extra_sans: Vec<String>,
    /// 会话有效期（天）。
    pub session_days: u32,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        GatewayConfig { https: true, auth: true, password_hash: String::new(), must_change_password: false, mdns_name: "shelf".into(), extra_sans: vec!["shelf.rm".into()], session_days: 30 }
    }
}

impl GatewayConfig {
    pub fn path(paths: &Paths) -> PathBuf {
        paths.service_config("gateway")
    }
    pub fn load(paths: &Paths) -> GatewayConfig {
        rmsvc_core::config::load_or_default(&Self::path(paths))
    }
    pub fn save(&self, paths: &Paths) -> Result<(), String> {
        // 含密码哈希 → 0o600。
        rmsvc_core::config::save(&Self::path(paths), self, Some(0o600))
    }

    /// 无密码哈希 → 写入默认密码并标记必改。返回 true 表示本次初始化。
    pub fn ensure_password(&mut self, paths: &Paths) -> Result<bool, String> {
        if !self.password_hash.is_empty() {
            return Ok(false);
        }
        self.password_hash = auth::hash_password(DEFAULT_PASSWORD);
        self.must_change_password = true;
        self.save(paths)?;
        Ok(true)
    }

    #[cfg(test)]
    pub fn verify(&self, pw: &str) -> bool {
        Self::verify_hash(&self.password_hash, pw)
    }

    /// 没有哈希一律不通过。拆成关联函数：`AuthState::verify` 只在锁内拷出哈希，PBKDF2 在锁外算。
    pub fn verify_hash(hash: &str, pw: &str) -> bool {
        !hash.is_empty() && auth::verify_password(pw, hash)
    }

    /// 新密码规则：≥6 位、不能是默认密码。
    pub fn validate_new(pw: &str) -> Result<(), String> {
        if pw.chars().count() < MIN_PASSWORD_LEN {
            return Err(format!("密码至少 {MIN_PASSWORD_LEN} 位"));
        }
        if pw == DEFAULT_PASSWORD {
            return Err("不能用默认密码".into());
        }
        Ok(())
    }

    pub fn set_password(&mut self, paths: &Paths, pw: &str) -> Result<(), String> {
        Self::validate_new(pw)?;
        self.password_hash = auth::hash_password(pw);
        self.must_change_password = false;
        self.save(paths)
    }

    /// 忘记密码：回到默认并强制下次登录改。
    pub fn reset_password(&mut self, paths: &Paths) -> Result<(), String> {
        self.password_hash = auth::hash_password(DEFAULT_PASSWORD);
        self.must_change_password = true;
        self.save(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn paths(t: &tempfile::TempDir) -> Paths {
        let h = t.path().to_str().unwrap().to_string();
        Paths::resolve(move |k| if k == "HOME" { Some(h.clone()) } else { None })
    }
    #[test]
    fn default_password_then_forced_change() {
        let t = tempfile::tempdir().unwrap();
        let paths = paths(&t);
        let mut c = GatewayConfig::load(&paths);
        assert!(c.https && c.auth && c.password_hash.is_empty() && c.mdns_name == "shelf");
        assert!(c.ensure_password(&paths).unwrap());
        assert!(c.must_change_password && c.verify(DEFAULT_PASSWORD) && !c.verify("x"));
        assert!(!c.ensure_password(&paths).unwrap(), "已有哈希不再初始化");
        assert!(c.set_password(&paths, "abc").is_err(), "太短");
        assert!(c.set_password(&paths, DEFAULT_PASSWORD).is_err(), "不能用默认");
        c.set_password(&paths, "newpass1").unwrap();
        let r = GatewayConfig::load(&paths);
        assert!(!r.must_change_password && r.verify("newpass1"));
        c.reset_password(&paths).unwrap();
        let r = GatewayConfig::load(&paths);
        assert!(r.must_change_password && r.verify(DEFAULT_PASSWORD));
    }
}
