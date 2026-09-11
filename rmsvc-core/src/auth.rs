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
        let mut m = crate::sync::lock(&self.inner);
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
        let m = crate::sync::lock(&self.inner);
        m.get(token).map(|exp| *exp > std::time::Instant::now()).unwrap_or(false)
    }
    pub fn revoke(&self, token: &str) {
        crate::sync::lock(&self.inner).remove(token);
    }
    /// 吊销除 `keep` 外的全部（改密码后踢掉其它设备）。
    pub fn revoke_others(&self, keep: &str) {
        crate::sync::lock(&self.inner).retain(|k, _| k == keep);
    }
    pub fn ttl(&self) -> std::time::Duration {
        self.ttl
    }
}

/// 登录失败限速（**按来源 IP 各自滑动窗口**）：同一 IP 在 `window` 内累计失败 `max` 次即锁定该 IP，
/// 锁到它最早那次失败滑出窗口为止；别的 IP 不受影响。锁定期间调用方应**不做密码校验**直接拒绝
/// （校验是 60 万轮 PBKDF2，被并发猜密码时既是暴力破解通道又是 CPU 消耗通道；单靠"失败后 sleep 500ms"
/// 挡不住并行连接）。
///
/// 2026-09-24 前是全局计数（HTTP 层当时不带对端地址）：局域网里任何人连错 5 次就把主人一起锁住。
/// 现在 IP 来自 TCP 对端地址（tiny_http `remote_addr()`，TLS 下同样取自底层 TcpStream，不信任
/// `X-Forwarded-For` 之类可伪造的头）。
///
/// **不豁免任何网段**（含 USB 网段 10.11.99.0/24 与 127.0.0.1）：设备的 lo 别名本身就是 10.11.99.1，
/// 本机进程/经本机转发进来的流量都可能以这些地址出现，豁免等于给一整类请求开了无限猜密码的口子。
///
/// 表最多记 `cap` 个 IP（防止大量来源地址把内存撑大）；满了先扔窗口已过期的，仍满则淘汰
/// **未锁定的 IP 里最久没失败的**，全都锁定时才淘汰最旧的锁定项——免得攻击者换一批地址各错一次，
/// 就把自己已被锁的 IP 挤出表外"洗白"。重启网关清零。
pub struct IpFailLimiter {
    max: usize,
    window: std::time::Duration,
    cap: usize,
    fails: std::sync::Mutex<std::collections::HashMap<std::net::IpAddr, std::collections::VecDeque<std::time::Instant>>>,
}

impl IpFailLimiter {
    pub fn new(max: usize, window: std::time::Duration, cap: usize) -> IpFailLimiter {
        IpFailLimiter { max, window, cap: cap.max(1), fails: std::sync::Mutex::new(std::collections::HashMap::new()) }
    }
    /// IPv4 映射的 IPv6（`::ffff:a.b.c.d`）与纯 IPv4 算同一个来源。
    fn key(ip: std::net::IpAddr) -> std::net::IpAddr {
        ip.to_canonical()
    }
    fn prune(&self, q: &mut std::collections::VecDeque<std::time::Instant>, now: std::time::Instant) {
        while q.front().is_some_and(|t| now.duration_since(*t) >= self.window) {
            q.pop_front();
        }
    }
    /// `ip` 被锁定时返回还要等多久，否则 `None`。
    pub fn locked_for(&self, ip: std::net::IpAddr) -> Option<std::time::Duration> {
        self.locked_for_at(ip, std::time::Instant::now())
    }
    pub fn locked_for_at(&self, ip: std::net::IpAddr, now: std::time::Instant) -> Option<std::time::Duration> {
        let mut m = crate::sync::lock(&self.fails);
        let q = m.get_mut(&Self::key(ip))?;
        self.prune(q, now);
        if q.len() >= self.max {
            q.front().map(|t| self.window.saturating_sub(now.duration_since(*t)))
        } else {
            None
        }
    }
    pub fn record_failure(&self, ip: std::net::IpAddr) {
        self.record_failure_at(ip, std::time::Instant::now());
    }
    pub fn record_failure_at(&self, ip: std::net::IpAddr, now: std::time::Instant) {
        let ip = Self::key(ip);
        let mut m = crate::sync::lock(&self.fails);
        if !m.contains_key(&ip) && m.len() >= self.cap {
            // 满了：先扔窗口已过期的
            m.retain(|_, q| q.back().is_some_and(|t| now.duration_since(*t) < self.window));
            if m.len() >= self.cap {
                // 仍满：未锁定优先、其中最久没失败的先走
                let victim = m.iter().min_by_key(|(_, q)| (q.len() >= self.max, q.back().copied())).map(|(k, _)| *k);
                if let Some(v) = victim {
                    m.remove(&v);
                }
            }
        }
        let q = m.entry(ip).or_default();
        self.prune(q, now);
        if q.len() >= self.max {
            return; // 已锁定：不再累计，避免攻击者不停撞把锁定期无限续下去
        }
        q.push_back(now);
    }
    /// 该 IP 登录成功：清零（只清它自己的）。
    pub fn reset(&self, ip: std::net::IpAddr) {
        crate::sync::lock(&self.fails).remove(&Self::key(ip));
    }
    /// 当前表里记着几个 IP（测试/诊断用）。
    pub fn tracked(&self) -> usize {
        crate::sync::lock(&self.fails).len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fail_limiter_locks_after_max_and_expires() {
        use std::time::{Duration, Instant};
        let a: std::net::IpAddr = "192.168.1.5".parse().unwrap();
        let l = IpFailLimiter::new(3, Duration::from_secs(60), 16);
        let t0 = Instant::now();
        assert!(l.locked_for_at(a, t0).is_none());
        l.record_failure_at(a, t0);
        l.record_failure_at(a, t0 + Duration::from_secs(1));
        assert!(l.locked_for_at(a, t0 + Duration::from_secs(2)).is_none(), "未满 3 次不锁");
        l.record_failure_at(a, t0 + Duration::from_secs(2));
        let w = l.locked_for_at(a, t0 + Duration::from_secs(10)).expect("满 3 次锁定");
        assert_eq!(w, Duration::from_secs(50), "锁到最早那次失败滑出窗口");
        l.record_failure_at(a, t0 + Duration::from_secs(11)); // 锁定期间再有失败不延长
        assert_eq!(l.locked_for_at(a, t0 + Duration::from_secs(20)), Some(Duration::from_secs(40)));
        assert!(l.locked_for_at(a, t0 + Duration::from_secs(61)).is_none(), "最早一次滑出窗口后解锁");
        l.reset(a);
        assert!(l.locked_for_at(a, t0).is_none());
    }
    #[test]
    fn fail_limiter_is_per_ip() {
        use std::time::{Duration, Instant};
        let a: std::net::IpAddr = "10.11.99.1".parse().unwrap();
        let b: std::net::IpAddr = "192.168.1.9".parse().unwrap();
        let l = IpFailLimiter::new(2, Duration::from_secs(60), 16);
        let t0 = Instant::now();
        l.record_failure_at(a, t0);
        l.record_failure_at(a, t0);
        assert!(l.locked_for_at(a, t0).is_some(), "A 锁定（USB 网段不豁免）");
        assert!(l.locked_for_at(b, t0).is_none(), "B 不受影响");
        let mapped: std::net::IpAddr = "::ffff:10.11.99.1".parse().unwrap();
        assert!(l.locked_for_at(mapped, t0).is_some(), "IPv4 映射地址与原地址同一来源");
        l.reset(b);
        assert!(l.locked_for_at(a, t0).is_some(), "B 登录成功不清 A");
    }
    #[test]
    fn fail_limiter_caps_table_and_evicts_oldest() {
        use std::time::{Duration, Instant};
        let ip = |i: u8| -> std::net::IpAddr { std::net::Ipv4Addr::new(192, 168, 0, i).into() };
        let l = IpFailLimiter::new(2, Duration::from_secs(60), 4);
        let t0 = Instant::now();
        // ip(1) 锁定；ip(2..=4) 各错一次，ip(2) 最早
        l.record_failure_at(ip(1), t0);
        l.record_failure_at(ip(1), t0);
        for i in 2..=4 {
            l.record_failure_at(ip(i), t0 + Duration::from_secs(i as u64));
        }
        assert_eq!(l.tracked(), 4);
        l.record_failure_at(ip(5), t0 + Duration::from_secs(10));
        assert_eq!(l.tracked(), 4, "不超过上限");
        assert!(l.locked_for_at(ip(1), t0 + Duration::from_secs(10)).is_some(), "锁定项不被换地址挤出去");
        l.record_failure_at(ip(2), t0 + Duration::from_secs(11));
        assert!(l.locked_for_at(ip(2), t0 + Duration::from_secs(11)).is_none(), "ip(2) 是最旧的未锁定项，已被淘汰后重新计数");
        // 全部锁定时淘汰最旧的锁定项
        let l = IpFailLimiter::new(1, Duration::from_secs(60), 2);
        l.record_failure_at(ip(1), t0);
        l.record_failure_at(ip(2), t0 + Duration::from_secs(1));
        l.record_failure_at(ip(3), t0 + Duration::from_secs(2));
        assert_eq!(l.tracked(), 2);
        let t = t0 + Duration::from_secs(3);
        assert!(l.locked_for_at(ip(1), t).is_none() && l.locked_for_at(ip(2), t).is_some() && l.locked_for_at(ip(3), t).is_some());
        // 窗口过期的先清
        l.record_failure_at(ip(4), t0 + Duration::from_secs(100));
        assert_eq!(l.tracked(), 1, "过期项一次清掉");
    }
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
