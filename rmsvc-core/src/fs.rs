//! 小文件工具：原子写（同目录 `.tmp` → rename，同分区 rename 原子）+ 可选 unix 权限。
//! 收编 registry / 字体 fonts.json / 各 config-save 里各自重复的"写 tmp 再 rename"实现，
//! 也把壁纸 state、book config 从"原地 write（非原子）"统一到原子写。
use std::path::{Path, PathBuf};

/// 原子写：先写 `<path>.tmp` 再 rename 覆盖。写前建齐父目录。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
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
}
