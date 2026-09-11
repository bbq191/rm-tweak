//! 本机 IPv4 地址表。证书 SAN、mDNS 应答选址共用。
//! 首选**不 fork 进程**：读 `/proc/net/fib_trie`（本机地址）+ `/proc/net/route`（直连路由 → 网卡名与前缀长度）。
//! 此前每 30 秒 fork 一次 `ip -4 -o addr`（mDNS 重扫接口），2 核设备上一次 fork+exec+管道要唤醒好几个核；
//! `/proc` 读取几乎零开销。读不到/解析为空（非 Linux、`/proc` 缺失）才回落到 `ip -4 -o addr`（busybox/iproute2 同格式）。
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

/// `/proc/net/fib_trie`：一行 `|-- a.b.c.d` 后面紧跟 `/32 host LOCAL` 的就是本机地址（Main/Local 两张表都会出现，去重）。
fn parse_fib_trie_locals(text: &str) -> Vec<Ipv4Addr> {
    let mut out: Vec<Ipv4Addr> = Vec::new();
    let mut last: Option<Ipv4Addr> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(ip) = t.strip_prefix("|--") {
            last = ip.trim().parse().ok();
        } else if t.starts_with("/32 host LOCAL") {
            if let Some(ip) = last.take() {
                if !ip.is_loopback() && !out.contains(&ip) {
                    out.push(ip);
                }
            }
        }
    }
    out
}

/// `/proc/net/route` 里的直连路由：(网卡名, 网络地址, 前缀长度)。地址是内核按主机字节序打印的 hex，
/// 这里按同一字节序还原；默认路由（掩码 0）跳过。
fn parse_route_networks(text: &str) -> Vec<(String, Ipv4Addr, u8)> {
    let mut v = Vec::new();
    for line in text.lines().skip(1) {
        let c: Vec<&str> = line.split_whitespace().collect();
        if c.len() < 8 {
            continue;
        }
        let (Ok(dest), Ok(mask)) = (u32::from_str_radix(c[1], 16), u32::from_str_radix(c[7], 16)) else { continue };
        if mask == 0 {
            continue;
        }
        v.push((c[0].to_string(), Ipv4Addr::from(dest.to_ne_bytes()), Ipv4Addr::from(mask.to_ne_bytes()).octets().iter().map(|b| b.count_ones() as u8).sum()));
    }
    v
}

/// 由两份 `/proc/net` 文本合成地址表：每个本机地址取包含它的最长前缀直连路由定网卡名/前缀；
/// 没有直连路由的（如挂在 lo 上的别名 IP）按 `/32`、无网卡名处理。
fn parse_proc_net(fib_trie: &str, route: &str) -> Vec<Iface> {
    let nets = parse_route_networks(route);
    parse_fib_trie_locals(fib_trie)
        .into_iter()
        .map(|ip| {
            let best = nets
                .iter()
                .filter(|(_, net, prefix)| Iface { name: String::new(), ip: *net, prefix: *prefix }.contains(ip))
                .max_by_key(|(_, _, prefix)| *prefix);
            match best {
                Some((name, _, prefix)) => Iface { name: name.clone(), ip, prefix: *prefix },
                None => Iface { name: String::new(), ip, prefix: 32 },
            }
        })
        .collect()
}

/// 当前非环回 IPv4 地址（解析失败返回空，调用方按无地址处理）。
pub fn ipv4_ifaces() -> Vec<Iface> {
    if let (Ok(fib), Ok(route)) = (std::fs::read_to_string("/proc/net/fib_trie"), std::fs::read_to_string("/proc/net/route")) {
        let v = parse_proc_net(&fib, &route);
        if !v.is_empty() {
            return v;
        }
    }
    ipv4_ifaces_via_ip()
}

/// 回落：`ip -4 -o addr`（会 fork，仅 `/proc` 不可用时用）。
fn ipv4_ifaces_via_ip() -> Vec<Iface> {
    std::process::Command::new("ip").args(["-4", "-o", "addr"]).output().map(|o| parse_ip_addr_output(&String::from_utf8_lossy(&o.stdout))).unwrap_or_default()
}

pub fn ipv4_strings() -> Vec<String> {
    ipv4_ifaces().into_iter().map(|i| i.ip.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parses_proc_net_fib_trie_and_route_without_forking() {
        let fib = "Main:\n  +-- 0.0.0.0/0 3 0 5\n     |-- 0.0.0.0\n        /0 universe UNICAST\n     +-- 10.11.99.0/24 2 0 2\n        |-- 10.11.99.0\n           /32 link BROADCAST\n        |-- 10.11.99.1\n           /32 host LOCAL\n     +-- 10.42.0.0/24 2 0 2\n        |-- 10.42.0.224\n           /32 host LOCAL\n     +-- 127.0.0.0/8 2 0 2\n        |-- 127.0.0.1\n           /32 host LOCAL\nLocal:\n     |-- 10.11.99.1\n        /32 host LOCAL\n";
        // 内核按主机字节序（小端）打印：10.11.99.0 → 00630B0A，掩码 255.255.255.0 → 00FFFFFF
        let route = "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\nwlan0\t00000000\t0100000A\t0003\t0\t0\t600\t00000000\t0\t0\t0\nwlan0\t00002A0A\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0\nusb0\t00630B0A\t00000000\t0001\t0\t0\t100\t00FFFFFF\t0\t0\t0\n";
        let expected_le = cfg!(target_endian = "little");
        let v = parse_proc_net(fib, route);
        assert_eq!(v.len(), 2, "loopback 排除、Local 表重复项去重: {v:?}");
        if expected_le {
            let usb = v.iter().find(|i| i.ip == "10.11.99.1".parse::<Ipv4Addr>().unwrap()).unwrap();
            assert_eq!((usb.name.as_str(), usb.prefix), ("usb0", 24));
            let wl = v.iter().find(|i| i.ip == "10.42.0.224".parse::<Ipv4Addr>().unwrap()).unwrap();
            assert_eq!((wl.name.as_str(), wl.prefix), ("wlan0", 24));
        }
        // 没有对应直连路由的地址（如 lo 上的别名）按 /32、无网卡名
        let v = parse_proc_net(fib, "Iface\tDestination\tGateway\tFlags\tRefCnt\tUse\tMetric\tMask\tMTU\tWindow\tIRTT\n");
        assert!(v.iter().all(|i| i.prefix == 32 && i.name.is_empty()));
    }

    #[test]
    fn ipv4_ifaces_runs_on_this_host_without_panicking() {
        // 宿主机上 /proc 存在，不 fork；只要不 panic、不返回环回
        assert!(ipv4_ifaces().iter().all(|i| !i.ip.is_loopback()));
    }

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
