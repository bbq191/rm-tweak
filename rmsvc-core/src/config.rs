//! 服务配置/状态的 JSON 读写模板：读→反序列化→缺省；可选首启写出缺省；原子保存 + 可选 0600。
//! 收编各服务 config/state 的 load/seed/save 复制（book / font / gateway / wallpaper 四处曾各写一份）。
use crate::fs::{set_mode, write_atomic_mode};
use serde::{de::DeserializeOwned, Serialize};
use std::path::Path;

/// 读并反序列化；文件缺失或损坏 → `T::default()`（不写盘）。
pub fn load_or_default<T: DeserializeOwned + Default>(path: &Path) -> T {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// 同 [`load_or_default`]，但文件**不存在**时把缺省写出一份（供用户改）。
/// 只在"不存在"时写：损坏文件（存在但解析失败）保留原样、退回缺省，绝不覆盖用户可能想修的内容。
/// 写失败忽略（下次再试），不阻塞启动。
pub fn load_or_seed<T: DeserializeOwned + Default + Serialize>(path: &Path) -> T {
    match std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<T>(&t).ok()) {
        Some(c) => c,
        None => {
            let c = T::default();
            if !path.exists() {
                let _ = save(path, &c, None);
            }
            c
        }
    }
}

/// 文件存在但读不出或解析不成 `T`（缺失不算）。给"启动时落盘一次"的调用方判断该不该跳过写盘，
/// 以免把用户可能想修的内容（含 API key）换成缺省。
pub fn is_corrupt<T: DeserializeOwned>(path: &Path) -> bool {
    path.exists() && std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str::<T>(&t).ok()).is_none()
}

/// 原子保存（tmp→rename，建齐父目录）。`mode` 给敏感文件设权限（如含密码哈希的 `Some(0o600)`）。
pub fn save<T: Serialize>(path: &Path, value: &T, mode: Option<u32>) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    write_atomic_mode(path, &bytes, mode).map_err(|e| e.to_string())?;
    if let Some(m) = mode {
        set_mode(path, m); // 创建时的权限会被 umask 收窄；这里再定成准确值
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    #[derive(Serialize, Deserialize, PartialEq, Debug)]
    #[serde(default)]
    struct C {
        a: u32,
        b: String,
    }
    impl Default for C {
        fn default() -> Self {
            C { a: 7, b: "x".into() }
        }
    }
    #[test]
    fn seed_writes_default_once_then_reads_back() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("c.json");
        assert!(!p.exists());
        let c: C = load_or_seed(&p);
        assert_eq!(c, C::default());
        assert!(p.exists(), "首启应写出缺省");
        // 用户改一个字段
        save(&p, &C { a: 9, b: "y".into() }, None).unwrap();
        let c: C = load_or_seed(&p);
        assert_eq!(c, C { a: 9, b: "y".into() });
    }
    #[test]
    fn default_on_corrupt_without_overwrite() {
        let t = tempfile::tempdir().unwrap();
        let p = t.path().join("c.json");
        std::fs::write(&p, b"{ not json").unwrap();
        let c: C = load_or_default(&p);
        assert_eq!(c, C::default());
        // 损坏文件保留原样（不被 seed 覆盖）
        let c: C = load_or_seed(&p);
        assert_eq!(c, C::default());
        assert_eq!(std::fs::read(&p).unwrap(), b"{ not json", "损坏内容不覆盖");
    }
}
