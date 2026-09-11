//! 网关 TLS：**私有 CA + 叶证书**（首启生成到 `<dir>/`，之后复用）。
//! - `ca.pem`/`ca.key`：私有根（10 年）。用户把 `ca.pem` 装进手机/电脑的信任库一次，此后叶证书随便换都不再提示。
//! - `cert.pem`（叶 + CA 链）/`key.pem`：叶证书由 CA 签发，SAN 含 `shelf.local`/设备 IP 等；
//!   有效期 800 天（Apple 平台拒绝 >825 天的 TLS 服务器证书），过期前 30 天或 SAN 变化时自动换叶、CA 不变。
//! 不装 CA 时浏览器仍提示"不受信任"（自签），确认一次即可；CLI 侧默认不校验证书（局域网 + 密码保护）。
use crate::clock::now_secs;
use rcgen::{BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, Issuer, KeyPair, KeyUsagePurpose};
use std::path::Path;

pub struct TlsPem {
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
}

/// 叶证书自动续签阈值：签发满这么多天就换（远小于 800 天有效期）。
const LEAF_RENEW_DAYS: u64 = 700;
const LEAF_VALID_DAYS: i64 = 800;
const CA_VALID_DAYS: i64 = 3650;

fn ca_params() -> Result<CertificateParams, rcgen::Error> {
    let mut p = CertificateParams::new(Vec::<String>::new())?;
    p.distinguished_name = rcgen::DistinguishedName::new();
    p.distinguished_name.push(DnType::CommonName, "shelf 书架私有 CA");
    p.distinguished_name.push(DnType::OrganizationName, "cang-jie shelf");
    p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    p.key_usages = vec![KeyUsagePurpose::KeyCertSign, KeyUsagePurpose::CrlSign, KeyUsagePurpose::DigitalSignature];
    p.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    p.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(CA_VALID_DAYS);
    Ok(p)
}

fn leaf_params(sans: Vec<String>) -> Result<CertificateParams, rcgen::Error> {
    let mut p = CertificateParams::new(sans)?;
    p.distinguished_name = rcgen::DistinguishedName::new();
    p.distinguished_name.push(DnType::CommonName, "shelf");
    p.is_ca = IsCa::ExplicitNoCa;
    p.key_usages = vec![KeyUsagePurpose::DigitalSignature, KeyUsagePurpose::KeyEncipherment];
    p.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    p.not_before = time::OffsetDateTime::now_utc() - time::Duration::days(1);
    p.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(LEAF_VALID_DAYS);
    Ok(p)
}

fn write_private(p: &Path, data: &[u8]) -> Result<(), String> {
    std::fs::write(p, data).map_err(|e| format!("写 {}: {e}", p.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// 默认 SAN（设备常用地址）+ 调用方追加（当前 IP、mDNS 名）。
pub fn default_sans(extra: &[String]) -> Vec<String> {
    let mut sans: Vec<String> = vec!["shelf".into(), "shelf.local".into(), "localhost".into(), "remarkable".into(), "remarkable.local".into(), "10.11.99.1".into(), "127.0.0.1".into()];
    for s in extra {
        let s = s.trim();
        if !s.is_empty() && !sans.iter().any(|x| x == s) {
            sans.push(s.to_string());
        }
    }
    sans
}

/// 读取或生成 CA + 叶证书。`extra_sans` 追加到叶证书 SAN；SAN 集合变化或叶证书临期则只换叶。
/// 旧版（无 `ca.pem` 的纯自签）自动升级为 CA 签发（浏览器会再提示一次）。
pub fn ensure_ca_signed(dir: &Path, extra_sans: &[String]) -> Result<TlsPem, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (ca_pem_p, ca_key_p) = (dir.join("ca.pem"), dir.join("ca.key"));
    let (cert_p, key_p, meta_p) = (dir.join("cert.pem"), dir.join("key.pem"), dir.join("cert.meta"));
    let sans = default_sans(extra_sans);
    // CA：有则加载（只需私钥；DN/用途由 ca_params 重建，与首次一致），无则生成。
    let ca_key = match std::fs::read_to_string(&ca_key_p) {
        Ok(pem) if ca_pem_p.is_file() => KeyPair::from_pem(&pem).map_err(|e| format!("读 CA 私钥: {e}"))?,
        _ => {
            let kp = KeyPair::generate().map_err(|e| format!("生成 CA 密钥: {e}"))?;
            let cert = ca_params().and_then(|p| p.self_signed(&kp)).map_err(|e| format!("生成 CA: {e}"))?;
            std::fs::write(&ca_pem_p, cert.pem()).map_err(|e| e.to_string())?;
            write_private(&ca_key_p, kp.serialize_pem().as_bytes())?;
            let _ = std::fs::remove_file(&cert_p); // 旧叶作废
            kp
        }
    };
    let ca_pem = std::fs::read(&ca_pem_p).map_err(|e| e.to_string())?;
    // 叶：meta 记 "签发时间戳\nSAN 列表"；不匹配/临期/缺失 → 重签。
    let want_meta = format!("{}\n{}\n", now_secs(), sans.join(","));
    let reuse = cert_p.is_file() && key_p.is_file() && std::fs::read_to_string(&meta_p).ok().map(|m| {
        let mut it = m.lines();
        let issued: u64 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let same_sans = it.next() == Some(&sans.join(","));
        same_sans && now_secs().saturating_sub(issued) < LEAF_RENEW_DAYS * 86400
    }).unwrap_or(false);
    if reuse {
        return Ok(TlsPem { cert: std::fs::read(&cert_p).map_err(|e| e.to_string())?, key: std::fs::read(&key_p).map_err(|e| e.to_string())? });
    }
    let ca_p = ca_params().map_err(|e| e.to_string())?;
    let issuer = Issuer::from_params(&ca_p, &ca_key);
    let leaf_key = KeyPair::generate().map_err(|e| format!("生成叶密钥: {e}"))?;
    let leaf = leaf_params(sans).and_then(|p| p.signed_by(&leaf_key, &issuer)).map_err(|e| format!("签发叶证书: {e}"))?;
    let mut chain = leaf.pem().into_bytes();
    chain.extend_from_slice(&ca_pem);
    let key = leaf_key.serialize_pem().into_bytes();
    std::fs::write(&cert_p, &chain).map_err(|e| e.to_string())?;
    write_private(&key_p, &key)?;
    std::fs::write(&meta_p, want_meta).map_err(|e| e.to_string())?;
    Ok(TlsPem { cert: chain, key })
}

/// CA 证书 PEM（给 `/ca.crt` 下载）。
pub fn ca_pem(dir: &Path) -> Option<Vec<u8>> {
    std::fs::read(dir.join("ca.pem")).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ca_persists_and_leaf_rotates_on_san_change() {
        let t = tempfile::tempdir().unwrap();
        let a = ensure_ca_signed(t.path(), &["10.42.0.224".into()]).unwrap();
        let s = std::str::from_utf8(&a.cert).unwrap();
        assert_eq!(s.matches("BEGIN CERTIFICATE").count(), 2, "叶 + CA 链");
        assert!(std::str::from_utf8(&a.key).unwrap().contains("BEGIN PRIVATE KEY"));
        let ca1 = ca_pem(t.path()).unwrap();
        let b = ensure_ca_signed(t.path(), &["10.42.0.224".into()]).unwrap();
        assert_eq!(a.cert, b.cert, "SAN 不变则复用叶");
        let c = ensure_ca_signed(t.path(), &["10.42.0.9".into()]).unwrap();
        assert_ne!(a.cert, c.cert, "SAN 变化换叶");
        assert_eq!(ca_pem(t.path()).unwrap(), ca1, "CA 不变");
        assert!(std::str::from_utf8(&c.cert).unwrap().ends_with(std::str::from_utf8(&ca1).unwrap()));
    }
    #[test]
    fn upgrades_legacy_self_signed() {
        let t = tempfile::tempdir().unwrap();
        std::fs::write(t.path().join("cert.pem"), "old").unwrap();
        std::fs::write(t.path().join("key.pem"), "old").unwrap();
        let a = ensure_ca_signed(t.path(), &[]).unwrap();
        assert!(t.path().join("ca.pem").is_file());
        assert_ne!(a.cert, b"old");
    }
}
