//! 阅读方向查询（2026-09-24）：xochitl 阅读器里的 `reader-page-turn.qmd` 打开书时问
//! `GET /reading-direction/{uuid}` → `{"rtl": bool}`，为真就把左右滑动/点边缘翻页对调（日漫从右往左）。
//! 判据只有一个：书库里 `<uuid>.epub` 的 OPF `<spine page-progression-direction="rtl">`
//! （`bookconv::placeholder::epub_is_rtl`，只读两个 zip 条目）。不是 EPUB / 找不到 / 读不了一律 `false`
//! ——宁可按原生方向，也不误翻。按（大小, mtime）缓存，重复打开同一本书不再解 zip。
//!
//! **手动指定清单**（`$XDG_STATE_HOME/shelf/books/rtl-overrides.json`，uuid 字符串数组）：书里没写标记、但确实
//! 从右往左的书（calibre 转出的漫画大多不写）。2026-09-24 用户定："这次先手动指定，以后新传的书还是看书里自带的标记"
//! ——所以清单没有网页入口，只是给已经在设备上的那批书补一次；每次查询现读（文件很小）。
//! 2026-09-25 起母版库可按书设"阅读方向"（`staging` 的 `set_direction`）：设了方向的书若已加入过 xochitl（边车
//! `render.uuid`），就顺手把那个 uuid 写进/移出清单（[`ReadingDirection::set_override`]），已落库的副本不必重投即生效；
//! 之后再投递、渲染自检认到新 uuid 时也按设置同步。清单仍可手改，两边写同一份文件（写入走进程内锁 + 原子替换）。
use rmsvc_core::fs::plain_name;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

/// 缓存条目上限：书库几十到几百本，超了整表清空重来即可（查询本身很便宜）。
const CACHE_MAX: usize = 1024;

/// uuid → (文件大小, mtime, 是否从右往左)。
type RtlCache = HashMap<String, (u64, Option<SystemTime>, bool)>;

pub struct ReadingDirection {
    lib: PathBuf,
    overrides: PathBuf,
    cache: Mutex<RtlCache>,
    /// 清单的读-改-写锁（查询只读不拿它）。
    write: Mutex<()>,
}

impl ReadingDirection {
    pub fn new(lib: &Path, overrides: &Path) -> ReadingDirection {
        ReadingDirection { lib: lib.to_path_buf(), overrides: overrides.to_path_buf(), cache: Mutex::new(HashMap::new()), write: Mutex::new(()) }
    }

    /// 手动指定清单里有没有这本（文件缺失 / 解析不了 = 没有）。
    fn overridden(&self, uuid: &str) -> bool {
        std::fs::read(&self.overrides)
            .ok()
            .and_then(|b| serde_json::from_slice::<Vec<String>>(&b).ok())
            .is_some_and(|list| list.iter().any(|u| u == uuid))
    }

    /// 把 `uuid` 加进（`on=true`）或移出手动清单。返回清单是否真的变了。清单文件缺失＝空清单；
    /// **解析不了就报错、不覆盖**（可能是用户手改到一半，不能替他清空）。
    pub fn set_override(&self, uuid: &str, on: bool) -> Result<bool, String> {
        let uuid = plain_name(uuid)?;
        let _guard = rmsvc_core::sync::lock(&self.write);
        let mut list: Vec<String> = match std::fs::read(&self.overrides) {
            Ok(b) => serde_json::from_slice(&b).map_err(|e| format!("手动清单 {} 解析失败（未改动）: {e}", self.overrides.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(format!("读手动清单失败: {e}")),
        };
        let had = list.iter().any(|u| u == uuid);
        if had == on {
            return Ok(false);
        }
        if on {
            list.push(uuid.to_string());
        } else {
            list.retain(|u| u != uuid);
        }
        if let Some(dir) = self.overrides.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("建目录失败: {e}"))?;
        }
        let bytes = serde_json::to_vec_pretty(&list).map_err(|e| e.to_string())?;
        rmsvc_core::fs::write_atomic(&self.overrides, &bytes).map_err(|e| format!("写手动清单失败: {e}"))?;
        Ok(true)
    }

    /// uuid 非法（含路径分隔符等）→ `Err`；其余情况都给出答案。
    pub fn is_rtl(&self, uuid: &str) -> Result<bool, String> {
        let uuid = plain_name(uuid)?;
        if self.overridden(uuid) {
            return Ok(true);
        }
        let path = self.lib.join(format!("{uuid}.epub"));
        let Ok(md) = std::fs::metadata(&path) else { return Ok(false) };
        let key = (md.len(), md.modified().ok());
        if let Some(&(len, mtime, rtl)) = rmsvc_core::sync::lock(&self.cache).get(uuid) {
            if (len, mtime) == key {
                return Ok(rtl);
            }
        }
        // 解 zip 不持锁：别的书同时打开时不必排在这一本后面（查询结果幂等，偶尔重复算一次无妨）。
        let rtl = bookconv::placeholder::epub_is_rtl(&path);
        let mut cache = rmsvc_core::sync::lock(&self.cache);
        if cache.len() >= CACHE_MAX {
            cache.clear();
        }
        cache.insert(uuid.to_string(), (key.0, key.1, rtl));
        Ok(rtl)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn epub(dir: &Path, uuid: &str, spine: &str) {
        let mut z = zip::ZipWriter::new(std::fs::File::create(dir.join(format!("{uuid}.epub"))).unwrap());
        let o: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        z.start_file("META-INF/container.xml", o).unwrap();
        z.write_all(br#"<container><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#).unwrap();
        z.start_file("content.opf", o).unwrap();
        z.write_all(format!("<package>{spine}</package>").as_bytes()).unwrap();
        z.finish().unwrap();
    }

    #[test]
    fn answers_rtl_ltr_missing_and_rejects_bad_uuid() {
        let t = tempfile::tempdir().unwrap();
        epub(t.path(), "manga", r#"<spine page-progression-direction="rtl"/>"#);
        epub(t.path(), "novel", "<spine/>");
        let rd = ReadingDirection::new(t.path(), &t.path().join("rtl-overrides.json"));
        assert_eq!(rd.is_rtl("manga"), Ok(true));
        assert_eq!(rd.is_rtl("novel"), Ok(false));
        assert_eq!(rd.is_rtl("nope"), Ok(false), "书库里没有 = 按原生方向");
        assert!(rd.is_rtl("../x").is_err());
        // 缓存按（大小, mtime）失效：同名文件内容变了要重新判
        std::fs::remove_file(t.path().join("manga.epub")).unwrap();
        epub(t.path(), "manga", "<spine/>");
        let f = std::fs::File::options().write(true).open(t.path().join("manga.epub")).unwrap();
        f.set_modified(SystemTime::now() + std::time::Duration::from_secs(5)).unwrap();
        assert_eq!(rd.is_rtl("manga"), Ok(false));
    }

    /// 母版库同步用的写入：缺文件从空清单起、加过不重复、移出只删这一条；清单坏了报错且不覆盖。
    #[test]
    fn set_override_adds_removes_and_refuses_to_clobber_bad_file() {
        let t = tempfile::tempdir().unwrap();
        let list = t.path().join("sub/rtl-overrides.json");
        let rd = ReadingDirection::new(t.path(), &list);
        assert_eq!(rd.set_override("u1", true), Ok(true));
        assert_eq!(rd.set_override("u1", true), Ok(false), "已在清单里");
        assert_eq!(rd.set_override("u2", true), Ok(true));
        assert_eq!(rd.is_rtl("u1"), Ok(true), "清单里的书即使书库没有这个 epub 也按从右往左");
        assert_eq!(rd.set_override("u1", false), Ok(true));
        assert_eq!(rd.set_override("u1", false), Ok(false));
        let got: Vec<String> = serde_json::from_slice(&std::fs::read(&list).unwrap()).unwrap();
        assert_eq!(got, vec!["u2".to_string()]);
        assert!(rd.set_override("../x", true).is_err());
        std::fs::write(&list, "[\"u2\", 手改到一半").unwrap();
        assert!(rd.set_override("u3", true).is_err());
        assert_eq!(std::fs::read_to_string(&list).unwrap(), "[\"u2\", 手改到一半", "坏清单原样保留");
    }

    /// 手动指定清单：书里没写标记也按从右往左；清单缺失或损坏不影响按书里标记判断。
    #[test]
    fn override_list_marks_books_without_spine_flag() {
        let t = tempfile::tempdir().unwrap();
        epub(t.path(), "ranma", "<spine/>");
        let list = t.path().join("rtl-overrides.json");
        let rd = ReadingDirection::new(t.path(), &list);
        assert_eq!(rd.is_rtl("ranma"), Ok(false));
        std::fs::write(&list, r#"["ranma"]"#).unwrap();
        assert_eq!(rd.is_rtl("ranma"), Ok(true), "清单现读，改了立即生效");
        std::fs::write(&list, "not json").unwrap();
        assert_eq!(rd.is_rtl("ranma"), Ok(false), "清单坏了当没有");
    }
}
