//! KOReader 安装目录模型（appload 外部应用：`$SHELF_KOREADER_ROOT`，缺省 `~/xovi/exthome/appload/koreader`）。
//! 书=从母版库 adopt 原字节落 `books/`（不转换不改名——KOReader 原生读 EPUB/PDF/AZW3/MOBI/FB2/CBZ）；
//! 运行态=扫 `/proc/*/cmdline` 含 koreader（改其配置必须在它退出后，退出回写会覆盖）。
use serde::Serialize;
use rmsvc_core::asset::{AssetItem, AssetStore};
use rmsvc_core::formats;
use rmsvc_core::fs::plain_name;
use std::path::{Path, PathBuf};

pub struct KoReader {
    root: PathBuf,
}

/// KOReader 落盘目标（books/fonts/dicts 共用一个 [`AssetStore`] 实现）。`exts` 空＝接受任意格式（books「原样」）。
/// install 走 `dest/.<name>.part` → rename：`.` 前缀半成品 KOReader 扫目录不见。
pub struct KoStore {
    dest: PathBuf,
    kind: &'static str,
    exts: &'static [&'static str],
    /// 回执落点显示名（"books/"、"fonts/"、"词典 X/"）。
    into: String,
}

pub const KO_ANY: &[&str] = &[];

impl KoStore {
    pub fn new(dest: PathBuf, kind: &'static str, exts: &'static [&'static str], into: impl Into<String>) -> KoStore {
        KoStore { dest, kind, exts, into: into.into() }
    }
}

impl AssetStore for KoStore {
    fn kind(&self) -> &'static str {
        self.kind
    }
    fn allowed_ext(&self) -> &'static [&'static str] {
        self.exts
    }
    fn install(&self, name: &str, staged: &Path) -> Result<AssetItem, String> {
        std::fs::create_dir_all(&self.dest).map_err(|e| format!("建目录失败: {e}"))?;
        let dest = self.dest.join(name);
        let part = self.dest.join(format!(".{name}.part"));
        std::fs::copy(staged, &part).map_err(|e| format!("落盘失败: {e}"))?;
        let bytes = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        std::fs::rename(&part, &dest).map_err(|e| {
            let _ = std::fs::remove_file(&part);
            format!("落盘失败: {e}")
        })?;
        Ok(AssetItem::plain(name, bytes))
    }
    fn list(&self) -> Vec<AssetItem> {
        list_files(&self.dest, self.exts).into_iter().map(|f| AssetItem::plain(f.name, f.bytes)).collect()
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        std::fs::remove_file(self.dest.join(plain_name(name)?)).map_err(|e| e.to_string())
    }
    fn success_message(&self, _requested: &str, item: &AssetItem) -> String {
        format!("已放入 KOReader {}（{} 字节）", self.into, item.bytes)
    }
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct FileItem {
    pub name: String,
    pub bytes: u64,
}

/// books/ 一层里的条目（目录或文件）。
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Entry {
    pub name: String,
    pub kind: &'static str,
    pub bytes: u64,
    /// kind=dir 时：直接子文件数。
    pub count: usize,
}

impl KoReader {
    pub fn new(root: &Path) -> KoReader {
        KoReader { root: root.to_path_buf() }
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn installed(&self) -> bool {
        self.root.join("reader.lua").is_file() || self.root.join("koreader.sh").is_file() || self.root.join("books").is_dir()
    }
    pub fn books_dir(&self) -> PathBuf {
        self.root.join("books")
    }
    pub fn fonts_dir(&self) -> PathBuf {
        self.root.join("fonts")
    }
    pub fn dict_dir(&self) -> PathBuf {
        self.root.join("data/dict")
    }
    /// `git-rev` 文件（KOReader 发行包自带）。
    pub fn version(&self) -> Option<String> {
        std::fs::read_to_string(self.root.join("git-rev")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
    }
    /// 是否有 koreader 进程在跑（linux：/proc 扫 cmdline）。
    pub fn running(&self) -> bool {
        running_by_proc(Path::new("/proc"))
    }
    /// 运行中才给的提示（改配置 / 删字体 / 加书后需它刷新或重启）。
    pub fn running_note(&self, note: &'static str) -> &'static str {
        if self.running() {
            note
        } else {
            ""
        }
    }

    /// 校验并规范 folder（相对 books/ 的多级路径，无 `..`/绝对路径/反斜杠；空=根）。
    pub fn subdir(&self, folder: &str) -> Result<PathBuf, String> {
        let f = folder.trim().trim_matches('/');
        if f.is_empty() {
            return Ok(self.books_dir());
        }
        if f.contains('\\') || f.split('/').any(|seg| seg.is_empty() || seg == "." || seg == ".." || seg.starts_with('.')) {
            return Err("folder 非法（不允许 .. / 隐藏目录 / 空段）".into());
        }
        Ok(self.books_dir().join(f))
    }

    /// 列一层：子目录（kind=dir，含书数）+ 文件；隐藏项与 .sdr 元数据目录不列。
    pub fn list_books(&self, folder: &str) -> Result<Vec<Entry>, String> {
        let dir = self.subdir(folder)?;
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if name.starts_with('.') {
                    continue;
                }
                let Ok(md) = e.metadata() else { continue };
                if md.is_dir() {
                    if name.ends_with(".sdr") {
                        continue; // KOReader 每本书的元数据目录
                    }
                    v.push(Entry { name, kind: "dir", bytes: 0, count: list_files(&e.path(), KO_ANY).len() });
                } else if md.is_file() {
                    v.push(Entry { name, kind: "file", bytes: md.len(), count: 0 });
                }
            }
        }
        v.sort_by(|a, b| (a.kind != "dir", a.name.to_lowercase()).cmp(&(b.kind != "dir", b.name.to_lowercase())));
        Ok(v)
    }

    /// data/dict/ 下每个子目录=一本词典（有 .ifo 才算）。
    pub fn list_dicts(&self) -> Vec<serde_json::Value> {
        let mut v = Vec::new();
        if let Ok(rd) = std::fs::read_dir(self.dict_dir()) {
            for e in rd.flatten() {
                if e.path().is_dir() {
                    let n = list_files(&e.path(), &["ifo"]).len();
                    if n > 0 {
                        v.push(serde_json::json!({"name": e.file_name().to_string_lossy(), "ifo": n}));
                    }
                }
            }
        }
        v
    }
}

/// 目录里的普通文件（隐藏项不列，`exts` 空＝任意），按名排序。
pub fn list_files(dir: &Path, exts: &[&str]) -> Vec<FileItem> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let Ok(md) = e.metadata() else { continue };
            let name = e.file_name().to_string_lossy().to_string();
            if md.is_file() && !name.starts_with('.') && formats::has_ext(&name, exts) {
                v.push(FileItem { name, bytes: md.len() });
            }
        }
    }
    v.sort_by(|a, b| a.name.cmp(&b.name));
    v
}

fn running_by_proc(proc_dir: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(proc_dir) else { return false };
    let me = std::process::id().to_string();
    for e in rd.flatten() {
        let name = e.file_name();
        let Some(pid) = name.to_str() else { continue };
        if !pid.chars().all(|c| c.is_ascii_digit()) || pid == me {
            continue;
        }
        if let Ok(cmd) = std::fs::read(e.path().join("cmdline")) {
            let s = String::from_utf8_lossy(&cmd);
            // 只认 KOReader 真身：luajit 跑 reader.lua / koreader.sh 启动脚本。不能只匹配 "koreader"——
            // koreader-serve 自己、cargo 测试二进制 koreader_serve-xxx 都含这个词（真机+CI 都误判过）。
            if s.contains("reader.lua") || s.contains("koreader.sh") || (s.contains("luajit") && s.contains("/koreader/")) {
                return true;
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subdir_validates_folder_and_lists() {
        let t = tempfile::tempdir().unwrap();
        let k = KoReader::new(t.path());
        assert_eq!(k.subdir("").unwrap(), k.books_dir());
        assert_eq!(k.subdir("a/b").unwrap(), k.books_dir().join("a/b")); // 多级目录允许
        assert!(k.subdir("../x").is_err());
        assert!(k.subdir("a/../b").is_err());
        assert!(k.subdir(".hide").is_err());
        // 直接铺文件测 list_books 的 .sdr / 隐藏项过滤（不经上传）
        std::fs::create_dir_all(k.books_dir().join("a/b")).unwrap();
        std::fs::write(k.books_dir().join("中文 名.azw3"), b"x").unwrap();
        std::fs::write(k.books_dir().join("a/b/x.epub"), b"x").unwrap();
        std::fs::create_dir_all(k.books_dir().join("book.sdr")).unwrap();
        let root = k.list_books("").unwrap();
        assert_eq!(root.iter().map(|e| (e.name.as_str(), e.kind)).collect::<Vec<_>>(), vec![("a", "dir"), ("中文 名.azw3", "file")]);
        assert_eq!(k.list_books("a/b").unwrap()[0].kind, "file");
        assert!(k.list_dicts().is_empty());
        std::fs::create_dir_all(k.dict_dir().join("cedict")).unwrap();
        std::fs::write(k.dict_dir().join("cedict/a.ifo"), b"x").unwrap();
        assert_eq!(k.list_dicts()[0]["ifo"], 1);
    }

    #[test]
    fn kostore_install_via_flow_keeps_name_bytes_and_leaves_no_part() {
        use rmsvc_core::asset::AssetUploadFlow;
        use rmsvc_core::paths::Paths;
        let t = tempfile::tempdir().unwrap();
        let k = KoReader::new(&t.path().join("ko"));
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |key| if key == "HOME" || key == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        // multipart：一个正常文件（文件名带 ../ 前缀，应被 safe_basename 规整）+ 一个空文件（应判空、不 install）+ 一个非字体
        let mut body = Vec::new();
        for (f, d) in [("../../中文 名.ttf", "字体内容"), ("e.otf", ""), ("x.epub", "book")] {
            body.extend_from_slice(format!("--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{f}\"\r\n\r\n{d}\r\n").as_bytes());
        }
        body.extend_from_slice(b"--B--\r\n");
        let dir = k.fonts_dir();
        let store = KoStore::new(dir.clone(), "koreader-font", formats::FONT_EXTS, "fonts/");
        let out = AssetUploadFlow::new(&paths).run(&store, &body[..], "B").unwrap();
        assert_eq!(out[0].name, "中文 名.ttf");
        assert!(out[0].ok && out[0].message.contains("已放入 KOReader fonts/"));
        assert_eq!(out[0].item.as_ref().unwrap().bytes, "字体内容".len() as u64);
        assert!(!out[1].ok && out[1].message == "空文件");
        assert!(!out[2].ok && out[2].message.contains("ttf"));
        assert_eq!(std::fs::read(dir.join("中文 名.ttf")).unwrap(), "字体内容".as_bytes());
        assert!(!dir.join(".中文 名.ttf.part").exists(), "成功后 .part 应已 rename");
        assert_eq!(store.list().len(), 1);
        assert!(store.remove("../x").is_err() && store.remove("中文 名.ttf").is_ok());
    }

    #[test]
    fn running_detection_via_fake_proc() {
        let t = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(t.path().join("123")).unwrap();
        std::fs::write(t.path().join("123/cmdline"), b"./luajit\0/home/root/xovi/exthome/appload/koreader/reader.lua\0").unwrap();
        std::fs::create_dir_all(t.path().join("self")).unwrap();
        assert!(running_by_proc(t.path()));
        std::fs::write(t.path().join("123/cmdline"), b"/home/root/.local/bin/koreader-serve\0serve\0").unwrap();
        assert!(!running_by_proc(t.path()));
        std::fs::write(t.path().join("123/cmdline"), b"/x/target/debug/deps/koreader_serve-abc\0").unwrap();
        assert!(!running_by_proc(t.path()));
    }
}
