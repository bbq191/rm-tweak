//! 极简 mDNS 应答器：让浏览器用 `https://shelf.local/` 而非记 IP（伪域名，零路由器/DNS 配置；
//! 2026-09-10 改绑标准 443 端口后，连端口号都不用带了）。
//! 只回答本机名字的 A 查询（`<name>.local`），应答地址选**与提问者同子网**的本机 IPv4（USB 网段问就答 10.11.99.1，
//! WiFi 网段问就答 WiFi 地址）。iOS/macOS/Windows 10+/Linux(avahi 或 systemd-resolved) 都能解析 .local；
//! **Android 系统解析器不查 mDNS**，安卓浏览器要走 host 热点的 dnsmasq 别名（见 shelf/docs §03j）。
//! 设备上没有 avahi，5353 端口空闲；若绑定失败（别的 mDNS 服务在跑）只打日志、功能退化，不影响网关。
//!
//! **接口重扫是事件驱动的**（2026-09-25）：订阅内核 netlink 的 IPv4 地址变化（`NETLINK_ROUTE` +
//! `RTMGRP_IPV4_IFADDR`），和 5353 套接字一起 `poll`，只有收到 `RTM_NEWADDR`/`RTM_DELADDR` 才重扫——空闲时
//! 零定时唤醒（此前每 60 秒醒一次），WiFi 后连/换网拿到地址后立刻加入多播组、被应答（此前最迟 60 秒）。
//! netlink 打不开（理论上设备内核一定支持，只为稳健）才退回原来的 60 秒读超时顺带重扫，并打一行日志。
use crate::netinfo::{ipv4_ifaces, Iface};
use socket2::{Domain, Protocol, Socket, Type};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::time::Duration;

const GROUP: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 251);
const PORT: u16 = 5353;
const TTL: u32 = 120;
/// **退路用**的接口重扫间隔 = socket 读超时（只在 netlink 打不开时生效）。历史：最早读超时 5 秒 + 每 30 秒 fork
/// 一次 `ip`；09-20 合并成 60 秒、只在读超时那次（或收到包但距上次重扫已过一个间隔）顺带重扫，重扫改读 `/proc`
/// （见 `netinfo`）不 fork；09-25 起正常路径改由 netlink 地址事件驱动，这个间隔只剩兜底意义。
const RESCAN_INTERVAL: Duration = Duration::from_secs(60);

// netlink 常量（内核 ABI，固定值；libc 没导出 RTMGRP_* 组掩码，这里统一本地定义）。
const RTMGRP_IPV4_IFADDR: u32 = 0x10;
const RTM_NEWADDR: u16 = 20;
const RTM_DELADDR: u16 = 21;
/// `struct nlmsghdr` 长度：len u32 + type u16 + flags u16 + seq u32 + pid u32。
const NLMSG_HDRLEN: usize = 16;

/// 一段 netlink 读出的字节里有没有 IPv4 地址增删（`RTM_NEWADDR`/`RTM_DELADDR`）——有就该重扫接口。
/// 按 `nlmsghdr` 逐条走（长度 4 字节对齐）；地址族不是 IPv4 的（只订阅了 IPv4 组，理论上不会来）不算；
/// 长度字段不合法就停（截断/畸形报文不 panic、不越界）。头字段是本机字节序（netlink 约定）。
pub fn addr_change_in(buf: &[u8]) -> bool {
    let mut off = 0usize;
    while off + NLMSG_HDRLEN <= buf.len() {
        let len = u32::from_ne_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]) as usize;
        let ty = u16::from_ne_bytes([buf[off + 4], buf[off + 5]]);
        if len < NLMSG_HDRLEN || len > buf.len() - off {
            break;
        }
        if ty == RTM_NEWADDR || ty == RTM_DELADDR {
            // `struct ifaddrmsg` 紧跟在头后，首字节是 ifa_family；缺了就保守当作变化。
            match buf.get(off + NLMSG_HDRLEN) {
                Some(&fam) if i32::from(fam) != libc::AF_INET => {}
                _ => return true,
            }
        }
        off += (len + 3) & !3;
    }
    false
}

/// 内核 IPv4 地址变化订阅（`NETLINK_ROUTE` 套接字，非阻塞）。
struct AddrWatch {
    fd: OwnedFd,
}

impl AddrWatch {
    fn open() -> Result<AddrWatch, String> {
        // SAFETY: 普通系统调用；返回值检查后立刻交给 OwnedFd 管理生命周期。
        let raw = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, libc::NETLINK_ROUTE) };
        if raw < 0 {
            return Err(format!("socket(AF_NETLINK): {}", std::io::Error::last_os_error()));
        }
        // SAFETY: raw 是刚创建成功、未被别处持有的描述符。
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: sockaddr_nl 全零是合法初值（nl_pid=0 让内核分配端口号）。
        let mut sa: libc::sockaddr_nl = unsafe { std::mem::zeroed() };
        sa.nl_family = libc::AF_NETLINK as libc::sa_family_t;
        sa.nl_groups = RTMGRP_IPV4_IFADDR;
        // SAFETY: 传入的指针/长度对应上面这个栈上的 sockaddr_nl。
        let rc = unsafe { libc::bind(fd.as_raw_fd(), &sa as *const libc::sockaddr_nl as *const libc::sockaddr, std::mem::size_of::<libc::sockaddr_nl>() as libc::socklen_t) };
        if rc < 0 {
            return Err(format!("bind(RTMGRP_IPV4_IFADDR): {}", std::io::Error::last_os_error()));
        }
        Ok(AddrWatch { fd })
    }

    /// 读空当前积压的通知，返回其中有没有地址增删。接收缓冲溢出（`ENOBUFS`＝丢过通知）也当作有变化，宁可多扫一次。
    fn drain(&self) -> bool {
        let mut buf = [0u8; 8192];
        let mut changed = false;
        loop {
            // SAFETY: buf 是本函数栈上的可写缓冲，长度如实传入。
            let n = unsafe { libc::recv(self.fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), libc::MSG_DONTWAIT) };
            if n > 0 {
                changed |= addr_change_in(&buf[..n as usize]);
                continue;
            }
            if n == 0 {
                return changed;
            }
            match std::io::Error::last_os_error().raw_os_error() {
                Some(libc::EINTR) => continue,
                Some(libc::ENOBUFS) => changed = true,
                _ => return changed, // EAGAIN（读空了）或别的错：本轮到此为止
            }
        }
    }
}

/// `poll` 一组描述符的"可读"位；`timeout_ms` 为 -1 表示无限等。`EINTR` 返回全 false（调用方重进循环即可）。
fn poll_readable(fds: &[RawFd], timeout_ms: i32) -> Vec<bool> {
    let mut pfds: Vec<libc::pollfd> = fds.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }).collect();
    // SAFETY: pfds 是长度如实的可写 pollfd 数组。
    let rc = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, timeout_ms) };
    if rc <= 0 {
        return vec![false; fds.len()];
    }
    // POLLERR/POLLHUP 也当"可读"交给读调用去取错误，别让它空转。
    pfds.iter().map(|p| p.revents & (libc::POLLIN | libc::POLLERR | libc::POLLHUP) != 0).collect()
}

/// 一次重扫后更新已加入多播组的地址表：已经不在本机的地址先移出（地址删了又回来——WiFi 断开重连拿到同一个
/// IP——才会被重新 join）；`join` 成功或"已经是成员"（`EADDRINUSE`）都算已加入。
fn update_joined(ifaces: &[Iface], joined: &mut Vec<Ipv4Addr>, mut join: impl FnMut(Ipv4Addr) -> std::io::Result<()>) {
    joined.retain(|ip| ifaces.iter().any(|i| i.ip == *ip));
    for i in ifaces {
        if joined.contains(&i.ip) {
            continue;
        }
        match join(i.ip) {
            Ok(()) => joined.push(i.ip),
            Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => joined.push(i.ip),
            Err(_) => {}
        }
    }
}

/// 解析一个 DNS 报文里的问题名（第一段 label 序列，不支持压缩指针——mDNS 查询极少用）。
/// 返回 (names, qtypes) 对列表。
pub fn parse_questions(pkt: &[u8]) -> Vec<(String, u16)> {
    let mut out = Vec::new();
    if pkt.len() < 12 {
        return out;
    }
    let flags = u16::from_be_bytes([pkt[2], pkt[3]]);
    if flags & 0x8000 != 0 {
        return out; // 是应答不是查询
    }
    let qd = u16::from_be_bytes([pkt[4], pkt[5]]) as usize;
    let mut i = 12;
    for _ in 0..qd {
        let mut labels = Vec::new();
        loop {
            let Some(&len) = pkt.get(i) else { return out };
            i += 1;
            if len == 0 {
                break;
            }
            if len & 0xC0 == 0xC0 {
                i += 1; // 压缩指针：跳过并结束该名字（不解引用）
                break;
            }
            let end = i + len as usize;
            let Some(part) = pkt.get(i..end) else { return out };
            labels.push(String::from_utf8_lossy(part).to_string());
            i = end;
        }
        let Some(q) = pkt.get(i..i + 4) else { return out };
        let qtype = u16::from_be_bytes([q[0], q[1]]);
        i += 4;
        out.push((labels.join(".").to_ascii_lowercase(), qtype));
    }
    out
}

/// 构造 A 记录应答（QR|AA，id 0，cache-flush 位）。
pub fn build_answer(name: &str, ip: Ipv4Addr) -> Vec<u8> {
    let mut p = vec![0, 0, 0x84, 0x00, 0, 0, 0, 1, 0, 0, 0, 0];
    for label in name.split('.') {
        p.push(label.len() as u8);
        p.extend_from_slice(label.as_bytes());
    }
    p.push(0);
    p.extend_from_slice(&[0, 1, 0x80, 0x01]); // A, IN + cache-flush
    p.extend_from_slice(&TTL.to_be_bytes());
    p.extend_from_slice(&[0, 4]);
    p.extend_from_slice(&ip.octets());
    p
}

/// 选与提问者同子网的本机地址。
pub fn pick_ip(ifaces: &[Iface], from: Ipv4Addr) -> Option<Ipv4Addr> {
    ifaces.iter().find(|i| i.contains(from)).map(|i| i.ip)
}

fn open_socket() -> Result<Socket, String> {
    let s = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP)).map_err(|e| e.to_string())?;
    s.set_reuse_address(true).map_err(|e| e.to_string())?;
    #[cfg(all(unix, not(target_os = "solaris"), not(target_os = "illumos")))]
    let _ = s.set_reuse_port(true);
    s.bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, PORT)).into()).map_err(|e| format!("绑定 udp/{PORT}: {e}"))?;
    let _ = s.set_multicast_loop_v4(false);
    let _ = s.set_multicast_ttl_v4(255);
    s.set_read_timeout(Some(RESCAN_INTERVAL)).map_err(|e| e.to_string())?;
    Ok(s)
}

/// 阻塞跑应答器（放线程里）。`names` 不带 `.local`。接口在内核报 IPv4 地址增删时重扫（WiFi 后连也能答）；
/// netlink 不可用时退回每 [`RESCAN_INTERVAL`]（60s）重扫。
pub fn serve(names: Vec<String>) -> Result<(), String> {
    let raw = open_socket()?;
    let sock: std::net::UdpSocket = raw.try_clone().map_err(|e| e.to_string())?.into();
    let wanted: Vec<String> = names.iter().map(|n| format!("{}.local", n.trim().to_ascii_lowercase())).filter(|n| n != ".local").collect();
    if wanted.is_empty() {
        return Err("无 mDNS 名字".into());
    }
    // 先订阅再做首次扫描：两者之间来的地址变化也不会漏。
    let watch = match AddrWatch::open() {
        Ok(w) => Some(w),
        Err(e) => {
            eprintln!("[mdns] 监听内核地址变化失败（{e}），退回每 {} 秒重扫一次接口", RESCAN_INTERVAL.as_secs());
            None
        }
    };
    let mut ifaces: Vec<Iface> = Vec::new();
    let mut joined: Vec<Ipv4Addr> = Vec::new();
    let mut last_scan: Option<std::time::Instant> = None;
    let mut buf = [0u8; 1500];
    let rescan = |ifaces: &mut Vec<Iface>, joined: &mut Vec<Ipv4Addr>| {
        *ifaces = ipv4_ifaces();
        update_joined(ifaces, joined, |ip| sock.join_multicast_v4(&GROUP, &ip));
    };
    loop {
        // 启动时扫一次；之后 netlink 路径只在地址增删时重扫，退路只在"读超时"或"收到包但已过一个重扫间隔"时重扫。
        if last_scan.is_none() {
            rescan(&mut ifaces, &mut joined);
            last_scan = Some(std::time::Instant::now());
        }
        if let Some(w) = &watch {
            // 两个描述符一起无限期等：空闲时不醒。地址变化先处理（重扫在前），同一轮里到的查询就能按新地址答。
            let ready = poll_readable(&[sock.as_raw_fd(), w.fd.as_raw_fd()], -1);
            if ready[1] && w.drain() {
                rescan(&mut ifaces, &mut joined);
                last_scan = Some(std::time::Instant::now());
            }
            if !ready[0] {
                continue;
            }
        }
        let (n, from) = match sock.recv_from(&mut buf) {
            Ok(x) => {
                if watch.is_none() && last_scan.is_some_and(|t| t.elapsed() >= RESCAN_INTERVAL) {
                    rescan(&mut ifaces, &mut joined);
                    last_scan = Some(std::time::Instant::now());
                }
                x
            }
            // netlink 路径：poll 报可读但 recv 读超时（校验和错的包被内核丢掉时会这样），不是该重扫的信号，回去接着等。
            Err(e) if watch.is_some() && matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
            // 退路：超时顺带重扫（WiFi 后连也能答）；EINTR（Interrupted，如进程 spawn 子进程时 SIGCHLD 打断阻塞 recv）只是重试，别刷屏
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {
                rescan(&mut ifaces, &mut joined);
                last_scan = Some(std::time::Instant::now());
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                std::thread::sleep(Duration::from_secs(1));
                eprintln!("[mdns] recv: {e}");
                continue;
            }
        };
        let SocketAddr::V4(from4) = from else { continue };
        for (qname, qtype) in parse_questions(&buf[..n]) {
            if !(qtype == 1 || qtype == 255) || !wanted.contains(&qname) {
                continue;
            }
            let Some(ip) = pick_ip(&ifaces, *from4.ip()) else { continue };
            let pkt = build_answer(&qname, ip);
            let _ = raw.set_multicast_if_v4(&ip);
            let _ = sock.send_to(&pkt, SocketAddrV4::new(GROUP, PORT));
            if from4.port() != PORT {
                let _ = sock.send_to(&pkt, from4); // 单播提问（QU/传统解析器）也单播答一份
            }
        }
    }
}

/// 后台线程启动；失败只打日志。
pub fn spawn(names: Vec<String>) {
    std::thread::Builder::new().name("mdns".into()).spawn(move || {
        if let Err(e) = serve(names) {
            eprintln!("[mdns] 未启用: {e}");
        }
    }).ok();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn query_roundtrip() {
        // 手工拼一个查询：id 0x1234, flags 0, qd=1, "shelf.local" A IN
        let mut q = vec![0x12, 0x34, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        for l in ["shelf", "local"] {
            q.push(l.len() as u8);
            q.extend_from_slice(l.as_bytes());
        }
        q.extend_from_slice(&[0, 0, 1, 0, 1]);
        assert_eq!(parse_questions(&q), vec![("shelf.local".into(), 1)]);
        let a = build_answer("shelf.local", Ipv4Addr::new(10, 42, 0, 224));
        assert_eq!(&a[2..4], &[0x84, 0]);
        assert_eq!(&a[a.len() - 4..], &[10, 42, 0, 224]);
        assert!(parse_questions(&a).is_empty(), "应答不当查询");
        assert!(parse_questions(&q[..5]).is_empty(), "截断不 panic");
        let ifs = vec![Iface { name: "wlan0".into(), ip: "10.42.0.224".parse().unwrap(), prefix: 24 }, Iface { name: "usb0".into(), ip: "10.11.99.1".parse().unwrap(), prefix: 24 }];
        assert_eq!(pick_ip(&ifs, "10.11.99.2".parse().unwrap()), Some("10.11.99.1".parse().unwrap()));
        assert_eq!(pick_ip(&ifs, "192.168.1.5".parse().unwrap()), None);
    }

    /// 拼一条 netlink 消息：nlmsghdr（本机字节序）+ 载荷，按 4 字节对齐补零。
    fn nlmsg(ty: u16, payload: &[u8]) -> Vec<u8> {
        let len = (NLMSG_HDRLEN + payload.len()) as u32;
        let mut m = Vec::new();
        m.extend_from_slice(&len.to_ne_bytes());
        m.extend_from_slice(&ty.to_ne_bytes());
        m.extend_from_slice(&0u16.to_ne_bytes()); // flags
        m.extend_from_slice(&0u32.to_ne_bytes()); // seq
        m.extend_from_slice(&0u32.to_ne_bytes()); // pid
        m.extend_from_slice(payload);
        while m.len() % 4 != 0 {
            m.push(0);
        }
        m
    }

    /// ifaddrmsg：family, prefixlen, flags, scope, index(u32)。
    fn ifaddr(family: u8) -> Vec<u8> {
        let mut p = vec![family, 24, 0, 0];
        p.extend_from_slice(&3u32.to_ne_bytes());
        p
    }

    #[test]
    fn netlink_addr_change_detection() {
        let inet = libc::AF_INET as u8;
        assert!(addr_change_in(&nlmsg(RTM_NEWADDR, &ifaddr(inet))), "新增 IPv4 地址");
        assert!(addr_change_in(&nlmsg(RTM_DELADDR, &ifaddr(inet))), "删除 IPv4 地址");
        assert!(!addr_change_in(&nlmsg(RTM_NEWADDR, &ifaddr(libc::AF_INET6 as u8))), "IPv6 地址不算");
        assert!(!addr_change_in(&nlmsg(16, &[0; 16])), "RTM_NEWLINK 等别的类型不算");
        assert!(!addr_change_in(&[]), "空");
        // 一次读出多条：前面无关、后面才是地址变化，要能走到（含奇数长度载荷的对齐）
        let mut multi = nlmsg(16, &[0; 5]);
        multi.extend(nlmsg(RTM_NEWADDR, &ifaddr(inet)));
        assert!(addr_change_in(&multi));
        // 载荷缺 ifaddrmsg：保守当作变化
        assert!(addr_change_in(&nlmsg(RTM_DELADDR, &[])));
    }

    #[test]
    fn netlink_parse_survives_malformed_input() {
        let good = nlmsg(RTM_NEWADDR, &ifaddr(libc::AF_INET as u8));
        assert!(!addr_change_in(&good[..NLMSG_HDRLEN - 1]), "头不完整");
        let mut huge = good.clone();
        huge[..4].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(!addr_change_in(&huge), "长度字段超出缓冲：停下，不越界");
        let mut tiny = good.clone();
        tiny[..4].copy_from_slice(&3u32.to_ne_bytes());
        assert!(!addr_change_in(&tiny), "长度小于头：停下，不死循环");
    }

    #[test]
    fn update_joined_rejoins_after_address_comes_back() {
        let wlan = Iface { name: "wlan0".into(), ip: "10.42.0.224".parse().unwrap(), prefix: 24 };
        let usb = Iface { name: "usb0".into(), ip: "10.11.99.1".parse().unwrap(), prefix: 24 };
        let mut joined = Vec::new();
        let mut calls = Vec::new();
        update_joined(&[usb.clone(), wlan.clone()], &mut joined, |ip| { calls.push(ip); Ok(()) });
        assert_eq!(joined, vec![usb.ip, wlan.ip]);
        // WiFi 断开：地址消失 → 移出已加入表
        update_joined(std::slice::from_ref(&usb), &mut joined, |_| panic!("usb0 已加入，不该重复 join"));
        assert_eq!(joined, vec![usb.ip]);
        // 重连拿到同一个 IP：重新 join；内核说"已是成员"也算加入
        update_joined(&[usb.clone(), wlan.clone()], &mut joined, |_| Err(std::io::Error::from(std::io::ErrorKind::AddrInUse)));
        assert_eq!(joined, vec![usb.ip, wlan.ip]);
        // 别的失败：不记为已加入，下次地址事件再试
        let mut j2 = Vec::new();
        update_joined(std::slice::from_ref(&wlan), &mut j2, |_| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)));
        assert!(j2.is_empty());
        assert_eq!(calls.len(), 2);
    }

    /// 真 netlink：host 上一般能开（不需要特权）；受限沙箱里开不了就跳过，不算失败。开得了就验证 drain 非阻塞、
    /// poll 按超时返回（不挂起）。
    #[test]
    fn real_netlink_watch_is_nonblocking_when_available() {
        let w = match AddrWatch::open() {
            Ok(w) => w,
            Err(e) => {
                eprintln!("跳过：本环境开不了 netlink（{e}）");
                return;
            }
        };
        let t = std::time::Instant::now();
        let _ = w.drain();
        let ready = poll_readable(&[w.fd.as_raw_fd()], 10);
        assert_eq!(ready.len(), 1);
        assert!(t.elapsed() < Duration::from_secs(5), "drain/poll 不该阻塞");
    }

    /// 端到端：真的加/删一个地址，netlink 要报出来。需要能改网络的环境，默认忽略；手动跑法（无需 root，
    /// 在一次性的用户+网络命名空间里改，不碰宿主网络）：
    /// `unshare -rn sh -c 'ip link set lo up; cargo test --lib mdns::tests::real_netlink_reports_addr_add_and_del -- --ignored'`
    #[test]
    #[ignore]
    fn real_netlink_reports_addr_add_and_del() {
        let w = AddrWatch::open().expect("netlink 应能打开");
        w.drain();
        let ip = |args: &[&str]| assert!(std::process::Command::new("ip").args(args).status().unwrap().success(), "ip {args:?}");
        ip(&["addr", "add", "10.254.0.1/24", "dev", "lo"]);
        assert!(poll_readable(&[w.fd.as_raw_fd()], 2000)[0], "加地址后 netlink 应可读");
        assert!(w.drain(), "RTM_NEWADDR 应被识别");
        ip(&["addr", "del", "10.254.0.1/24", "dev", "lo"]);
        assert!(poll_readable(&[w.fd.as_raw_fd()], 2000)[0]);
        assert!(w.drain(), "RTM_DELADDR 应被识别");
        assert!(!poll_readable(&[w.fd.as_raw_fd()], 50)[0], "没有变化时不该醒");
    }
}
