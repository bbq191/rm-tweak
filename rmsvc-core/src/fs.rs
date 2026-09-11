//! 小文件工具：原子写（同目录 `.tmp` → rename，同分区 rename 原子）+ 可选 unix 权限。
//! 收编 registry / 字体 fonts.json / 各 config-save 里各自重复的"写 tmp 再 rename"实现，
//! 也把壁纸 state、book config 从"原地 write（非原子）"统一到原子写。
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// 原子写：先写同目录临时文件再 rename 覆盖。写前建齐父目录。
/// 临时名 `<path>.<pid>.<序号>.tmp`：进程内用原子计数保证唯一，多线程/多进程同时写同一个目标文件时
/// 各写各的临时文件、各自 rename（最后一个 rename 胜出），不会再像固定 `<path>.tmp` 那样互相截断，
/// 或者一个线程 rename 走了另一个线程正在写的文件。仍以 `.tmp` 结尾，按后缀忽略半成品的规则继续有效。
/// 写/rename 失败时清掉自己的临时文件，不留垃圾。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_mode(path, bytes, None)
}

/// 同 [`write_atomic`]，`mode` 给定时临时文件**创建时**就带这个权限（再受 umask 收窄）——含密钥的文件
/// 不能先按默认 0644 落出来、改名后再 chmod，中间那段窗口任何本地用户都读得到（2026-09-24 审查）。
pub fn write_atomic_mode(path: &Path, bytes: &[u8], mode: Option<u32>) -> std::io::Result<()> {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.{}.tmp", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
    let tmp = PathBuf::from(tmp);
    let write = || -> std::io::Result<()> {
        use std::io::Write;
        let mut o = std::fs::OpenOptions::new();
        o.write(true).create_new(true);
        #[cfg(unix)]
        if let Some(m) = mode {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(m);
        }
        #[cfg(not(unix))]
        let _ = mode;
        o.open(&tmp)?.write_all(bytes)
    };
    let r = write().and_then(|_| std::fs::rename(&tmp, path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// 校验"单段普通文件名"：非空、无路径分隔符、非 `.`/`..`、不以 `.` 开头（隐藏名留给各目录的半成品 / sidecar）。
/// 母版库 / 壁纸池 / KOReader 字体删除等所有"按名找文件"的入口共用，替代各处手写的 `contains('/')` 判断。
pub fn plain_name(name: &str) -> Result<&str, String> {
    if name.is_empty() || name.contains('/') || name.contains('\\') || name.starts_with('.') {
        return Err("非法文件名".into());
    }
    Ok(name)
}

/// `dir/name` 不存在则原样，否则 `dir/1_name`、`dir/2_name`… 直到空位（同名不覆盖）。
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let mut t = dir.join(name);
    let mut n = 1;
    while t.exists() {
        t = dir.join(format!("{n}_{name}"));
        n += 1;
    }
    t
}

/// 把文件挪到 `dest_dir` 下的唯一名：rename 优先，跨设备回退 copy+rm。返回落地路径（失败 None）。
pub fn move_unique(src: &Path, dest_dir: &Path) -> Option<PathBuf> {
    let name = src.file_name().and_then(|s| s.to_str()).unwrap_or("file");
    let target = unique_path(dest_dir, name);
    if std::fs::rename(src, &target).is_ok() {
        Some(target)
    } else if std::fs::copy(src, &target).is_ok() {
        let _ = std::fs::remove_file(src);
        Some(target)
    } else {
        None
    }
}

/// 设 unix 权限（如 0o600 给含密码哈希的配置）；非 unix 平台 no-op。失败静默（非致命）。
pub fn set_mode(path: &Path, mode: u32) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode));
    }
    #[cfg(not(unix))]
    let _ = (path, mode);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atomic_write_creates_parent_and_leaves_no_tmp() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("sub/dir/f.json");
        write_atomic(&p, b"hello").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello");
        // 覆盖写
        write_atomic(&p, b"world").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"world");
        // 不留 .tmp
        let mut tmp = p.as_os_str().to_owned();
        tmp.push(".tmp");
        assert!(!PathBuf::from(tmp).exists(), "tmp 应已 rename 掉");
    }

    /// 回归：多线程同时原子写同一个目标，内容始终是某一次完整写入（不被截断/撕裂），且不留 tmp。
    #[test]
    fn concurrent_atomic_writes_never_tear() {
        let t = tempfile::tempdir().unwrap();
        let p = std::sync::Arc::new(t.path().join("c.json"));
        let hs: Vec<_> = (0..8)
            .map(|i| {
                let p = p.clone();
                std::thread::spawn(move || {
                    let body = vec![b'a' + i as u8; 64 * 1024];
                    for _ in 0..50 {
                        write_atomic(&p, &body).unwrap();
                    }
                })
            })
            .collect();
        for h in hs {
            h.join().unwrap();
        }
        let got = std::fs::read(&*p).unwrap();
        assert_eq!(got.len(), 64 * 1024);
        assert!(got.iter().all(|b| *b == got[0]), "内容必须来自同一次完整写入");
        let left: Vec<_> = std::fs::read_dir(t.path()).unwrap().flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.ends_with(".tmp")).collect();
        assert!(left.is_empty(), "不该残留临时文件: {left:?}");
    }

    #[test]
    fn plain_name_and_unique_move() {
        assert_eq!(plain_name("a b.epub").unwrap(), "a b.epub");
        for bad in ["", ".", "..", "../x", "a/b", "a\\b", ".hidden"] {
            assert!(plain_name(bad).is_err(), "{bad:?} 应拒绝");
        }
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        assert_eq!(unique_path(d, "x"), d.join("x"));
        std::fs::write(d.join("x"), b"1").unwrap();
        assert_eq!(unique_path(d, "x"), d.join("1_x"));
        let src = d.join("src");
        std::fs::write(&src, b"2").unwrap();
        let dest = d.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        assert_eq!(move_unique(&src, &dest).unwrap(), dest.join("src"));
        assert!(!src.exists());
    }

    /// 含密钥文件：临时文件创建时就是 0600，不存在"先宽后紧"的窗口。
    #[cfg(unix)]
    #[test]
    fn write_atomic_mode_creates_with_mode() {
        use std::os::unix::fs::PermissionsExt;
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("k.json");
        write_atomic_mode(&p, b"{}", Some(0o600)).unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        write_atomic(&p, b"[]").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"[]");
    }
}
