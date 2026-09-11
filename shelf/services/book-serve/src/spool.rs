//! inbox 追平队列（`$XDG_STATE_HOME/shelf/books/{inbox,.work,failed}`）——给 scp 直接丢文件的人一条"不经网页也进母版库"的路。
//! - `inbox/`  待处理（scp 丢进来的）——fswatch 追平；
//! - `.work/`  已认领、处理中（rename 原子独占，防并发重复处理）；上传流程的暂存也在这（与母版库同分区，入库 rename 零拷贝）；
//! - `failed/` 失败源（封顶 50MB，`<name>.reason` sidecar 记原因；重试=人工拷回 `inbox/`，2026-09-22 起不再有 HTTP 重试/删除接口）。
//!
//! 处理成功的书进母版库（`staging/`，见 `staging.rs`），本队列不再另存一份。
use serde::Serialize;
use rmsvc_core::fs::{move_unique, unique_path};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

const FAILED_CAP: u64 = 50 * 1024 * 1024;
/// 失败原因 sidecar 后缀。
const REASON_EXT: &str = ".reason";

pub struct Spool {
    root: PathBuf,
    lock: Mutex<()>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SpoolEntry {
    pub name: String,
    pub bytes: u64,
    pub state: &'static str,
    /// 失败原因（仅 failed 条目）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Spool {
    pub fn new(root: PathBuf) -> Spool {
        Spool { root, lock: Mutex::new(()) }
    }
    pub fn inbox(&self) -> PathBuf {
        self.root.join("inbox")
    }
    pub fn work(&self) -> PathBuf {
        self.root.join(".work")
    }
    pub fn failed(&self) -> PathBuf {
        self.root.join("failed")
    }
    pub fn ensure(&self) -> std::io::Result<()> {
        for d in [self.inbox(), self.work(), self.failed()] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }

    /// inbox 追平临界区（同一时刻只有一轮 `process_inbox`）。网页上传不再持这把锁——它曾被攥到整个请求体收完，
    /// 母版库落名的串行化改由 `Staging` 自己的落名临界区负责。
    pub fn guard(&self) -> std::sync::MutexGuard<'_, ()> {
        rmsvc_core::sync::lock(&self.lock)
    }

    /// 认领 inbox 里的文件：rename 进 .work（原子独占）。另一线程已搬走 → None。
    pub fn claim(&self, name: &str) -> Option<PathBuf> {
        let dst = unique_path(&self.work(), name);
        std::fs::rename(self.inbox().join(name), &dst).ok().map(|_| dst)
    }

    /// 归档失败源到 `failed/`，并把 `reason` 写进 `<归档名>.reason` sidecar（供列表展示，重试不再靠猜）。
    pub fn archive_failed(&self, p: &Path, reason: &str) {
        if let Some(t) = move_unique(p, &self.failed()) {
            if !reason.trim().is_empty() {
                let _ = std::fs::write(reason_path(&t), reason.trim());
            }
        }
        prune(&self.failed(), FAILED_CAP);
    }

    /// 崩溃恢复：.work 里已认领的残留移回 inbox（只在启动时调用），返回移回个数。
    /// 点开头的是上传半成品（`.<uuid>.book.part`，进程中途被杀才会残留，可达数百 MB）：直接删。
    /// 此前它们也被"移回 inbox"——但 inbox 追平按点开头名字跳过（半成品不动），于是这些垃圾永远躺在 inbox 里占盘、
    /// 每次重启还被再"恢复"一遍。
    pub fn recover_orphans(&self) -> usize {
        let mut n = 0;
        if let Ok(rd) = std::fs::read_dir(self.work()) {
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_file() {
                    continue;
                }
                if e.file_name().to_string_lossy().starts_with('.') {
                    let _ = std::fs::remove_file(&p);
                    continue;
                }
                move_unique(&p, &self.inbox());
                n += 1;
            }
        }
        n
    }

    pub fn list(&self) -> Vec<SpoolEntry> {
        let mut out = Vec::new();
        for (dir, state) in [(self.inbox(), "pending"), (self.work(), "working"), (self.failed(), "failed")] {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let fname = e.file_name().to_string_lossy().to_string();
                let Ok(md) = e.metadata() else { continue };
                if fname.ends_with(REASON_EXT) || fname.starts_with('.') || !md.is_file() {
                    continue; // sidecar / 上传半成品不作为条目
                }
                let reason = if state == "failed" { std::fs::read_to_string(reason_path(&e.path())).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty()) } else { None };
                out.push(SpoolEntry { name: fname, bytes: md.len(), state, reason });
            }
        }
        out.sort_by(|a, b| (a.state, &a.name).cmp(&(b.state, &b.name)));
        out
    }
}

/// `<文件>.reason` sidecar 路径。
fn reason_path(p: &Path) -> PathBuf {
    let mut s = p.as_os_str().to_os_string();
    s.push(REASON_EXT);
    PathBuf::from(s)
}

/// 超过 `cap` 时按 mtime 从旧到新删，直到不超。
fn prune(dir: &Path, cap: u64) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = Vec::new();
    let mut total = 0u64;
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue };
        if !md.is_file() {
            continue;
        }
        total += md.len();
        files.push((e.path(), md.len(), md.modified().unwrap_or(std::time::UNIX_EPOCH)));
    }
    if total <= cap {
        return;
    }
    files.sort_by_key(|f| f.2);
    for (p, sz, _) in files {
        if total <= cap {
            break;
        }
        if std::fs::remove_file(&p).is_ok() {
            total = total.saturating_sub(sz);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claim_and_archive_failed_roundtrip() {
        let t = tempfile::tempdir().unwrap();
        let s = Spool::new(t.path().join("books"));
        s.ensure().unwrap();
        std::fs::write(s.inbox().join("a.epub"), b"x").unwrap();
        let w = s.claim("a.epub").unwrap();
        assert!(w.starts_with(s.work()));
        assert!(s.claim("a.epub").is_none(), "二次认领必须失败");
        s.archive_failed(&w, "质量门未过：双 id");
        let listed = s.list();
        assert_eq!(listed, vec![SpoolEntry { name: "a.epub".into(), bytes: 1, state: "failed", reason: Some("质量门未过：双 id".into()) }]);
        // 同名再入 failed 不覆盖，reason 跟着归档名走（人工把 failed/ 里的文件拷回 inbox/ 即重试）
        std::fs::write(s.inbox().join("a.epub"), b"x").unwrap();
        let w2 = s.claim("a.epub").unwrap();
        std::fs::write(s.failed().join("a.epub"), b"old").unwrap();
        s.archive_failed(&w2, "转换失败");
        let l = s.list();
        assert_eq!(l.len(), 2);
        assert!(l.iter().any(|e| e.name == "1_a.epub" && e.reason.as_deref() == Some("转换失败")), "{l:?}");
    }

    #[test]
    fn recover_orphans_moves_work_back_and_hides_upload_parts() {
        let t = tempfile::tempdir().unwrap();
        let s = Spool::new(t.path().to_path_buf());
        s.ensure().unwrap();
        std::fs::write(s.work().join("half.azw3"), b"x").unwrap();
        std::fs::write(s.work().join(".abc.book.part"), b"x").unwrap();
        assert_eq!(s.list().iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["half.azw3"], "上传半成品不列");
        assert_eq!(s.recover_orphans(), 1, "只有已认领的真文件移回，半成品不计");
        assert!(s.inbox().join("half.azw3").is_file());
        assert!(!s.work().join(".abc.book.part").exists(), "上传半成品直接删");
        assert!(!s.inbox().join(".abc.book.part").exists(), "半成品不该被搬进 inbox 成为永久垃圾");
    }

    #[test]
    fn prune_keeps_newest_under_cap() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().to_path_buf();
        for (i, n) in ["old", "mid", "new"].iter().enumerate() {
            std::fs::write(d.join(n), vec![0u8; 10]).unwrap();
            let ft = std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1000 + i as u64));
            std::fs::File::options().write(true).open(d.join(n)).unwrap().set_times(ft).unwrap();
        }
        prune(&d, 20);
        assert!(!d.join("old").exists() && d.join("mid").exists() && d.join("new").exists());
    }
}
