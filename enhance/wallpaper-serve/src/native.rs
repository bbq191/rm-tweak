//! 原生休眠屏：xochitl.conf `[General] SleepScreenPath=<current.png>`（3.28 隐藏键，书架白皮书 §03w）。
//! 2026-09-05 真机：xochitl 把该图满屏画成休眠屏、插画卡自动隐藏、**每次休眠重读文件**——所以键只写一次、
//! 永远指向 `current.png`，换图仍是原地覆盖 current.png（wake.rs 唤醒轮换），零 `/usr` 写入、零 bind-mount。
//! 键写进去后要 xochitl 重启一次（`xovi/start`）才生效；本模块记住"写键时的 xochitl PID"，PID 变了即视为已生效。
use rmsvc_core::paths::Paths;
use rmsvc_core::xochitl_conf::{self, SLEEP_SCREEN_KEY};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

pub struct Native {
    conf: PathBuf,
    target: PathBuf,
    /// 写键时的 xochitl MainPID（None=本进程没写过键）。
    written_under_pid: Mutex<Option<u32>>,
}

impl Native {
    pub fn new(paths: &Paths, current: &Path) -> Native {
        Native { conf: xochitl_conf::path(paths), target: current.to_path_buf(), written_under_pid: Mutex::new(None) }
    }
    fn target_str(&self) -> String {
        self.target.to_string_lossy().to_string()
    }
    /// 键已指向 current.png。
    pub fn enabled(&self) -> bool {
        xochitl_conf::get(&self.conf, SLEEP_SCREEN_KEY).as_deref() == Some(self.target_str().as_str())
    }
    /// 写键（幂等）。返回是否新写入；新写入时记住当前 xochitl PID。
    pub fn enable(&self) -> Result<bool, String> {
        let changed = xochitl_conf::set(&self.conf, SLEEP_SCREEN_KEY, Some(&self.target_str()))?;
        if changed {
            *self.written_under_pid.lock().unwrap_or_else(|e| e.into_inner()) = Some(xochitl_pid().unwrap_or(0));
        }
        Ok(changed)
    }
    /// 删键（还原原生休眠屏）。返回是否真删了。
    pub fn disable(&self) -> Result<bool, String> {
        let changed = xochitl_conf::set(&self.conf, SLEEP_SCREEN_KEY, None)?;
        if changed {
            *self.written_under_pid.lock().unwrap_or_else(|e| e.into_inner()) = Some(xochitl_pid().unwrap_or(0));
        }
        Ok(changed)
    }
    /// 本进程改过键、且 xochitl 自那以后没重启过 → 还没生效。
    pub fn restart_pending(&self) -> bool {
        match *self.written_under_pid.lock().unwrap_or_else(|e| e.into_inner()) {
            None => false,
            Some(pid) => xochitl_pid().unwrap_or(0) == pid,
        }
    }
    pub fn status(&self) -> serde_json::Value {
        serde_json::json!({
            "enabled": self.enabled(),
            "path": self.target_str(),
            "restartPending": self.restart_pending(),
        })
    }
}

/// `systemctl show xochitl -p MainPID --value`；拿不到（host 测试）→ None。
fn xochitl_pid() -> Option<u32> {
    let out = Command::new("systemctl").args(["show", "xochitl", "-p", "MainPID", "--value"]).output().ok()?;
    String::from_utf8_lossy(&out.stdout).trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_disable_roundtrip_on_temp_conf() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" { Some(h.clone()) } else { None });
        let conf = xochitl_conf::path(&paths);
        std::fs::create_dir_all(conf.parent().unwrap()).unwrap();
        std::fs::write(&conf, "[General]\nUserToken=abc\n").unwrap();
        let n = Native::new(&paths, &t.path().join("current.png"));
        assert!(!n.enabled());
        assert!(n.enable().unwrap());
        assert!(n.enabled());
        assert!(!n.enable().unwrap(), "幂等");
        assert!(std::fs::read_to_string(&conf).unwrap().contains("UserToken=abc"), "其它键原样");
        assert!(n.disable().unwrap());
        assert!(!n.enabled());
        assert_eq!(std::fs::read_to_string(&conf).unwrap(), "[General]\nUserToken=abc\n");
    }
}
