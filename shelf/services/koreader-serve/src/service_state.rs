//! 服务组合根：KOReader 安装目录模型、配置同步、事件总线，加状态汇总与上传公共流程（HTTP 路由在 `api.rs`）。
use crate::config::ConfigSync;
use crate::koreader::{self, KoReader, KoStore};
use rmsvc_core::asset::{self, AssetUploadFlow};
use rmsvc_core::cache::TtlCache;
use rmsvc_core::formats::FONT_EXTS;
use rmsvc_core::http::{ApiError, ApiResult, Reply, Request};
use rmsvc_core::paths::Paths;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub struct State {
    pub ko: Arc<KoReader>,
    pub paths: Paths,
    pub sync: ConfigSync,
    pub bus: Arc<rmsvc_core::events::EventBus>,
    /// `GET /status` 的结果缓存（[`STATUS_TTL`]）。网页每次 refresh 都会打这个接口，而它要遍历 `/proc` 读每个
    /// 进程的 cmdline 判 KOReader 是否在跑，再扫书/字体/词典目录。本服务自己的操作（传书/字体/词典/配置）
    /// 走 [`State::notify`]，先失效再发事件；KOReader 启停这类外部变化最多滞后一个 TTL。
    /// 注意：会**改配置**的安全判断（`ConfigSync::apply` 里的"运行中拒写"）用的是实时 `running()`，不走这个缓存。
    status_cache: TtlCache<serde_json::Value>,
    /// 字体汉字覆盖率缓存：文件名 → (大小, 修改时间, 覆盖率%)。算一次要把整份字体（中文字体常 10-20MB）读进内存
    /// 解 cmap，而 `GET /fonts` 每次 refresh 都要列——文件没变（大小+mtime 一致）就不再碰它。
    font_cov: Mutex<FontCovCache>,
}

/// 字体覆盖率缓存表：文件名 → (大小, 修改时间, 覆盖率%)。
type FontCovCache = HashMap<String, (u64, Option<SystemTime>, u8)>;

/// `/status` 缓存时长：够挡住"连续几次 refresh"，又短到 KOReader 启停几秒内就能在页面上看到。
const STATUS_TTL: Duration = Duration::from_secs(3);

impl State {
    pub fn new(paths: &Paths) -> State {
        let ko = Arc::new(KoReader::new(paths.koreader_root()));
        State {
            sync: ConfigSync { ko: ko.clone(), backup_dir: paths.state_dir().join("koreader-backups"), tmp_dir: paths.runtime_dir().join("koreader") },
            ko,
            paths: paths.clone(),
            bus: Arc::new(rmsvc_core::events::EventBus::new()),
            status_cache: TtlCache::new(STATUS_TTL),
            font_cov: Mutex::new(HashMap::new()),
        }
    }

    pub fn require_installed(&self) -> Result<(), ApiError> {
        if self.ko.installed() {
            Ok(())
        } else {
            Err(ApiError { status: 409, message: "KOReader 未安装（appload 目录不存在）".into() })
        }
    }

    /// 本服务的操作改变了状态：先让 `/status` 缓存失效，再发事件（网页收到事件马上来取 `/status`，必须看到新值）。
    pub fn notify(&self, kind: &str) {
        self.status_cache.invalidate();
        self.bus.publish("koreader", kind);
    }

    pub fn status(&self) -> serde_json::Value {
        self.status_cache.get_or(|| self.compute_status())
    }

    fn compute_status(&self) -> serde_json::Value {
        let k = &self.ko;
        serde_json::json!({
            "ok": true,
            "installed": k.installed(),
            "running": k.running(),
            "version": k.version(),
            "root": k.root(),
            "booksDir": k.books_dir(),
            "books": k.count_root_books(),
            "fonts": koreader::list_files(&k.fonts_dir(), FONT_EXTS).len(),
            "dicts": k.list_dicts().len(),
        })
    }

    /// `GET /fonts` 的条目：名字、字节数、中文基本区覆盖率（同原生字体一致的判据，低覆盖当正文会缺字）。
    pub fn fonts_json(&self) -> Vec<serde_json::Value> {
        self.fonts_json_with(rmsvc_core::ttf::han_coverage_pct)
    }

    /// 同 [`Self::fonts_json`]，覆盖率计算可注入（单测数调用次数用）。按（大小, mtime）缓存；已被删掉的字体从缓存里清掉。
    fn fonts_json_with(&self, coverage: impl Fn(&[u8]) -> Option<u8>) -> Vec<serde_json::Value> {
        let dir = self.ko.fonts_dir();
        let mut cache = rmsvc_core::sync::lock(&self.font_cov);
        let mut seen = std::collections::HashSet::new();
        let items = koreader::list_files(&dir, FONT_EXTS)
            .into_iter()
            .map(|it| {
                let path = dir.join(&it.name);
                let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                let pct = match cache.get(&it.name) {
                    Some(&(len, mt, pct)) if len == it.bytes && mt == modified => pct,
                    _ => {
                        let pct = std::fs::read(&path).ok().and_then(|b| coverage(&b)).unwrap_or(0);
                        cache.insert(it.name.clone(), (it.bytes, modified, pct));
                        pct
                    }
                };
                seen.insert(it.name.clone());
                serde_json::json!({"name": it.name, "bytes": it.bytes, "cjkPct": pct})
            })
            .collect();
        cache.retain(|k, _| seen.contains(k));
        items
    }

    /// multipart 多文件 → `store`（fonts/dicts 共用同一 [`AssetUploadFlow`]）。KOReader 未装→409。
    pub fn upload(&self, r: &mut Request<'_>, store: &KoStore) -> ApiResult {
        self.require_installed()?;
        let boundary = r.multipart_boundary()?;
        let items = AssetUploadFlow::in_dir(self.upload_dir()).run(store, &mut *r.body, &boundary).map_err(ApiError::bad)?;
        Ok(Reply::ok(&asset::receipt(&items, serde_json::json!({"note": self.ko.running_note("KOReader 正在运行：重启它后才生效")}))))
    }

    /// 字体/词典上传的暂存目录：`$XDG_STATE_HOME/shelf/koreader-upload`（/home 分区，与 KOReader 目录同分区）。
    /// 此前暂存在运行时目录——单元没设 `XDG_RUNTIME_DIR` 时是 `/tmp`（tmpfs，吃内存且计入本服务 cgroup 的
    /// `MemoryMax=192M`），几十上百 MB 的词典整份先落进内存再拷一遍到 /home（2026-09-25 第四轮审计）。
    /// 现在同分区暂存、安装时直接改名进去。半成品是点前缀的 `.<uuid>.<kind>.part`，启动时 [`Self::clean_upload_dir`] 清。
    pub fn upload_dir(&self) -> std::path::PathBuf {
        self.paths.state_dir().join("koreader-upload")
    }

    /// 清掉上次进程中途被杀留下的上传暂存（只在启动时调用，此时不可能有上传在进行）。返回清掉几个。
    pub fn clean_upload_dir(&self) -> usize {
        AssetUploadFlow::in_dir(self.upload_dir()).clean_stale()
    }

    pub fn font_store(&self) -> KoStore {
        KoStore::new(self.ko.fonts_dir(), "koreader-font", FONT_EXTS, "fonts/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_in(root: &std::path::Path) -> State {
        let h = root.to_str().unwrap().to_string();
        let ko = root.join("ko");
        let paths = Paths::resolve(move |k| match k {
            "HOME" | "XDG_RUNTIME_DIR" => Some(h.clone()),
            "SHELF_KOREADER_ROOT" => Some(ko.to_str().unwrap().to_string()),
            _ => None,
        });
        State::new(&paths)
    }

    #[test]
    fn status_cached_within_ttl_and_notify_invalidates_immediately() {
        let t = tempfile::tempdir().unwrap();
        let st = state_in(t.path());
        std::fs::create_dir_all(st.ko.fonts_dir()).unwrap();
        assert_eq!(st.status()["fonts"], 0);
        std::fs::write(st.ko.fonts_dir().join("a.ttf"), b"x").unwrap();
        assert_eq!(st.status()["fonts"], 0, "TTL 内命中缓存，不重扫目录");
        st.notify("fonts");
        assert_eq!(st.status()["fonts"], 1, "操作完成路径 notify 后，马上刷新就看到变化");
    }

    /// 上传暂存在 /home 的状态目录（不是 tmpfs 运行时目录）；启动清理只删半成品。
    #[test]
    fn upload_staging_lives_in_state_dir_and_startup_cleans_parts() {
        let t = tempfile::tempdir().unwrap();
        let st = state_in(t.path());
        assert!(st.upload_dir().starts_with(st.paths.state_dir()));
        std::fs::create_dir_all(st.upload_dir()).unwrap();
        std::fs::write(st.upload_dir().join(".abc.koreader-font.part"), b"x").unwrap();
        std::fs::write(st.upload_dir().join("keep.txt"), b"x").unwrap();
        assert_eq!(st.clean_upload_dir(), 1);
        assert!(st.upload_dir().join("keep.txt").exists());
    }

    #[test]
    fn font_coverage_computed_once_per_unchanged_file() {
        use std::cell::Cell;
        let t = tempfile::tempdir().unwrap();
        let st = state_in(t.path());
        std::fs::create_dir_all(st.ko.fonts_dir()).unwrap();
        let f = st.ko.fonts_dir().join("a.ttf");
        std::fs::write(&f, b"first").unwrap();
        let calls = Cell::new(0);
        let cov = |_: &[u8]| {
            calls.set(calls.get() + 1);
            Some(77)
        };
        assert_eq!(st.fonts_json_with(cov)[0]["cjkPct"], 77);
        st.fonts_json_with(cov);
        assert_eq!(calls.get(), 1, "文件没变（大小+mtime 一致）不重读重算");
        std::fs::write(&f, b"changed-and-longer").unwrap();
        st.fonts_json_with(cov);
        assert_eq!(calls.get(), 2, "内容变了（大小变）就重算");
        std::fs::remove_file(&f).unwrap();
        assert!(st.fonts_json_with(cov).is_empty());
        assert!(st.font_cov.lock().unwrap().is_empty(), "删掉的字体从缓存清掉");
    }
}
