//! KOReader 配置同步执行端（Template Method：ensure_stopped → backup → stage → merge → verify）。
//! Lua 处理全交给 KOReader 自带 `luajit` 跑内嵌的 `merge.lua`（单一事实源 `shelf/koreader/merge.lua`）。
use crate::koreader::KoReader;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const MERGE_LUA: &str = include_str!("../../../koreader/merge.lua");

/// 可同步的三份文件（URL 名 → 相对 KOReader 根的路径）。
pub fn file_of(name: &str) -> Option<&'static str> {
    Some(match name {
        "settings" => "settings.reader.lua",
        "defaults" => "defaults.custom.lua",
        "gestures" => "settings/gestures.lua",
        _ => return None,
    })
}

#[derive(Serialize, Debug, PartialEq)]
pub struct ApplyResult {
    pub file: String,
    pub dry_run: bool,
    pub written: bool,
    pub changes: serde_json::Value,
    pub backup: Option<String>,
}

pub struct ConfigSync {
    pub ko: std::sync::Arc<KoReader>,
    pub backup_dir: PathBuf,
    pub tmp_dir: PathBuf,
}

impl ConfigSync {
    fn luajit(&self) -> PathBuf {
        let p = self.ko.root().join("luajit");
        if p.is_file() {
            p
        } else {
            PathBuf::from("luajit") // host 测试/无捆绑 luajit 时走 PATH
        }
    }

    fn backup(&self, target: &Path, name: &str) -> Result<Option<String>, String> {
        if !target.is_file() {
            return Ok(None);
        }
        std::fs::create_dir_all(&self.backup_dir).map_err(|e| e.to_string())?;
        let stamp = rmsvc_core::clock::now_secs();
        let b = self.backup_dir.join(format!("{name}.bak.pre-shelf-{stamp}"));
        std::fs::copy(target, &b).map_err(|e| format!("备份失败: {e}"))?;
        Ok(Some(b.display().to_string()))
    }

    /// 应用补丁（Lua 文本）。dry_run 只算差异。运行态由 `KoReader::running()`（扫 /proc）判定。
    pub fn apply(&self, file: &str, patch_lua: &str, dry_run: bool) -> Result<ApplyResult, String> {
        self.apply_with(file, patch_lua, dry_run, || self.ko.running())
    }

    /// 同 apply，运行态判定可注入（单测不碰真 /proc）。
    pub fn apply_with(&self, file: &str, patch_lua: &str, dry_run: bool, running: impl Fn() -> bool) -> Result<ApplyResult, String> {
        let rel = file_of(file).ok_or("file ∈ settings|defaults|gestures")?;
        if !dry_run && running() {
            return Err("KOReader 正在运行：退出后再同步（它退出时会回写覆盖）".into());
        }
        let target = self.ko.root().join(rel);
        if let Some(p) = target.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        std::fs::create_dir_all(&self.tmp_dir).map_err(|e| e.to_string())?;
        let merge = self.tmp_dir.join("merge.lua");
        std::fs::write(&merge, MERGE_LUA).map_err(|e| e.to_string())?;
        let patch = self.tmp_dir.join(format!("{file}.patch.lua"));
        std::fs::write(&patch, patch_lua).map_err(|e| e.to_string())?;
        let backup = if dry_run { None } else { self.backup(&target, rel.rsplit('/').next().unwrap_or(rel))? };
        let mut cmd = std::process::Command::new(self.luajit());
        cmd.arg(&merge).arg(&target).arg(&patch);
        if dry_run {
            cmd.arg("--dry-run");
        }
        let out = cmd.output().map_err(|e| format!("起 luajit 失败: {e}"))?;
        let _ = std::fs::remove_file(&patch);
        if !out.status.success() {
            return Err(format!("merge.lua 失败: {}", String::from_utf8_lossy(&out.stderr).trim()));
        }
        let j: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|e| format!("merge.lua 输出不可解析: {e}"))?;
        let written = j.get("written").and_then(|v| v.as_bool()).unwrap_or(false);
        if written {
            // 回读校验：能 dofile 且返回表
            let v = std::process::Command::new(self.luajit()).arg("-e").arg(format!("local t=dofile({:?}); assert(type(t)=='table')", target.display())).output().map_err(|e| e.to_string())?;
            if !v.status.success() {
                if let Some(b) = &backup {
                    let _ = std::fs::copy(b, &target);
                }
                return Err(format!("回读校验失败，已从备份还原: {}", String::from_utf8_lossy(&v.stderr).trim()));
            }
        }
        Ok(ApplyResult { file: rel.into(), dry_run, written, changes: j.get("changes").cloned().unwrap_or(serde_json::json!([])), backup })
    }

    pub fn read(&self, file: &str) -> Result<String, String> {
        let rel = file_of(file).ok_or("file ∈ settings|defaults|gestures")?;
        std::fs::read_to_string(self.ko.root().join(rel)).map_err(|e| format!("读 {rel} 失败: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_luajit() -> bool {
        std::process::Command::new("luajit").arg("-v").output().map(|o| o.status.success()).unwrap_or(false)
    }

    #[test]
    fn apply_dry_run_then_write_with_backup_and_idempotent() {
        if !has_luajit() {
            eprintln!("跳过：host 无 luajit");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let ko = std::sync::Arc::new(KoReader::new(t.path()));
        std::fs::write(t.path().join("settings.reader.lua"), "return { wf_level = 3, footer = { battery = true } }\n").unwrap();
        let cs = ConfigSync { ko, backup_dir: t.path().join("bk"), tmp_dir: t.path().join("tmp") };
        let patch = "return { wf_level = 1, footer = { battery = false, reclaim_height = true } }";
        let d = cs.apply_with("settings", patch, true, || false).unwrap();
        assert!(d.dry_run && !d.written && d.changes.as_array().unwrap().len() == 3 && d.backup.is_none());
        assert!(cs.apply_with("settings", patch, false, || true).unwrap_err().contains("正在运行"));
        let w = cs.apply_with("settings", patch, false, || false).unwrap();
        assert!(w.written && w.backup.is_some());
        let again = cs.apply_with("settings", patch, false, || false).unwrap();
        assert!(!again.written && again.changes.as_array().unwrap().is_empty(), "幂等");
        assert!(cs.read("settings").unwrap().contains("reclaim_height = true"));
        assert!(cs.apply_with("bogus", patch, true, || false).is_err());
        // 不存在的文件也能建（gestures 在子目录）
        let g = cs.apply_with("gestures", "return { gesture_reader = { hold_top_left_corner = { exit = true } } }", false, || false).unwrap();
        assert!(g.written && t.path().join("settings/gestures.lua").is_file());
    }
}
