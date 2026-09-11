//! 网关 TLS：**私有 CA + 叶证书**（首启生成到 `<dir>/`，之后复用）。
//! - `ca.pem`/`ca.key`：私有根（10 年）。用户把 `ca.pem` 装进手机/电脑的信任库一次，此后叶证书随便换都不再提示。
//! - `cert.pem`（叶 + CA 链）/`key.pem`：叶证书由 CA 签发，SAN 含 `shelf.local`/设备 IP 等；
//!   有效期 800 天（Apple 平台拒绝 >825 天的 TLS 服务器证书），过期前 30 天或 SAN 变化时自动换叶、CA 不变。
//!
//!
//! **名称约束（2026-09-24）**：CA 带 NameConstraints，只准签 [`PERMITTED_DNS`]（及其子域）与私网/回环
//! IPv4 段 [`PERMITTED_V4`]。用户把这个 CA 装进了手机/电脑的系统信任库，若 CA 不带约束，设备上的
//! `ca.key` 一旦泄露就能给任何网站（银行、邮箱……）签出被信任的证书；带约束后泄露的危害被限定在
//! 这几个本地名字与私网地址内。叶证书的 SAN 必须全部落在约束内，越界的 SAN 会被**剔除并打日志**
//! （否则浏览器整张证书都拒）。旧版无约束 CA 在启动时自动迁移：旧文件改名 `.bak-<秒>` 保留，
//! 重新生成 CA + 叶，并打印"手机和电脑需要重新安装证书"。
//!
//! 不装 CA 时浏览器仍提示"不受信任"（自签），确认一次即可；CLI 侧默认不校验证书（局域网 + 密码保护）。
use crate::clock::now_secs;
use rcgen::{BasicConstraints, CertificateParams, CidrSubnet, DnType, ExtendedKeyUsagePurpose, GeneralSubtree, IsCa, Issuer, KeyPair, KeyUsagePurpose, NameConstraints};
use std::net::{IpAddr, Ipv4Addr};
use std::path::Path;

pub struct TlsPem {
    pub cert: Vec<u8>,
    pub key: Vec<u8>,
}

/// 叶证书自动续签阈值：签发满这么多天就换（远小于 800 天有效期）。
const LEAF_RENEW_DAYS: u64 = 700;
const LEAF_VALID_DAYS: i64 = 800;
const CA_VALID_DAYS: i64 = 3650;

/// CA 名称约束允许的 DNS 名（dNSName 约束语义：名字本身及其全部子域）。取自 [`default_sans`] 里的固定名
/// 加网关默认的 mDNS 名 `shelf.local`、热点 dnsmasq 别名 `shelf.rm`（gateway 配置 `extra_sans` 默认值）。
/// 用户若把 mDNS 名/额外 SAN 改成别的名字，那些名字不在约束内，签叶时会被剔除（见 [`ensure_ca_signed`]）。
pub const PERMITTED_DNS: &[&str] = &["shelf", "shelf.local", "shelf.rm", "localhost", "remarkable", "remarkable.local"];

/// CA 名称约束允许的 IPv4 段：RFC 1918 私网三段 + 回环。USB 网段 10.11.99.0/24、常见热点 10.42.0.0/24、
/// 家用路由 192.168.x.x 都在内。设备若拿到这之外的地址（如 CGNAT 100.64/10、链路本地 169.254/16、公网），
/// 该 IP 不进叶证书 SAN，只能改用域名访问。
pub const PERMITTED_V4: &[([u8; 4], u8)] = &[([10, 0, 0, 0], 8), ([172, 16, 0, 0], 12), ([192, 168, 0, 0], 16), ([127, 0, 0, 0], 8)];

fn name_constraints() -> NameConstraints {
    let mut permitted: Vec<GeneralSubtree> = PERMITTED_DNS.iter().map(|d| GeneralSubtree::DnsName(d.to_string())).collect();
    permitted.extend(PERMITTED_V4.iter().map(|(a, p)| GeneralSubtree::IpAddress(CidrSubnet::from_v4_prefix(*a, *p))));
    NameConstraints { permitted_subtrees: permitted, excluded_subtrees: vec![] }
}

/// 某个 SAN 是否落在 CA 名称约束内（与浏览器的判定一致：IP 按网段，DNS 按"等于或是其子域"，不分大小写）。
pub fn san_permitted(san: &str) -> bool {
    match san.parse::<IpAddr>() {
        Ok(IpAddr::V4(ip)) => PERMITTED_V4.iter().any(|(a, p)| {
            let mask = if *p == 0 { 0 } else { u32::MAX << (32 - u32::from(*p)) };
            u32::from(ip) & mask == u32::from(Ipv4Addr::from(*a)) & mask
        }),
        Ok(IpAddr::V6(_)) => false,
        Err(_) => {
            let n = san.trim_end_matches('.').to_ascii_lowercase();
            PERMITTED_DNS.iter().any(|d| n == *d || n.ends_with(&format!(".{d}")))
        }
    }
}

/// 解析 CA 证书 PEM，看它是否带了非空的 permitted 名称约束。解析失败按"没有"算（会触发迁移重建）。
pub fn ca_has_name_constraints(pem: &[u8]) -> bool {
    let Ok((_, p)) = x509_parser::pem::parse_x509_pem(pem) else { return false };
    let Ok(cert) = p.parse_x509() else { return false };
    matches!(cert.name_constraints(), Ok(Some(nc)) if nc.value.permitted_subtrees.as_ref().is_some_and(|v| !v.is_empty()))
}

/// 把 `dir` 下的 CA 与叶证书文件改名为 `<名>.bak-<秒>`（不删，便于回退/核对），返回改了哪些。
fn backup_tls_files(dir: &Path, stamp: u64) -> Result<Vec<String>, String> {
    let mut moved = Vec::new();
    for f in ["ca.pem", "ca.key", "cert.pem", "key.pem", "cert.meta"] {
        let from = dir.join(f);
        if from.is_file() {
            let to = dir.join(format!("{f}.bak-{stamp}"));
            std::fs::rename(&from, &to).map_err(|e| format!("备份 {} → {}: {e}", from.display(), to.display()))?;
            moved.push(to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
    }
    Ok(moved)
}

fn ca_params() -> Result<CertificateParams, rcgen::Error> {
    let mut p = CertificateParams::new(Vec::<String>::new())?;
    p.distinguished_name = rcgen::DistinguishedName::new();
    p.distinguished_name.push(DnType::CommonName, "shelf 书架私有 CA");
    p.distinguished_name.push(DnType::OrganizationName, "cang-jie shelf");
    // 路径长度 0：此 CA 只签叶证书，不能再派生下级 CA。
    p.is_ca = IsCa::Ca(BasicConstraints::Constrained(0));
    p.name_constraints = Some(name_constraints());
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
/// 旧版无名称约束的 CA：备份为 `.bak-<秒>` 后重建 CA + 叶（已装旧 CA 的手机/电脑要重新装）。
/// 不在 CA 名称约束内的 SAN 不进叶证书（打日志说明）。
pub fn ensure_ca_signed(dir: &Path, extra_sans: &[String]) -> Result<TlsPem, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let (ca_pem_p, ca_key_p) = (dir.join("ca.pem"), dir.join("ca.key"));
    let (cert_p, key_p, meta_p) = (dir.join("cert.pem"), dir.join("key.pem"), dir.join("cert.meta"));
    let (sans, dropped): (Vec<String>, Vec<String>) = default_sans(extra_sans).into_iter().partition(|s| san_permitted(s));
    if !dropped.is_empty() {
        eprintln!("[tls] 以下地址/名字不在私有 CA 名称约束内（只允许 {} 与私网/回环 IPv4），已从证书中剔除，用它们访问会提示证书错误: {}", PERMITTED_DNS.join("/"), dropped.join(", "));
    }
    // 迁移：CA 在但不带名称约束（2026-09-24 前生成的）→ 备份后走下面的"生成"分支。
    if ca_pem_p.is_file() && ca_key_p.is_file() {
        let pem = std::fs::read(&ca_pem_p).map_err(|e| format!("读 {}: {e}", ca_pem_p.display()))?;
        if !ca_has_name_constraints(&pem) {
            let moved = backup_tls_files(dir, now_secs())?;
            println!("[tls] 旧私有 CA 没有名称约束（私钥泄露可给任意网站签证书），已重新生成带约束的 CA 与服务器证书；旧文件已备份: {}", moved.join(", "));
            println!("[tls] 注意：手机和电脑需要重新安装证书：打开网关 /ca.crt 下载新 CA 装进信任库，并删除旧的「shelf 书架私有 CA」");
        }
    }
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
    // 叶的签发者 DN/密钥标识由 ca_params 重建（与 ca.pem 一致）；名称约束只写在 CA 证书里，叶不带。
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
    /// PEM 里的全部证书（DER）。
    fn ders(pem: &[u8]) -> Vec<Vec<u8>> {
        x509_parser::pem::Pem::iter_from_buffer(pem).map(|p| p.unwrap().contents).collect()
    }

    /// 叶证书里的 SAN（DNS 名原样，IP 转成点分字符串）。
    fn leaf_sans(der: &[u8]) -> Vec<String> {
        use x509_parser::extensions::GeneralName;
        let (_, c) = x509_parser::parse_x509_certificate(der).unwrap();
        let san = c.subject_alternative_name().unwrap().expect("叶证书要有 SAN");
        san.value
            .general_names
            .iter()
            .map(|g| match g {
                GeneralName::DNSName(d) => d.to_string(),
                GeneralName::IPAddress(b) => IpAddr::from(<[u8; 4]>::try_from(*b).unwrap()).to_string(),
                other => panic!("意外的 SAN 类型 {other:?}"),
            })
            .collect()
    }

    /// 用 rustls 同款校验器（webpki，会执行 NameConstraints）验证 `leaf` 对名字 `name` 是否可信。
    fn webpki_ok(ca_der: &[u8], leaf_der: &[u8], name: &str) -> Result<(), String> {
        use rustls_pki_types::{CertificateDer, ServerName, UnixTime};
        let ca = CertificateDer::from(ca_der.to_vec());
        let anchor = webpki::anchor_from_trusted_cert(&ca).map_err(|e| format!("{e:?}"))?;
        let leaf = CertificateDer::from(leaf_der.to_vec());
        let ee = webpki::EndEntityCert::try_from(&leaf).map_err(|e| format!("{e:?}"))?;
        ee.verify_for_usage(webpki::ALL_VERIFICATION_ALGS, &[anchor], &[], UnixTime::now(), webpki::KeyUsage::server_auth(), None, None).map_err(|e| format!("{e:?}"))?;
        let sn = ServerName::try_from(name.to_string()).map_err(|e| format!("{e:?}"))?;
        ee.verify_is_valid_for_subject_name(&sn).map_err(|e| format!("{e:?}"))
    }

    fn bak_files(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| n.contains(".bak-")).collect();
        v.sort();
        v
    }

    #[test]
    fn san_permitted_matches_constraints() {
        for ok in ["shelf", "shelf.local", "SHELF.LOCAL.", "a.shelf.rm", "localhost", "remarkable.local", "10.11.99.1", "10.42.0.224", "172.16.0.1", "172.31.255.254", "192.168.1.20", "127.0.0.1"] {
            assert!(san_permitted(ok), "{ok}");
        }
        for bad in ["evil.com", "shelf.com", "notshelf.local", "books.local", "172.32.0.1", "100.64.1.1", "169.254.1.1", "8.8.8.8", "fe80::1"] {
            assert!(!san_permitted(bad), "{bad}");
        }
        // 默认 SAN（含 USB 网段 10.11.99.1）与网关默认的 mDNS 名/热点别名全部在约束内
        for s in default_sans(&["shelf.local".into(), "shelf.rm".into()]) {
            assert!(san_permitted(&s), "默认 SAN {s} 越界");
        }
    }

    #[test]
    fn new_ca_is_name_constrained_and_leaf_sans_stay_inside() {
        let t = tempfile::tempdir().unwrap();
        let extra: Vec<String> = ["192.168.1.20", "10.42.0.224", "shelf.local", "shelf.rm", "evil.com", "100.64.1.1", "books.local"].iter().map(|s| s.to_string()).collect();
        let pem = ensure_ca_signed(t.path(), &extra).unwrap();
        let ca = ca_pem(t.path()).unwrap();
        assert!(ca_has_name_constraints(&ca), "新 CA 必须带名称约束");
        let chain = ders(&pem.cert);
        let (leaf, ca_der) = (&chain[0], &ders(&ca)[0]);
        assert_eq!(&chain[1], ca_der, "链尾是 CA");
        let sans = leaf_sans(leaf);
        for must in ["10.11.99.1", "127.0.0.1", "192.168.1.20", "10.42.0.224", "shelf.local", "shelf.rm", "localhost"] {
            assert!(sans.iter().any(|s| s == must), "缺 SAN {must}: {sans:?}");
        }
        for gone in ["evil.com", "100.64.1.1", "books.local"] {
            assert!(!sans.iter().any(|s| s == gone), "越界 SAN {gone} 应被剔除");
        }
        // 每个 SAN 都要能通过真实校验器（含名称约束检查）
        for s in &sans {
            assert!(san_permitted(s), "{s}");
            webpki_ok(ca_der, leaf, s).unwrap_or_else(|e| panic!("SAN {s} 校验失败: {e}"));
        }
        // 反证：拿同一把 CA 私钥硬签一张 evil.com 的叶，校验器必须拒绝——约束真的生效
        let ca_key = KeyPair::from_pem(&std::fs::read_to_string(t.path().join("ca.key")).unwrap()).unwrap();
        let ca_p = ca_params().unwrap();
        let issuer = Issuer::from_params(&ca_p, &ca_key);
        let k = KeyPair::generate().unwrap();
        let rogue = leaf_params(vec!["evil.com".into()]).unwrap().signed_by(&k, &issuer).unwrap();
        let err = webpki_ok(ca_der, rogue.der(), "evil.com").expect_err("越界证书必须被拒");
        assert!(err.contains("NameConstraint"), "应是名称约束错误: {err}");
    }

    #[test]
    fn migrates_unconstrained_ca_and_keeps_backup() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        // 造一个旧版（无名称约束）的 CA + 叶
        let kp = KeyPair::generate().unwrap();
        let mut p = ca_params().unwrap();
        p.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        p.name_constraints = None;
        let old = p.self_signed(&kp).unwrap();
        std::fs::write(d.join("ca.pem"), old.pem()).unwrap();
        std::fs::write(d.join("ca.key"), kp.serialize_pem()).unwrap();
        std::fs::write(d.join("cert.pem"), "old-leaf").unwrap();
        std::fs::write(d.join("key.pem"), "old-leaf-key").unwrap();
        std::fs::write(d.join("cert.meta"), format!("{}\n{}\n", now_secs(), default_sans(&[]).join(","))).unwrap();
        let old_ca = std::fs::read(d.join("ca.pem")).unwrap();
        assert!(!ca_has_name_constraints(&old_ca), "按证书内容判断：旧 CA 无约束");

        let pem = ensure_ca_signed(d, &[]).unwrap();
        let new_ca = ca_pem(d).unwrap();
        assert_ne!(new_ca, old_ca, "旧 CA 被换掉");
        assert!(ca_has_name_constraints(&new_ca));
        assert_ne!(std::fs::read_to_string(d.join("ca.key")).unwrap(), kp.serialize_pem(), "CA 私钥也换新");
        assert_ne!(pem.cert, b"old-leaf", "叶证书用新 CA 重签");
        assert!(std::str::from_utf8(&pem.cert).unwrap().ends_with(std::str::from_utf8(&new_ca).unwrap()));
        // 备份：五个文件全在、内容是旧的
        let baks = bak_files(d);
        assert_eq!(baks.len(), 5, "{baks:?}");
        let find = |pfx: &str| std::fs::read(d.join(baks.iter().find(|n| n.starts_with(pfx)).unwrap())).unwrap();
        assert_eq!(find("ca.pem.bak-"), old_ca);
        assert_eq!(find("ca.key.bak-"), kp.serialize_pem().into_bytes());
        assert_eq!(find("cert.pem.bak-"), b"old-leaf");
    }

    #[test]
    fn constrained_ca_is_not_regenerated() {
        let t = tempfile::tempdir().unwrap();
        let a = ensure_ca_signed(t.path(), &["192.168.1.20".into()]).unwrap();
        let ca1 = ca_pem(t.path()).unwrap();
        let key1 = std::fs::read(t.path().join("ca.key")).unwrap();
        let b = ensure_ca_signed(t.path(), &["192.168.1.20".into()]).unwrap();
        assert_eq!(a.cert, b.cert, "叶复用");
        let _ = ensure_ca_signed(t.path(), &["192.168.1.21".into()]).unwrap(); // 换叶，不换 CA
        assert_eq!(ca_pem(t.path()).unwrap(), ca1, "已带约束的 CA 不重签");
        assert_eq!(std::fs::read(t.path().join("ca.key")).unwrap(), key1);
        assert!(bak_files(t.path()).is_empty(), "没有迁移就不产生备份");
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
