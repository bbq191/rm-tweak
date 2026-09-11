//! `xochitl.conf`（QSettings INI，`~/.config/remarkable/xochitl.conf`）`[General]` 单键读写。
//! 用途：原生休眠屏隐藏键 `SleepScreenPath`（3.28.0.172 真机通，书架白皮书 §03w）——xochitl 把该 png 满屏画成休眠屏、
//! 隐藏插画卡、每次休眠重读文件，取代了 bind-mount 覆盖 `/usr/share/remarkable/suspended.png` 整套。
//!
//! **纪律**：文件里有 `DeveloperPassword` / `UserToken` / `devicetoken` 等凭证——本模块**绝不返回、绝不打印任何行内容**，
//! 错误信息只带键名。写法：整文件读入 → 只动目标行 → 同目录 tmp+rename 原子覆盖；首次改动前备份一份 `<conf>.shelf-bak`
//! （同目录、只留最早那份）。xochitl 运行中改：Qt QSettings 在 sync 时按 mtime 重读再合并，外部加的键不会被抹；
//! 但新值要到 xochitl 下次启动（`xovi/start`）才进 `isettings`。
use crate::paths::Paths;
use std::path::{Path, PathBuf};

pub const SLEEP_SCREEN_KEY: &str = "SleepScreenPath";
const GENERAL: &str = "[General]";

/// `$XDG_CONFIG_HOME/remarkable/xochitl.conf`。
pub fn path(paths: &Paths) -> PathBuf {
    paths.config_root().join("remarkable").join("xochitl.conf")
}

/// `[General]` 段的行号范围 `[start, end)`（不含段头）；没有该段 → None。
fn general_range(lines: &[String]) -> Option<(usize, usize)> {
    let g = lines.iter().position(|l| l.trim() == GENERAL)?;
    let end = lines[g + 1..].iter().position(|l| l.trim_start().starts_with('[')).map(|i| g + 1 + i).unwrap_or(lines.len());
    Some((g + 1, end))
}

fn key_line(lines: &[String], range: (usize, usize), key: &str) -> Option<usize> {
    (range.0..range.1).find(|&i| lines[i].split_once('=').map(|(k, _)| k.trim() == key).unwrap_or(false))
}

/// 读 `[General]` 里某键的值（文件不存在 / 无键 → None）。只应对非凭证键使用。
pub fn get(conf: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(conf).ok()?;
    let lines: Vec<String> = text.lines().map(str::to_string).collect();
    let r = general_range(&lines)?;
    let i = key_line(&lines, r, key)?;
    lines[i].split_once('=').map(|(_, v)| v.trim().to_string())
}

/// 设置（`Some`）或删除（`None`）`[General]` 里某键。返回 `Ok(true)`=文件被改写，`Ok(false)`=已是目标状态。
/// 没有 `[General]` 段时在文件开头补一段。
pub fn set(conf: &Path, key: &str, value: Option<&str>) -> Result<bool, String> {
    if key.is_empty() || key.contains('=') || key.contains('\n') {
        return Err("非法键名".into());
    }
    if let Some(v) = value {
        if v.contains('\n') {
            return Err(format!("{key} 的值不能含换行"));
        }
    }
    let text = match std::fs::read_to_string(conf) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("读 xochitl.conf 失败: {e}")),
    };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let range = match general_range(&lines) {
        Some(r) => r,
        None => {
            if value.is_none() {
                return Ok(false);
            }
            lines.insert(0, GENERAL.to_string());
            (1, 1)
        }
    };
    let existing = key_line(&lines, range, key);
    match (existing, value) {
        (Some(i), Some(v)) => {
            let line = format!("{key}={v}");
            if lines[i] == line {
                return Ok(false);
            }
            lines[i] = line;
        }
        (None, Some(v)) => lines.insert(range.0, format!("{key}={v}")),
        (Some(i), None) => {
            lines.remove(i);
        }
        (None, None) => return Ok(false),
    }
    backup_once(conf)?;
    let mut out = lines.join("\n");
    out.push('\n');
    crate::fs::write_atomic(conf, out.as_bytes()).map_err(|e| format!("写 xochitl.conf 失败: {e}"))?;
    Ok(true)
}

/// 首次改动前留一份原件（`<conf>.shelf-bak`，已存在则不覆盖）。
fn backup_once(conf: &Path) -> Result<(), String> {
    if !conf.is_file() {
        return Ok(());
    }
    let mut bak = conf.as_os_str().to_owned();
    bak.push(".shelf-bak");
    let bak = PathBuf::from(bak);
    if bak.exists() {
        return Ok(());
    }
    std::fs::copy(conf, &bak).map(|_| ()).map_err(|e| format!("备份 xochitl.conf 失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "[General]\nDeveloperPassword=secret\nWebInterfaceEnabled=true\n\n[Dialogs]\nfoo=1\n";

    #[test]
    fn set_get_remove_in_general_only() {
        let t = tempfile::tempdir().unwrap();
        let c = t.path().join("xochitl.conf");
        std::fs::write(&c, SAMPLE).unwrap();
        assert_eq!(get(&c, SLEEP_SCREEN_KEY), None);
        assert!(set(&c, SLEEP_SCREEN_KEY, Some("/home/root/x.png")).unwrap());
        assert!(!set(&c, SLEEP_SCREEN_KEY, Some("/home/root/x.png")).unwrap(), "幂等");
        assert_eq!(get(&c, SLEEP_SCREEN_KEY).as_deref(), Some("/home/root/x.png"));
        let text = std::fs::read_to_string(&c).unwrap();
        assert!(text.starts_with("[General]\nSleepScreenPath=/home/root/x.png\nDeveloperPassword=secret\n"), "插在段头之后，其余原样：{text}");
        assert!(text.ends_with("[Dialogs]\nfoo=1\n"));
        assert_eq!(get(&c, "foo"), None, "别的段的键不算 General");
        assert!(set(&c, SLEEP_SCREEN_KEY, Some("/b.png")).unwrap());
        assert_eq!(text.matches("SleepScreenPath").count(), 1);
        assert!(set(&c, SLEEP_SCREEN_KEY, None).unwrap());
        assert!(!set(&c, SLEEP_SCREEN_KEY, None).unwrap());
        assert_eq!(std::fs::read_to_string(&c).unwrap(), SAMPLE, "删键后与原件逐字节相同");
        let bak = std::fs::read_to_string(t.path().join("xochitl.conf.shelf-bak")).unwrap();
        assert_eq!(bak, SAMPLE, "首次改动前的原件");
    }

    #[test]
    fn creates_general_when_missing_and_rejects_bad_input() {
        let t = tempfile::tempdir().unwrap();
        let c = t.path().join("xochitl.conf");
        std::fs::write(&c, "[Dialogs]\nfoo=1\n").unwrap();
        assert!(set(&c, "K", Some("v")).unwrap());
        assert_eq!(std::fs::read_to_string(&c).unwrap(), "[General]\nK=v\n[Dialogs]\nfoo=1\n");
        assert!(set(&c, "a=b", Some("v")).is_err());
        assert!(set(&c, "K", Some("x\ny")).is_err());
        let missing = t.path().join("none.conf");
        assert!(!set(&missing, "K", None).unwrap());
        assert!(set(&missing, "K", Some("v")).unwrap());
        assert_eq!(get(&missing, "K").as_deref(), Some("v"));
    }
}
