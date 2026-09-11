//! 固件升级（OTA）之后"需要重新安装"的判定（2026-09-25）。页头横幅读这里。
//!
//! **OTA 冲掉什么**（权威说明见 `docs/INSTALL.md`「固件升级（OTA）之后」）：`/usr/lib/systemd/system/` 下我们的
//! 单元（`shelf.target`、各服务、`xovi-reenable` …）和 `/etc` 里的 xovi 加载配置；`/home` 保留。所以网关能跑起来
//! 并不说明安装完好——二进制在 `/home`，可能是被手动拉起的。判据（只读、无副作用）：
//!
//! | 判据 | 说明 | 触发横幅 |
//! |---|---|---|
//! | 单元文件缺失 | `gateway.service`、`shelf.target`，以及**二进制已装**的每个服务的 `<服务>.service` 在单元目录里不在 | 是 |
//! | xovi 未生效 | xochitl 主进程在跑、却没映射 `xovi.so`（扩展、界面补丁全部不生效） | 是 |
//! | 固件不在白名单 | `/usr/bin/xochitl` 的 sha256 不在构建时打进来的 `packaging/firmware-allowlist.txt` | **否**，只作附加原因 |
//!
//! 固件哈希单独不触发横幅：用户在新固件上用 `install-all.sh --force` 装好后（哈希只追加进 host 的
//! `firmware-allowlist.local.txt`，网关构建时看不到），单元齐、xovi 生效，这时再弹"需要重新安装"就是永久误报。
//! 它只在另两条命中时一起列出，告诉用户"新固件要加 `--force`"；平时在「设备健康」卡片里显示。
//!
//! **什么时候算**：sha256 要读几十 MB 的 xochitl 二进制，网关启动后由后台线程**只算一次**（先等
//! [`HASH_DELAY`]，避开开机高峰），结果常驻内存；OTA 必然伴随整机重启，网关也随之重启，所以不需要再算。
//! 另外两条是几次 `stat` + 已缓存的 xochitl 映射扫描（`enhance::loaded`，同一个进程只扫一次 maps），在打开
//! 页面时现查、结果缓存 [`CHECK_TTL`]——**不在启动时一次定死**：OTA 后用户照横幅重装，`install-all.sh` 只重启
//! 有变化的服务，网关常常不会重启，定死的话恢复完横幅还挂着。都不轮询。
use crate::manage::MODULES;
use rmsvc_core::paths::Paths;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

/// 设备上 systemd 单元目录（`shelf/install.sh`、`packaging/devlib.sh` 的 `CJ_SYSD` 缺省值）。
pub const UNIT_DIR: &str = "/usr/lib/systemd/system";
const XOCHITL_BIN: &str = "/usr/bin/xochitl";
/// 构建时打进来的固件白名单（每行 `<sha256>  <标签>`，`#` 开头为注释）。
const ALLOWLIST: &str = include_str!("../../../packaging/firmware-allowlist.txt");
const HASH_DELAY: Duration = Duration::from_secs(30);
pub const CHECK_TTL: Duration = Duration::from_secs(30);

/// 白名单解析：`(sha256 小写, 标签)`。
pub fn allowlist() -> Vec<(String, String)> {
    parse_allowlist(ALLOWLIST)
}

pub fn parse_allowlist(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| {
            let (h, label) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
            (h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| (h.to_ascii_lowercase(), label.trim().to_string()))
        })
        .collect()
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Firmware {
    /// 后台线程还没算完。
    Pending,
    #[serde(rename_all = "camelCase")]
    Done { sha256: String, known: bool, label: String, millis: u64 },
    Error { message: String },
}

static FIRMWARE: OnceLock<Firmware> = OnceLock::new();

/// 网关启动时调一次：后台线程等 [`HASH_DELAY`] 后算 `/usr/bin/xochitl` 的 sha256，存进 [`FIRMWARE`]。
pub fn spawn_firmware_hash() {
    std::thread::Builder::new()
        .name("fw-hash".into())
        .spawn(|| {
            std::thread::sleep(HASH_DELAY);
            let _ = FIRMWARE.set(hash_firmware(Path::new(XOCHITL_BIN), &allowlist()));
        })
        .ok();
}

pub fn firmware() -> Firmware {
    FIRMWARE.get().cloned().unwrap_or(Firmware::Pending)
}

pub fn firmware_json() -> serde_json::Value {
    serde_json::to_value(firmware()).unwrap_or_default()
}

/// 流式算 sha256（64KB 缓冲，不把几十 MB 读进内存），再查白名单。
pub fn hash_firmware(bin: &Path, allow: &[(String, String)]) -> Firmware {
    use sha2::Digest;
    use std::io::Read;
    let t = std::time::Instant::now();
    let mut f = match std::fs::File::open(bin) {
        Ok(f) => f,
        Err(e) => return Firmware::Error { message: format!("{}: {e}", bin.display()) },
    };
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => h.update(&buf[..n]),
            Err(e) => return Firmware::Error { message: format!("{}: {e}", bin.display()) },
        }
    }
    let sha256: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    let hit = allow.iter().find(|(s, _)| *s == sha256);
    Firmware::Done { known: hit.is_some(), label: hit.map(|(_, l)| l.clone()).unwrap_or_default(), sha256, millis: t.elapsed().as_millis() as u64 }
}

/// 应当在单元目录里、实际不在的单元文件名。只查"二进制已装"的服务——没装的服务本来就没有单元，不算缺。
pub fn missing_units(paths: &Paths, unit_dir: &Path) -> Vec<String> {
    let mut want: Vec<String> = vec!["gateway.service".into(), "shelf.target".into()];
    want.extend(MODULES.iter().filter(|m| paths.bin_dir().join(m.service).is_file()).map(|m| format!("{}.service", m.service)));
    want.into_iter().filter(|u| !unit_dir.join(u).is_file()).collect()
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    /// 页头要不要显示"需要重新安装"横幅。
    pub needs_reinstall: bool,
    /// 命中的判据代码（前端按代码出中英文案）：`units-missing` / `xovi-inactive` / `firmware-unknown`。
    pub reasons: Vec<&'static str>,
    pub missing_units: Vec<String>,
    /// 恢复建议：`full`（OTA 恢复全流程）/ `xovi`（只是 xovi 没生效）/ 空。
    pub recovery: &'static str,
    pub firmware: Firmware,
}

/// 由三项事实组合出判定（纯函数，测试直接喂）。`xochitl`/`xovi` 取自 xochitl 主进程映射扫描。
pub fn decide(missing: Vec<String>, xochitl: bool, xovi: bool, fw: Firmware) -> Check {
    let mut reasons = Vec::new();
    if !missing.is_empty() {
        reasons.push("units-missing");
    }
    if xochitl && !xovi {
        reasons.push("xovi-inactive");
    }
    let needs = !reasons.is_empty();
    if needs && matches!(fw, Firmware::Done { known: false, .. }) {
        reasons.push("firmware-unknown");
    }
    let recovery = if !missing.is_empty() { "full" } else if needs { "xovi" } else { "" };
    Check { needs_reinstall: needs, reasons, missing_units: missing, recovery, firmware: fw }
}

pub fn check(paths: &Paths, unit_dir: &Path) -> Check {
    let l = crate::enhance::xochitl_loaded(paths);
    decide(missing_units(paths, unit_dir), l.xochitl, l.xovi, firmware())
}

pub fn unit_dir() -> PathBuf {
    PathBuf::from(UNIT_DIR)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_built_in_parses_and_skips_comments() {
        let a = allowlist();
        assert!(!a.is_empty(), "构建时打进来的白名单至少有当前固件那一行");
        assert!(a.iter().any(|(_, l)| l.contains("3.28.0.172")));
        assert!(a.iter().all(|(h, _)| h.len() == 64));
        assert_eq!(parse_allowlist("# x\n\nABCDEF  y\n"), vec![], "长度不对的行跳过");
        let h = "A".repeat(64);
        assert_eq!(parse_allowlist(&format!("{h}\tlabel one\n")), vec![("a".repeat(64), "label one".to_string())]);
    }

    #[test]
    fn hash_streams_and_matches_allowlist() {
        let t = tempfile::tempdir().unwrap();
        let bin = t.path().join("xochitl");
        std::fs::write(&bin, b"abc").unwrap();
        let abc = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".to_string();
        match hash_firmware(&bin, &[(abc.clone(), "测试固件".into())]) {
            Firmware::Done { sha256, known, label, .. } => assert_eq!((sha256, known, label.as_str()), (abc.clone(), true, "测试固件")),
            x => panic!("{x:?}"),
        }
        assert!(matches!(hash_firmware(&bin, &[]), Firmware::Done { known: false, .. }));
        assert!(matches!(hash_firmware(&t.path().join("none"), &[]), Firmware::Error { .. }));
    }

    #[test]
    fn missing_units_only_counts_installed_services() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().join("home").to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" { Some(h.clone()) } else { None });
        let units = t.path().join("units");
        std::fs::create_dir_all(&units).unwrap();
        std::fs::create_dir_all(paths.bin_dir()).unwrap();
        std::fs::write(paths.bin_dir().join("book-serve"), b"x").unwrap(); // 装了 book-serve，其余没装
        assert_eq!(missing_units(&paths, &units), ["gateway.service", "shelf.target", "book-serve.service"]);
        for u in ["gateway.service", "shelf.target", "book-serve.service"] {
            std::fs::write(units.join(u), b"").unwrap();
        }
        assert!(missing_units(&paths, &units).is_empty(), "没装的 font-serve 等不算缺");
    }

    #[test]
    fn decide_combines_reasons_and_firmware_alone_never_triggers() {
        let known = Firmware::Done { sha256: "x".into(), known: true, label: String::new(), millis: 1 };
        let unknown = Firmware::Done { sha256: "y".into(), known: false, label: String::new(), millis: 1 };
        let ok = decide(vec![], true, true, unknown.clone());
        assert!(!ok.needs_reinstall && ok.reasons.is_empty() && ok.recovery.is_empty(), "只有固件不在白名单：不弹横幅（--force 装过的新固件）");
        let ota = decide(vec!["gateway.service".into()], true, false, unknown.clone());
        assert!(ota.needs_reinstall);
        assert_eq!(ota.reasons, ["units-missing", "xovi-inactive", "firmware-unknown"]);
        assert_eq!(ota.recovery, "full");
        let xovi = decide(vec![], true, false, known.clone());
        assert_eq!((xovi.reasons.as_slice(), xovi.recovery), (&["xovi-inactive"][..], "xovi"));
        let no_x = decide(vec![], false, false, Firmware::Pending);
        assert!(!no_x.needs_reinstall, "xochitl 没在跑：看不出 xovi 状态，不下结论");
        let pend = decide(vec!["shelf.target".into()], true, true, Firmware::Pending);
        assert_eq!(pend.reasons, ["units-missing"], "哈希还没算完不附固件原因");
        let j = serde_json::to_value(&pend).unwrap();
        assert_eq!((j["needsReinstall"].as_bool(), j["firmware"]["state"].as_str()), (Some(true), Some("pending")));
        let j = serde_json::to_value(&known).unwrap();
        assert_eq!((j["state"].as_str(), j["known"].as_bool()), (Some("done"), Some(true)));
    }
}
