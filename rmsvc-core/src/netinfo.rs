//! 本机 IPv4 地址表（`ip -4 -o addr` 解析；busybox/iproute2 同格式）。证书 SAN、mDNS 应答选址共用。
use std::net::Ipv4Addr;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Iface {
    pub name: String,
    pub ip: Ipv4Addr,
    pub prefix: u8,
}

impl Iface {
    /// `other` 是否与本地址同一子网。
    pub fn contains(&self, other: Ipv4Addr) -> bool {
        let mask: u32 = if self.prefix == 0 { 0 } else { u32::MAX << (32 - self.prefix as u32) };
        (u32::from(self.ip) & mask) == (u32::from(other) & mask)
    }
}

pub fn parse_ip_addr_output(out: &str) -> Vec<Iface> {
    let mut v = Vec::new();
    for line in out.lines() {
        let cols: Vec<&str> = line.split_whitespace().collect();
        // "3: wlan0    inet 10.42.0.224/24 brd ..."
        let Some(pos) = cols.iter().position(|c| *c == "inet") else { continue };
        let name = cols.get(1).map(|s| s.trim_end_matches(':').to_string()).unwrap_or_default();
        let Some(cidr) = cols.get(pos + 1) else { continue };
        let (ip, prefix) = cidr.split_once('/').unwrap_or((cidr, "32"));
        let Ok(ip) = ip.parse::<Ipv4Addr>() else { continue };
        if ip.is_loopback() {
            continue;
        }
        let prefix = prefix.parse().unwrap_or(32);
        if !v.iter().any(|x: &Iface| x.ip == ip) {
            v.push(Iface { name, ip, prefix });
        }
    }
    v
}

/// 当前非环回 IPv4 地址（解析失败返回空，调用方按无地址处理）。
pub fn ipv4_ifaces() -> Vec<Iface> {
    std::process::Command::new("ip").args(["-4", "-o", "addr"]).output().map(|o| parse_ip_addr_output(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

pub fn ipv4_strings() -> Vec<String> {
    ipv4_ifaces().into_iter().map(|i| i.ip.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_and_matches_subnet() {
        let out = "1: lo    inet 127.0.0.1/8 scope host lo\\       valid_lft forever\n3: wlan0    inet 10.42.0.224/24 brd 10.42.0.255 scope global wlan0\n5: usb0    inet 10.11.99.1/24 brd 10.11.99.255 scope global usb0\n6: usb1    inet 10.11.99.1/24 scope global usb1\n";
        let v = parse_ip_addr_output(out);
        assert_eq!(v.len(), 2, "lo 排除、同 IP 去重: {v:?}");
        assert_eq!(v[0].name, "wlan0");
        assert!(v[0].contains("10.42.0.7".parse().unwrap()));
        assert!(!v[0].contains("10.11.99.2".parse().unwrap()));
        assert!(v[1].contains("10.11.99.2".parse().unwrap()));
    }
}
