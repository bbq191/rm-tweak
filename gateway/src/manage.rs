//! 服务管理台（Track 2）：**单一模块目录表** [`MODULES`] 是唯一事实源——URL 段↔服务名↔install `--only` 令牌↔显示名
//! 都从它派生（`service_of`、代理路由、管理台三态都复用它，不再各处硬编码）。
//!
//! 三态（用户 2026-09-04 定）：**未装**（二进制不在→引导安装）/ **已装未开**（二进制在、服务没跑→可开启）/
//! **已开**（跑着→网页有该功能）。开关＝`systemctl start/stop`（仅关后台省占用，非省电）；卸载＝调已装的
//! `shelf-uninstall --only <令牌>`（对称删单元/二进制/qmd）；**安装不走网页**（不让网页 remount /usr 装系统单元），
//! 未装模块只给引导。网关自身永远在（管理台宿主），不可从网页关/卸。基石（xovi/appload/qrr/KOReader）只读探测。
use rmsvc_core::http::{ApiError, ApiResult, Reply, Request};
use rmsvc_core::paths::Paths;
use rmsvc_core::registry;
use std::time::{Duration, Instant};

/// 一个可管理的领域模块（网关自身不在此列）。
pub struct Module {
    /// URL 段（`/api/<seg>/*` 反向代理 + 管理路由）。
    pub seg: &'static str,
    /// systemd 单元 / 注册名。
    pub service: &'static str,
    /// `install.sh`/`uninstall.sh` 的 `--only` 令牌。
    pub only: &'static str,
    pub label: &'static str,
    /// 门控未上线 → 不可装、不可开（当前全为 true；机制保留给将来的新模块）。
    pub installable: bool,
    /// 该服务是否提供 `GET /events`（SSE）。网关只给提供的服务起订阅线程：mind-serve 是纯被动的
    /// 问答服务（没有事件流，见其 main.rs 头注），此前网关对它每 3 秒打一个 404、白白唤醒它。
    pub events: bool,
}

pub const MODULES: &[Module] = &[
    Module { seg: "books", service: "book-serve", only: "book", label: "母版库 / 落原生", installable: true, events: true },
    Module { seg: "fonts", service: "font-serve", only: "font", label: "xochitl 字体", installable: true, events: true },
    Module { seg: "koreader", service: "koreader-serve", only: "koreader", label: "KOReader", installable: true, events: true },
    Module { seg: "wallpapers", service: "wallpaper-serve", only: "wallpaper", label: "壁纸", installable: true, events: true },
    // 笔记线（notes/）：矿 / 转写 / 脑 / 本，挂同一网关；网页只有 note-serve 注册「笔记」tab，前端组合四个 seg。
    Module { seg: "ink", service: "ink-serve", only: "ink", label: "笔记·矿（条目库）", installable: true, events: true },
    Module { seg: "transcribe", service: "transcribe-serve", only: "transcribe", label: "笔记·转写（手写→文字）", installable: true, events: true },
    Module { seg: "mind", service: "mind-serve", only: "mind", label: "笔记·脑（问AI）", installable: true, events: false },
    Module { seg: "notes", service: "note-serve", only: "note", label: "笔记·本（笔记本/导出）", installable: true, events: true },
];

pub fn by_seg(seg: &str) -> Option<&'static Module> {
    MODULES.iter().find(|m| m.seg == seg)
}

/// URL 段 → 服务名（反向代理用；从目录表派生，单一事实源）。
pub fn service_of(seg: &str) -> Option<&'static str> {
    by_seg(seg).map(|m| m.service)
}

/// 外部命令默认超时。`systemctl start/stop` 正常几十毫秒到几秒；给 30 秒是为了容纳慢启动服务，
/// 又不至于在 systemd 卡死（2026-08-29 cgroup/RCU 事故那类）时让请求线程无限堆积。
const RUN_TIMEOUT: Duration = Duration::from_secs(30);
/// 卸载脚本要 stop 单元、删二进制/qmd，宽一些。
const UNINSTALL_TIMEOUT: Duration = Duration::from_secs(180);

/// `pub(crate)`：`enhance::battop` 复用同一套 systemctl 调用（避免重新实现一遍 `Command` 样板）。
/// 带 [`RUN_TIMEOUT`] 超时，超时会 kill 子进程并报错。
pub(crate) fn run(cmd: &str, args: &[&str]) -> Result<String, String> {
    run_timeout(cmd, args, RUN_TIMEOUT)
}

/// 起子进程等它结束，最多等 `timeout`。stdout/stderr 各用一条线程排空（否则输出超过管道缓冲会把子进程写阻塞，
/// 被误判成超时）；超时 kill 并返回错误，不 join 读线程（孙进程可能还握着管道，别被它拖住）。
pub(crate) fn run_timeout(cmd: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    use std::io::Read;
    use std::process::Stdio;
    let mut child = std::process::Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| format!("{cmd}: {e}"))?;
    let drain = |mut r: Box<dyn Read + Send>| std::thread::spawn(move || {
        let mut v = Vec::new();
        let _ = r.read_to_end(&mut v);
        v
    });
    let out_t = child.stdout.take().map(|s| drain(Box::new(s)));
    let err_t = child.stderr.take().map(|s| drain(Box::new(s)));
    let deadline = Instant::now() + timeout;
    // 轮询间隔 20ms 起、逐步翻倍到 200ms：短命令（systemctl is-active 几十毫秒）响应不变慢，
    // 慢命令（卸载脚本最长 180 秒）不再以 50 次/秒的频率白白唤醒。
    let mut poll = Duration::from_millis(20);
    let status = loop {
        match child.try_wait().map_err(|e| format!("{cmd}: {e}"))? {
            Some(st) => break st,
            None if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("{cmd} 超过 {} 秒未结束，已终止", timeout.as_secs()));
            }
            None => {
                std::thread::sleep(poll);
                poll = (poll * 2).min(Duration::from_millis(200));
            }
        }
    };
    let text = |t: Option<std::thread::JoinHandle<Vec<u8>>>| t.and_then(|h| h.join().ok()).map(|v| String::from_utf8_lossy(&v).trim().to_string()).unwrap_or_default();
    let (out, err) = (text(out_t), text(err_t));
    if status.success() {
        Ok(out)
    } else {
        Err(err)
    }
}

fn installed(paths: &Paths, m: &Module) -> bool {
    paths.bin_dir().join(m.service).is_file()
}

/// 运行中：复用注册表（服务起时写、退/死时清），无需 shell。
fn running(reg: &[registry::ServiceInfo], m: &Module) -> bool {
    reg.iter().any(|s| s.name == m.service)
}

/// `GET /api/manage`：每模块三态。
pub fn status(paths: &Paths) -> Reply {
    let reg = registry::list(paths);
    let modules: Vec<serde_json::Value> = MODULES
        .iter()
        .map(|m| {
            let inst = installed(paths, m);
            serde_json::json!({
                "seg": m.seg, "service": m.service, "only": m.only, "label": m.label,
                "installable": m.installable,
                "installed": inst,
                "running": running(&reg, m),
                "hasWeb": inst && m.installable,
            })
        })
        .collect();
    Reply::ok(&serde_json::json!({ "modules": modules, "gateway": {"running": true} }))
}

/// `GET /api/foundation`：基石（xovi 栈 + KOReader + WeRead）只读探测——引导页据此显示红绿 + 官方链接。
/// WeRead 不是本项目服务（不在 [`MODULES`] 里、没有 `-serve` 后端），是外部发行包自带 `install.sh`
/// 直接 SSH 装到设备的第三方 app（跟 2026-09-05 已砍的旧微读管线无关），这里只探测装没装。
pub fn foundation(paths: &Paths) -> Reply {
    let home = paths.home();
    let xovi = home.join("xovi");
    let exists = |p: std::path::PathBuf| p.exists();
    Reply::ok(&serde_json::json!({
        "xovi": exists(xovi.join("xovi.so")) || exists(xovi.join("start")),
        "appload": exists(xovi.join("exthome/appload")),
        "qrr": exists(xovi.join("exthome/qt-resource-rebuilder")),
        "koreader": exists(paths.koreader_root().join("reader.lua")) || exists(paths.koreader_root().to_path_buf()),
        "weread": exists(paths.weread_root().join("bin/start-remarkable-weread.sh")) || exists(paths.weread_root().to_path_buf()),
    }))
}

/// `POST /api/manage/{seg}/{start|stop}`：仅开关后台服务（省占用/隐藏功能，非省电）。网关不可关。
pub fn toggle(_paths: &Paths, seg: &str, action: &str) -> ApiResult {
    let m = by_seg(seg).ok_or_else(|| ApiError::bad(format!("未知模块 {seg}")))?;
    if !m.installable {
        return Err(ApiError::bad(format!("{} 未上线，不可开关", m.label)));
    }
    let unit = format!("{}.service", m.service);
    match action {
        "start" => run("systemctl", &["start", &unit]).map_err(ApiError::internal)?,
        "stop" => run("systemctl", &["stop", &unit]).map_err(ApiError::internal)?,
        _ => return Err(ApiError::bad("action 只能 start|stop")),
    };
    Ok(Reply::ok(&serde_json::json!({"ok": true, "service": m.service, "action": action})))
}

/// `POST /api/manage/{seg}/uninstall`：调已装的 `shelf-uninstall --only <令牌>`（对称删单元/二进制/qmd）。
/// 安装不走网页（见文件头）。
pub fn uninstall(paths: &Paths, seg: &str, req: &mut Request<'_>) -> ApiResult {
    let m = by_seg(seg).ok_or_else(|| ApiError::bad(format!("未知模块 {seg}")))?;
    let _ = req.read_small_body(); // 排空 body
    let script = paths.bin_dir().join("shelf-uninstall");
    if !script.is_file() {
        return Err(ApiError::bad("设备上没有 shelf-uninstall（重装一次 shelf 会装上它），网页卸载不可用；可 SSH 跑 uninstall.sh --only"));
    }
    let out = run_timeout("sh", &[&script.to_string_lossy(), "--only", m.only], UNINSTALL_TIMEOUT).map_err(ApiError::internal)?;
    Ok(Reply::ok(&serde_json::json!({"ok": true, "service": m.service, "log": out})))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn run_timeout_captures_output_kills_hung_child_and_survives_big_output() {
        assert_eq!(run_timeout("sh", &["-c", "echo hi"], Duration::from_secs(5)).unwrap(), "hi");
        assert_eq!(run_timeout("sh", &["-c", "echo bad >&2; exit 3"], Duration::from_secs(5)).unwrap_err(), "bad");
        let t = Instant::now();
        let e = run_timeout("sh", &["-c", "sleep 30"], Duration::from_millis(200)).unwrap_err();
        assert!(e.contains("未结束") && t.elapsed() < Duration::from_secs(5), "{e}");
        // 输出远超管道缓冲（64KB）也不能因为没人读而被误判超时
        let big = run_timeout("sh", &["-c", "head -c 300000 /dev/zero | tr '\\0' 'x'"], Duration::from_secs(10)).unwrap();
        assert_eq!(big.len(), 300000);
    }

    #[test]
    fn service_of_from_catalog() {
        assert_eq!(service_of("books"), Some("book-serve"));
        assert_eq!(service_of("fonts"), Some("font-serve"));
        assert_eq!(service_of("wallpapers"), Some("wallpaper-serve"));
        assert_eq!(service_of("nope"), None);
        assert_eq!(service_of("weread"), None, "微读线已砍（2026-09-05），目录表不再有它");
        assert!(MODULES.iter().all(|m| m.installable));
    }
    #[test]
    fn only_mind_serve_has_no_event_stream() {
        // mind-serve 没有 /events 路由；其余都有。表和服务真实情况不一致会让网关白打 404（耗电）或漏掉事件。
        let no: Vec<&str> = MODULES.iter().filter(|m| !m.events).map(|m| m.service).collect();
        assert_eq!(no, ["mind-serve"]);
    }
    #[test]
    fn status_reports_three_states() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        std::fs::create_dir_all(paths.bin_dir()).unwrap();
        std::fs::write(paths.bin_dir().join("book-serve"), b"x").unwrap(); // 已装、未跑（注册表空）
        let v: serde_json::Value = serde_json::from_slice(&status(&paths).body).unwrap();
        let mods = v["modules"].as_array().unwrap();
        let find = |svc: &str| mods.iter().find(|m| m["service"] == svc).unwrap();
        assert_eq!(find("book-serve")["installed"], true);
        assert_eq!(find("book-serve")["running"], false);
        assert_eq!(find("book-serve")["hasWeb"], true);
        assert_eq!(find("font-serve")["installed"], false);
        assert_eq!(find("font-serve")["hasWeb"], false, "未装则网页无该功能");
        assert!(mods.iter().all(|m| m["service"] != "weread-serve"));
    }
    #[test]
    fn foundation_probes_weread_alongside_koreader() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        let v: serde_json::Value = serde_json::from_slice(&foundation(&paths).body).unwrap();
        assert_eq!(v["weread"], false, "没装时探测为 false");
        std::fs::create_dir_all(paths.weread_root().join("bin")).unwrap();
        std::fs::write(paths.weread_root().join("bin/start-remarkable-weread.sh"), b"x").unwrap();
        let v: serde_json::Value = serde_json::from_slice(&foundation(&paths).body).unwrap();
        assert_eq!(v["weread"], true);
    }
}
