//! 密码哈希 + HTTP Basic 解析 + 内存会话表。只保护对外的网关；loopback 领域服务不认证（只有设备
//! 本机能连）。会话：登录页校验密码后发 Cookie 令牌（随机 32 字节 hex），令牌只在内存（重启网关=
//! 全部重新登录）。
//!
//! **哈希格式（2026-09-09 审计修）**：新哈希是 `pbkdf2$<rounds>$<salt-hex>$<digest-hex>`
//! （PBKDF2-HMAC-SHA256，salt 16 字节随机）——旧格式 `sha256$<salt-hex>$<digest-hex>` 是单轮
//! 加盐 SHA-256，没有任何迭代/慢哈希，网关是全系统唯一的网络暴露面，密码文件一旦泄露可被消费级
//! 硬件快速离线暴力破解（尤其密码最小长度只有 6 位）。`verify_password` **两种格式都认**——已经
//! 部署在真机上的旧哈希不用强制改密就能继续登录；下次用户改密码（`passwd`/网页改密）时
//! `hash_password` 只会产出新格式，自然完成迁移，不需要额外的迁移脚本或强制登出。
use base64::Engine;
use sha2::{Digest, Sha256};

/// PBKDF2 迭代次数：OWASP 2023 对 PBKDF2-HMAC-SHA256 的建议下限是 600,000；登录是低频操作（一次
/// 会话一次，不是热路径），这个开销可接受。真机验证过实际耗时（见 auth.rs 测试/白皮书），如果
/// 设备实测明显卡顿再下调，不要凭空猜数字改。
const PBKDF2_ROUNDS: u32 = 600_000;

fn random_bytes(n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut v = vec![0u8; n];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        if f.read_exact(&mut v).is_ok() {
            return v;
        }
    }
    // 兜底：时间+pid 混合（只在 /dev/urandom 不可用的怪环境）
    let t = crate::clock::now_nanos();
    let mut h = Sha256::new();
    h.update(t.to_le_bytes());
    h.update(std::process::id().to_le_bytes());
    h.finalize()[..n.min(32)].to_vec()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn hex_decode(s: &str) -> Option<Vec<u8>> {
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok())).collect()
}

/// 常数时间比较（长度相同时逐字节异或），两种格式共用。
fn constant_time_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn hash_password(password: &str) -> String {
    let salt = random_bytes(16);
    hash_pbkdf2(password, &salt, PBKDF2_ROUNDS)
}

fn hash_pbkdf2(password: &str, salt: &[u8], rounds: u32) -> String {
    let mut digest = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, rounds, &mut digest);
    format!("pbkdf2${rounds}${}${}", hex(salt), hex(&digest))
}

/// 旧格式（2026-09 之前）：单轮加盐 SHA-256，没有迭代——只为兼容已部署在真机上的旧哈希保留读路径，
/// `hash_password` 从不再产出这个格式。
fn hash_sha256_legacy(password: &str, salt: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(password.as_bytes());
    format!("sha256${}${}", hex(salt), hex(&h.finalize()))
}

pub fn verify_password(password: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.split('$').collect();
    match parts.as_slice() {
        ["pbkdf2", rounds, salt_hex, _digest_hex] => {
            let Ok(rounds) = rounds.parse::<u32>() else { return false };
            let Some(salt) = hex_decode(salt_hex) else { return false };
            constant_time_eq(&hash_pbkdf2(password, &salt, rounds), stored)
        }
        ["sha256", salt_hex, _digest_hex] => {
            let Some(salt) = hex_decode(salt_hex) else { return false };
            constant_time_eq(&hash_sha256_legacy(password, &salt), stored)
        }
        _ => false,
    }
}

/// 解析 `Authorization: Basic …` 头 → (user, password)。
pub fn parse_basic(header: &str) -> Option<(String, String)> {
    let b64 = header.strip_prefix("Basic ")?.trim();
    let raw = base64::engine::general_purpose::STANDARD.decode(b64).ok()?;
    let s = String::from_utf8(raw).ok()?;
    let (u, p) = s.split_once(':')?;
    Some((u.to_string(), p.to_string()))
}

/// 解析 `Cookie:` 头里指定名字的值。
pub fn parse_cookie(header: &str, name: &str) -> Option<String> {
    header.split(';').map(|kv| kv.trim()).find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k.trim() == name).then(|| v.trim().to_string())
    })
}

/// 内存会话表（Mutex 保护）：签发/校验/吊销，带绝对过期与容量上限（防无限增长）。
pub struct SessionStore {
    inner: std::sync::Mutex<std::collections::HashMap<String, std::time::Instant>>,
    ttl: std::time::Duration,
    max: usize,
}

impl SessionStore {
    pub fn new(ttl: std::time::Duration, max: usize) -> SessionStore {
        SessionStore { inner: std::sync::Mutex::new(std::collections::HashMap::new()), ttl, max }
    }
    /// 签发新令牌；满了先清过期，仍满则淘汰最早到期的。
    pub fn issue(&self) -> String {
        let token = hex(&random_bytes(32));
        let mut m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let now = std::time::Instant::now();
        m.retain(|_, exp| *exp > now);
        if m.len() >= self.max {
            if let Some(oldest) = m.iter().min_by_key(|(_, e)| **e).map(|(k, _)| k.clone()) {
                m.remove(&oldest);
            }
        }
        m.insert(token.clone(), now + self.ttl);
        token
    }
    pub fn check(&self, token: &str) -> bool {
        let m = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        m.get(token).map(|exp| *exp > std::time::Instant::now()).unwrap_or(false)
    }
    pub fn revoke(&self, token: &str) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).remove(token);
    }
    /// 吊销除 `keep` 外的全部（改密码后踢掉其它设备）。
    pub fn revoke_others(&self, keep: &str) {
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).retain(|k, _| k == keep);
    }
    pub fn ttl(&self) -> std::time::Duration {
        self.ttl
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cookie_and_sessions() {
        assert_eq!(parse_cookie("a=1; shelf_session=abc ; b=2", "shelf_session").as_deref(), Some("abc"));
        assert_eq!(parse_cookie("a=1", "shelf_session"), None);
        let s = SessionStore::new(std::time::Duration::from_secs(60), 2);
        let t1 = s.issue();
        assert!(s.check(&t1) && !s.check("nope"));
        let t2 = s.issue();
        let t3 = s.issue(); // 超容量：最早的 t1 被淘汰
        assert!(!s.check(&t1) && s.check(&t2) && s.check(&t3));
        s.revoke_others(&t3);
        assert!(!s.check(&t2) && s.check(&t3));
        s.revoke(&t3);
        assert!(!s.check(&t3));
        let e = SessionStore::new(std::time::Duration::from_secs(0), 8);
        let t = e.issue();
        assert!(!e.check(&t), "ttl=0 立即过期");
    }
    #[test]
    fn hash_roundtrip_and_reject() {
        let h = hash_password("s3cret");
        assert!(h.starts_with("pbkdf2$"), "新哈希该是 PBKDF2 格式：{h}");
        assert!(verify_password("s3cret", &h));
        assert!(!verify_password("s3cre", &h));
        assert!(!verify_password("s3cret", "garbage"));
        assert_ne!(hash_password("x"), hash_password("x"), "salt 随机");
    }
    #[test]
    fn legacy_sha256_hash_still_verifies_but_new_hashes_never_produce_it() {
        // 2026-09-09 升级 PBKDF2 后：已经部署在真机上的旧哈希（单轮 SHA-256）不能因为这次升级就
        // 登不进去——旧格式必须继续能验证；但 hash_password 从此只产出新格式，不会再生成旧格式，
        // 用户下次改密码时自然完成迁移。
        let legacy = hash_sha256_legacy("s3cret", b"0123456789abcdef");
        assert!(legacy.starts_with("sha256$"));
        assert!(verify_password("s3cret", &legacy), "旧格式哈希应该继续能验证");
        assert!(!verify_password("wrong", &legacy));
        assert!(!hash_password("s3cret").starts_with("sha256$"), "新哈希永远不该是旧格式");
    }
    #[test]
    fn pbkdf2_rejects_malformed_or_tampered_rounds() {
        let h = hash_password("s3cret");
        let tampered = h.replacen(&format!("${PBKDF2_ROUNDS}$"), "$1$", 1);
        assert_ne!(tampered, h, "确认真的替换到了");
        assert!(!verify_password("s3cret", &tampered), "轮数被改，摘要对不上");
        assert!(!verify_password("s3cret", "pbkdf2$notanumber$aa$bb"), "轮数不是数字应直接拒绝而不是 panic");
    }
    #[test]
    fn basic_header() {
        assert_eq!(parse_basic("Basic c2hlbGY6cGFzczp3b3Jk"), Some(("shelf".into(), "pass:word".into())));
        assert_eq!(parse_basic("Bearer x"), None);
    }
}
