//! XDG 基目录规范（https://specifications.freedesktop.org/basedir-spec/）的单一路径表。
//!
//! 设备 HOME 固定 `/home/root`，各变量缺省展开为：
//! | 变量 | 缺省 | 书架用途 |
//! |---|---|---|
//! | `XDG_CONFIG_HOME` | `~/.config` | `shelf/<service>.json` 配置 |
//! | `XDG_DATA_HOME` | `~/.local/share` | `shelf/{fonts.json,wallpapers/}`；`fonts/`（fontconfig 标准位）；`remarkable/xochitl`（原生书库，同为 XDG 数据位） |
//! | `XDG_STATE_HOME` | `~/.local/state` | `shelf/{books/,wallpaper-state}` 易变状态 |
//! | `XDG_RUNTIME_DIR` | `/tmp/shelf-<uid>` | `shelf/{services/,upload/}` 注册表与上传分片（重启即清） |
//! | 可执行 | `~/.local/bin` | 各服务二进制 |
//! （host 侧缓存 `XDG_CACHE_HOME` 由 Python `shelf_cli/paths.py` 各自处理，不在此 Rust 表内。）
//! 外部约定单点可覆盖：`SHELF_KOREADER_ROOT`（appload 外部应用目录）、`SHELF_WEREAD_ROOT`
//! （WeRead 第三方 app 安装目录——跟本项目早年自建、已在 2026-09-05 砍掉的旧微读管线无关，
//! 是外部下载的独立发行包自带 `install.sh` 直接 SSH 装到设备，本项目只做只读装机探测）。
//! qmd 里的 XHR 只能写绝对路径，写的是这些缺省值的展开（文档注明，非新约定）。
use std::path::{Path, PathBuf};

const APP: &str = "shelf";

#[derive(Clone, Debug, PartialEq)]
pub struct Paths {
    home: PathBuf,
    config: PathBuf,
    data: PathBuf,
    state: PathBuf,
    runtime: PathBuf,
    koreader_root: PathBuf,
    weread_root: PathBuf,
}

impl Paths {
    /// 从真实环境解析。
    pub fn from_env() -> Paths {
        Paths::resolve(|k| std::env::var(k).ok())
    }

    /// 从任意查找函数解析（测试用 HashMap，避免改进程环境）。缺 HOME 时回落 `/home/root`（设备事实）。
    pub fn resolve<F: Fn(&str) -> Option<String>>(get: F) -> Paths {
        let home = PathBuf::from(get("HOME").filter(|s| !s.is_empty()).unwrap_or_else(|| "/home/root".into()));
        let pick = |var: &str, default: PathBuf| -> PathBuf {
            match get(var) {
                Some(v) if v.starts_with('/') => PathBuf::from(v), // 规范：相对路径视为无效
                _ => default,
            }
        };
        let uid = get("UID").and_then(|u| u.parse::<u32>().ok()).unwrap_or(0);
        Paths {
            config: pick("XDG_CONFIG_HOME", home.join(".config")),
            data: pick("XDG_DATA_HOME", home.join(".local/share")),
            state: pick("XDG_STATE_HOME", home.join(".local/state")),
            runtime: pick("XDG_RUNTIME_DIR", PathBuf::from(format!("/tmp/{APP}-{uid}"))),
            koreader_root: pick("SHELF_KOREADER_ROOT", home.join("xovi/exthome/appload/koreader")),
            weread_root: pick("SHELF_WEREAD_ROOT", home.join(".local/opt/remarkable-weread")),
            home,
        }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }
    /// `~/.local/bin`（XDG basedir 0.8 起明示的用户可执行目录）。
    pub fn bin_dir(&self) -> PathBuf {
        self.home.join(".local/bin")
    }
    pub fn config_dir(&self) -> PathBuf {
        self.config.join(APP)
    }
    pub fn data_dir(&self) -> PathBuf {
        self.data.join(APP)
    }
    /// 别的应用（如笔记线 `notes`）在同一张 XDG 表上的三个私有目录：`$XDG_{CONFIG,DATA,STATE}_HOME/<app>`。
    /// 注册表 / 上传分片仍在书架的运行时目录下（网关只认那一处），所以只开放这三个。
    pub fn app_config_dir(&self, app: &str) -> PathBuf {
        self.config.join(app)
    }
    pub fn app_data_dir(&self, app: &str) -> PathBuf {
        self.data.join(app)
    }
    pub fn app_state_dir(&self, app: &str) -> PathBuf {
        self.state.join(app)
    }
    /// `$XDG_CONFIG_HOME` 本身（只读对接如 fontconfig/fonts.conf）。
    pub fn config_root(&self) -> &Path {
        &self.config
    }
    pub fn state_dir(&self) -> PathBuf {
        self.state.join(APP)
    }
    /// 母版库（中间层暂存池）：所有内容源先原样落这里，用户再选优化 / 落库去向。
    /// 与 book-serve 的 spool 同根（`state_dir()/books`）；koreader-serve 从母版库 adopt 时也读这里。
    /// 在 /home 分区，重启 / OTA 不丢。**不套 spool done/ 的 LRU 淘汰**——留住用户还没落库的书。
    pub fn staging_dir(&self) -> PathBuf {
        self.state_dir().join("books").join("staging")
    }
    pub fn runtime_dir(&self) -> PathBuf {
        self.runtime.join(APP)
    }
    /// 用户字体目录（fontconfig 默认扫描 `$XDG_DATA_HOME/fonts`）。
    pub fn user_fonts_dir(&self) -> PathBuf {
        self.data.join("fonts")
    }
    /// xochitl 原生书库（reMarkable 自己就放在 XDG 数据位）。
    pub fn xochitl_dir(&self) -> PathBuf {
        self.data.join("remarkable/xochitl")
    }
    pub fn koreader_root(&self) -> &Path {
        &self.koreader_root
    }
    /// WeRead（第三方 app，官方安装器落点，非本项目服务）安装目录。
    pub fn weread_root(&self) -> &Path {
        &self.weread_root
    }
    /// 服务注册表目录。
    pub fn services_dir(&self) -> PathBuf {
        self.runtime_dir().join("services")
    }
    /// 上传分片暂存目录：`$XDG_STATE_HOME/shelf/upload`（/home 分区）。
    /// 原先在运行时目录——单元没设 `XDG_RUNTIME_DIR` 时落到 `/tmp`（tmpfs），几十 MB 的中文字体整份占内存
    /// 且计入服务 cgroup 的 `MemoryMax`，安装时还要再拷一遍到 /home；改到 /home 后安装可直接改名（2026-09-25）。
    pub fn upload_tmp_dir(&self) -> PathBuf {
        self.state_dir().join("upload")
    }
    /// 某服务的配置文件。
    pub fn service_config(&self, service: &str) -> PathBuf {
        self.config_dir().join(format!("{service}.json"))
    }

    /// 建齐本进程要用的目录（幂等）。
    pub fn ensure(&self) -> std::io::Result<()> {
        for d in [self.config_dir(), self.data_dir(), self.state_dir(), self.services_dir(), self.upload_tmp_dir()] {
            std::fs::create_dir_all(&d)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn defaults_expand_under_home() {
        let p = Paths::resolve(env(&[("HOME", "/home/root")]));
        assert_eq!(p.config_dir(), PathBuf::from("/home/root/.config/shelf"));
        assert_eq!(p.data_dir(), PathBuf::from("/home/root/.local/share/shelf"));
        assert_eq!(p.state_dir(), PathBuf::from("/home/root/.local/state/shelf"));
        assert_eq!(p.runtime_dir(), PathBuf::from("/tmp/shelf-0/shelf"));
        assert_eq!(p.bin_dir(), PathBuf::from("/home/root/.local/bin"));
        assert_eq!(p.user_fonts_dir(), PathBuf::from("/home/root/.local/share/fonts"));
        assert_eq!(p.xochitl_dir(), PathBuf::from("/home/root/.local/share/remarkable/xochitl"));
        assert_eq!(p.koreader_root(), Path::new("/home/root/xovi/exthome/appload/koreader"));
        assert_eq!(p.weread_root(), Path::new("/home/root/.local/opt/remarkable-weread"));
    }

    #[test]
    fn env_overrides_win_and_relative_is_ignored() {
        let p = Paths::resolve(env(&[
            ("HOME", "/h"),
            ("XDG_CONFIG_HOME", "/etc/x"),
            ("XDG_DATA_HOME", "rel/ignored"),
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("SHELF_KOREADER_ROOT", "/opt/ko"),
            ("SHELF_WEREAD_ROOT", "/opt/wr"),
            ("UID", "1000"),
        ]));
        assert_eq!(p.config_dir(), PathBuf::from("/etc/x/shelf"));
        assert_eq!(p.data_dir(), PathBuf::from("/h/.local/share/shelf"));
        assert_eq!(p.services_dir(), PathBuf::from("/run/user/1000/shelf/services"));
        assert_eq!(p.koreader_root(), Path::new("/opt/ko"));
        assert_eq!(p.weread_root(), Path::new("/opt/wr"));
        assert_eq!(p.app_state_dir("notes"), PathBuf::from("/h/.local/state/notes"), "笔记线私有目录与书架并列");
        assert_eq!(p.app_config_dir("notes"), PathBuf::from("/etc/x/notes"));
    }

    #[test]
    fn missing_home_falls_back_to_device_root() {
        let p = Paths::resolve(env(&[]));
        assert_eq!(p.home(), Path::new("/home/root"));
    }

    #[test]
    fn ensure_creates_dirs() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let p = Paths::resolve(env(&[("HOME", &h), ("XDG_RUNTIME_DIR", &h)]));
        p.ensure().unwrap();
        assert!(p.services_dir().is_dir());
        assert!(p.state_dir().is_dir());
    }
}
