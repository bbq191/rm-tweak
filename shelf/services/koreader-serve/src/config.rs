//! KOReader 配置同步执行端（Template Method：ensure_stopped → backup → stage → merge → verify）。
//! Lua 处理全交给 KOReader 自带 `luajit` 跑内嵌的 `merge.lua`（单一事实源 `shelf/koreader/merge.lua`）。
use crate::koreader::KoReader;
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const MERGE_LUA: &str = include_str!("../../../koreader/merge.lua");

/// 可同步的文件（URL 名 → 相对 KOReader 根的路径）。`directory`/`profiles` 是 2026-09-24 为"文字书 / 漫画两套方案"加的：
/// 按文件夹给新书套单书设置（docsettingtweak 插件），和按书路径自动执行的配置档（profiles 插件）。
pub const FILES: &str = "settings|defaults|gestures|directory|profiles";

/// 每个配置文件保留的写前备份份数。
const BACKUPS_KEEP: usize = 10;

pub fn file_of(name: &str) -> Option<&'static str> {
    Some(match name {
        "settings" => "settings.reader.lua",
        "defaults" => "defaults.custom.lua",
        "gestures" => "settings/gestures.lua",
        "directory" => "settings/directory_defaults.lua",
        "profiles" => "settings/profiles.lua",
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

    /// 写前备份到 `<backup_dir>/<name>.bak.pre-shelf-<unix秒>`；同一秒里第二次备份加 `-1`、`-2`… 后缀——此前同名直接
    /// 覆盖，一秒内连着应用两个补丁，第一份（真正的原件）就被第二份（第一次改完的结果）盖掉了。
    fn backup(&self, target: &Path, name: &str) -> Result<Option<PathBuf>, String> {
        if !target.is_file() {
            return Ok(None);
        }
        std::fs::create_dir_all(&self.backup_dir).map_err(|e| e.to_string())?;
        let base = format!("{name}.bak.pre-shelf-{}", rmsvc_core::clock::now_secs());
        // 序号取"这一秒已有的最大序号 + 1"而不是第一个空位：封顶清理删掉的是旧的，空出来的小序号若被新备份占用，
        // 新备份反而排在最旧、下一轮就被清掉。
        let taken = std::fs::read_dir(&self.backup_dir)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| {
                        let f = e.file_name().to_str()?.to_string();
                        let rest = f.strip_prefix(&base)?;
                        if rest.is_empty() { Some(0) } else { rest.strip_prefix('-')?.parse::<u64>().ok() }
                    })
                    .max()
            })
            .unwrap_or(None);
        let b = match taken {
            None => self.backup_dir.join(&base),
            Some(n) => self.backup_dir.join(format!("{base}-{}", n + 1)),
        };
        std::fs::copy(target, &b).map_err(|e| format!("备份失败: {e}"))?;
        Ok(Some(b))
    }

    /// 每个配置文件只留最近 [`BACKUPS_KEEP`] 份备份（按文件名里的时间戳 + 同秒序号排）。此前每次真写都留一份、从不清理，
    /// 网页每点一次"同步"就在 /home 多一份（2026-09-25 第四轮审计）。
    fn prune_backups(&self, name: &str) {
        let prefix = format!("{name}.bak.pre-shelf-");
        let Ok(rd) = std::fs::read_dir(&self.backup_dir) else { return };
        let mut found: Vec<((u64, u64), PathBuf)> = rd
            .flatten()
            .filter_map(|e| {
                let file = e.file_name().to_str()?.to_string();
                let rest = file.strip_prefix(&prefix)?;
                let (secs, seq) = rest.split_once('-').unwrap_or((rest, "0"));
                Some(((secs.parse().ok()?, seq.parse().ok()?), e.path()))
            })
            .collect();
        found.sort_by_key(|f| std::cmp::Reverse(f.0));
        for (_, p) in found.into_iter().skip(BACKUPS_KEEP) {
            let _ = std::fs::remove_file(p);
        }
    }

    /// 应用补丁（Lua 文本）。dry_run 只算差异。运行态由 `KoReader::running()`（扫 /proc）判定。
    pub fn apply(&self, file: &str, patch_lua: &str, dry_run: bool) -> Result<ApplyResult, String> {
        self.apply_with(file, patch_lua, dry_run, || self.ko.running())
    }

    /// 同 apply，运行态判定可注入（单测不碰真 /proc）。
    ///
    /// 整个过程串行化（进程内一把锁）：`merge.lua` 与 `<file>.patch.lua` 落在同一个临时目录、名字固定，两个并发请求
    /// 会互相截断/覆盖对方刚写的脚本和补丁（luajit 读到半截脚本，或者 A 的 dry-run 算的是 B 的补丁），真写时还会
    /// 交错"备份 → 合并 → 回读校验"。配置同步是人手点的低频操作，排队没有代价（2026-09-24 审计）。
    pub fn apply_with(&self, file: &str, patch_lua: &str, dry_run: bool, running: impl Fn() -> bool) -> Result<ApplyResult, String> {
        static APPLY: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _serial = rmsvc_core::sync::lock(&APPLY);
        let rel = file_of(file).ok_or(format!("file ∈ {FILES}"))?;
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
        let backup_name = rel.rsplit('/').next().unwrap_or(rel);
        let mut backup = if dry_run { None } else { self.backup(&target, backup_name)? };
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
                let how = match &backup {
                    Some(b) => {
                        let _ = std::fs::copy(b, &target);
                        "已从备份还原"
                    }
                    // 原来没有这个文件（首次同步 gestures 等）：删掉刚写出的坏文件，回到"没有"的原状——此前留着它，
                    // KOReader 下次启动 dofile 这份坏配置。
                    None => {
                        let _ = std::fs::remove_file(&target);
                        "原来没有这个文件，已删除写坏的新文件"
                    }
                };
                return Err(format!("回读校验失败，{how}: {}", String::from_utf8_lossy(&v.stderr).trim()));
            }
        } else if let Some(b) = backup.take() {
            // 补丁没带来任何改动（幂等重放）：刚才那份备份与现文件相同，留着只是占盘。
            let _ = std::fs::remove_file(b);
        }
        if backup.is_some() {
            self.prune_backups(backup_name);
        }
        Ok(ApplyResult { file: rel.into(), dry_run, written, changes: j.get("changes").cloned().unwrap_or(serde_json::json!([])), backup: backup.map(|b| b.display().to_string()) })
    }

    pub fn read(&self, file: &str) -> Result<String, String> {
        let rel = file_of(file).ok_or(format!("file ∈ {FILES}"))?;
        std::fs::read_to_string(self.ko.root().join(rel)).map_err(|e| format!("读 {rel} 失败: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::koreader::has_luajit;

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

    /// 备份只留有用的：幂等重放（没写）不留备份；同一秒里多次真写各留一份不互相覆盖；每个文件最多 [`BACKUPS_KEEP`] 份。
    #[test]
    fn backups_skip_noop_never_overwrite_and_are_capped() {
        if !has_luajit() {
            eprintln!("跳过：host 无 luajit");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let ko = std::sync::Arc::new(KoReader::new(t.path()));
        std::fs::write(t.path().join("settings.reader.lua"), "return { wf_level = 3 }\n").unwrap();
        let cs = ConfigSync { ko, backup_dir: t.path().join("bk"), tmp_dir: t.path().join("tmp") };
        let count = || std::fs::read_dir(t.path().join("bk")).map(|r| r.count()).unwrap_or(0);
        let first = cs.apply_with("settings", "return { wf_level = 100 }", false, || false).unwrap();
        assert!(first.written && count() == 1);
        let first_backup = std::fs::read_to_string(first.backup.as_ref().unwrap()).unwrap();
        assert!(first_backup.contains("wf_level = 3"), "第一份备份是原件");
        let noop = cs.apply_with("settings", "return { wf_level = 100 }", false, || false).unwrap();
        assert!(!noop.written && noop.backup.is_none() && count() == 1, "没改动不留备份");
        for i in 0..(BACKUPS_KEEP + 3) {
            assert!(cs.apply_with("settings", &format!("return {{ wf_level = {} }}", 200 + i), false, || false).unwrap().written);
        }
        assert_eq!(count(), BACKUPS_KEEP, "封顶");
        // 留下的是最新的那几份：最后一次写前的状态（wf_level = 200 + KEEP + 1）一定在
        let last = format!("wf_level = {}", 200 + BACKUPS_KEEP + 1);
        let kept: Vec<String> = std::fs::read_dir(t.path().join("bk")).unwrap().flatten().map(|e| std::fs::read_to_string(e.path()).unwrap()).collect();
        assert!(kept.iter().any(|k| k.contains(&last)), "{kept:?}");
        assert!(!kept.iter().any(|k| k.contains("wf_level = 3")), "最旧的被清掉");
    }

    /// 回读校验失败：原来有文件 → 从备份还原；原来没有 → 删掉写坏的新文件（不留一份 KOReader 读不了的配置）。
    /// 用 KOReader 根目录下的假 `luajit` 脚本模拟 merge.lua 写出坏文件。
    #[cfg(unix)]
    #[test]
    fn failed_verification_restores_or_removes() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let fake = t.path().join("luajit");
        std::fs::write(&fake, "#!/bin/sh\nif [ \"$1\" = \"-e\" ]; then exit 1; fi\nprintf 'garbage(' > \"$2\"\necho '{\"written\":true,\"changes\":[]}'\n").unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
        let ko = std::sync::Arc::new(KoReader::new(t.path()));
        let cs = ConfigSync { ko, backup_dir: t.path().join("bk"), tmp_dir: t.path().join("tmp") };
        std::fs::write(t.path().join("settings.reader.lua"), "return {}\n").unwrap();
        let e = cs.apply_with("settings", "return {}", false, || false).unwrap_err();
        assert!(e.contains("还原"), "{e}");
        assert_eq!(std::fs::read_to_string(t.path().join("settings.reader.lua")).unwrap(), "return {}\n");
        let e = cs.apply_with("gestures", "return {}", false, || false).unwrap_err();
        assert!(e.contains("删除"), "{e}");
        assert!(!t.path().join("settings/gestures.lua").exists(), "原来没有的文件不该留下写坏的版本");
    }

    /// 回归：并发的配置同步各算各的补丁（固定名的临时补丁文件不再被别的请求覆盖）。
    #[test]
    fn concurrent_applies_do_not_mix_patches() {
        if !has_luajit() {
            eprintln!("跳过：host 无 luajit");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let ko = std::sync::Arc::new(KoReader::new(t.path()));
        std::fs::write(t.path().join("settings.reader.lua"), "return { wf_level = 3 }\n").unwrap();
        let cs = std::sync::Arc::new(ConfigSync { ko, backup_dir: t.path().join("bk"), tmp_dir: t.path().join("tmp") });
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let cs = cs.clone();
                std::thread::spawn(move || {
                    let want = 1000 + i;
                    for _ in 0..5 {
                        let d = cs.apply_with("settings", &format!("return {{ wf_level = {want} }}"), true, || false).unwrap();
                        assert!(d.changes.to_string().contains(&want.to_string()), "线程 {i} 拿到了别人的补丁: {}", d.changes);
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
    }

    /// 仓库里真实的 5 份补丁（`shelf/koreader/profile/`）都能应用到各自的目标文件，且二次应用幂等（零改动）。
    #[test]
    fn repo_profile_patches_apply_and_are_idempotent() {
        if !has_luajit() {
            eprintln!("跳过：host 无 luajit");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let ko = std::sync::Arc::new(KoReader::new(t.path()));
        let cs = ConfigSync { ko, backup_dir: t.path().join("bk"), tmp_dir: t.path().join("tmp") };
        let patches = [
            ("settings", include_str!("../../../koreader/profile/settings.reader.patch.lua")),
            ("defaults", include_str!("../../../koreader/profile/defaults.custom.lua")),
            ("gestures", include_str!("../../../koreader/profile/gestures.patch.lua")),
            ("directory", include_str!("../../../koreader/profile/directory_defaults.patch.lua")),
            ("profiles", include_str!("../../../koreader/profile/profiles.patch.lua")),
        ];
        for (file, patch) in patches {
            let r = cs.apply_with(file, patch, false, || false).unwrap_or_else(|e| panic!("{file}: {e}"));
            assert!(r.written, "{file} 首次应该写入");
            let again = cs.apply_with(file, patch, true, || false).unwrap();
            assert!(again.changes.as_array().unwrap().is_empty(), "{file} 二次应用应零改动: {}", again.changes);
        }
        // 抽查落盘结果：漫画目录默认从右往左、配置档引用的状态栏预设确实存在
        let dd = std::fs::read_to_string(t.path().join("settings/directory_defaults.lua")).unwrap();
        assert!(dd.contains("books/漫画") && dd.contains("inverse_reading_order"), "{dd}");
        let st = std::fs::read_to_string(t.path().join("settings.reader.lua")).unwrap();
        assert!(st.contains("footer_presets") && st.contains("profiles_autoexec"), "settings 缺预设/自动执行");
    }
}
