//! 「管理 → 设备健康 → 清理遗留数据」（2026-09-25）。两类遗留，删法完全不同：
//!
//! 1. **我们自己目录里的死文件**（[`AREAS`]）：目前只有 `~/.local/state/shelf/books/done/`——2026-09-03 早期直投流程
//!    的遗留目录，现行代码不读不写（全仓 grep 过：`rs/sh/qmd/js/py/lua` 里没有任何读写 `books/done` 的地方，只剩
//!    文档里的记录），设备上还有 09-03/04 的几份旧 EPUB。由网关**逐个文件**删除，规则见 [`delete`]。
//! 2. **xochitl 书库里的旧版重复副本**（书架白皮书真机待办第 8 条：14 份《火影忍者》旧拆分卷等）。拆分卷的书名来自
//!    原书目录（如"第01卷"），跟新版整本的书名不一样，**没有精确的识别规则**，所以不做自动识别、不预先勾选：
//!    网关只**只读**列出书库里的 EPUB/PDF 文档（同名出现不止一次的标"同名"，供人工核对），用户勾选后前端逐本调
//!    book-serve 已有的 `POST /api/books/trash/add {uuid,name}`——走 xochitl 自己的软删除（`shelf-trash-agent.qmd`
//!    长轮询后调 `LibraryController.moveEntriesToTrash`，进回收站可恢复）。**网关绝不直接删、也不改 xochitl 目录里的任何文件**（外部改
//!    `.metadata` 会被运行中的 xochitl 覆写回去，直接删文件更会让它的内存模型与磁盘不一致）。
//!
//! 安全纪律（2026-09 曾因清理 `rm -rf` 掉用户漫画目录出过事故）：只删请求里逐个列出的文件名；不整目录删、不递归、
//! 不跟随符号链接；名字只能是单段文件名；删前核对"规范化后的真实路径仍在允许目录内、且是普通文件"。
use rmsvc_core::paths::Paths;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// 可清理的目录：`(代码, 目录)`。前端只能传代码，不能传路径。
pub fn areas(paths: &Paths) -> Vec<(&'static str, PathBuf)> {
    vec![("books-done", paths.state_dir().join("books").join("done"))]
}

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverFile {
    pub area: &'static str,
    pub name: String,
    pub bytes: u64,
    pub mtime: u64,
}

/// 允许目录里的普通文件（不递归；符号链接、子目录、其它类型一律不列）。目录本身是符号链接也不列。
pub fn list_files(paths: &Paths) -> Vec<LeftoverFile> {
    let mut out = Vec::new();
    for (area, dir) in areas(paths) {
        if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) {
            continue;
        }
        for e in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            if !m.file_type().is_file() {
                continue;
            }
            let Some(name) = e.file_name().to_str().map(str::to_string) else { continue };
            let mtime = m.modified().ok().map(rmsvc_core::clock::secs_of).unwrap_or(0);
            out.push(LeftoverFile { area, name, bytes: m.len(), mtime });
        }
    }
    out.sort_by(|a, b| (a.area, &a.name).cmp(&(b.area, &b.name)));
    out
}

/// 单段文件名：非空、不是 `.`/`..`、不含 `/` `\` NUL。**只看形状**，真正的"在不在允许目录里"由 [`resolve`] 核对。
fn single_component(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0'])
}

/// 把 `(area, name)` 解析成要删的真实路径；任何一步不满足就拒绝：
/// 名字是单段文件名 → 允许目录本身存在且不是符号链接 → 目标**不是符号链接**、是普通文件 →
/// 两边 `canonicalize` 后目标的父目录恰好是允许目录（挡住目录层面的链接/挂载把戏）。
pub fn resolve(paths: &Paths, area: &str, name: &str) -> Result<PathBuf, String> {
    let dir = areas(paths).into_iter().find(|(a, _)| *a == area).map(|(_, d)| d).ok_or_else(|| format!("未知清理区 {area}"))?;
    if !single_component(name) {
        return Err(format!("文件名不合法：{name}"));
    }
    let dm = std::fs::symlink_metadata(&dir).map_err(|_| "清理目录不存在".to_string())?;
    if !dm.is_dir() {
        return Err("清理目录不是普通目录（可能是符号链接），拒绝".into());
    }
    let p = dir.join(name);
    let m = std::fs::symlink_metadata(&p).map_err(|_| format!("{name} 不存在"))?;
    if m.file_type().is_symlink() {
        return Err(format!("{name} 是符号链接，拒绝"));
    }
    if !m.file_type().is_file() {
        return Err(format!("{name} 不是普通文件，拒绝"));
    }
    let (cd, cp) = (std::fs::canonicalize(&dir).map_err(|e| e.to_string())?, std::fs::canonicalize(&p).map_err(|e| e.to_string())?);
    if cp.parent() != Some(cd.as_path()) {
        return Err(format!("{name} 规范化后不在清理目录内，拒绝"));
    }
    Ok(p)
}

#[derive(Serialize, Debug, PartialEq, Default)]
pub struct DeleteOutcome {
    pub deleted: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// 逐个删除（`remove_file`，从不 `remove_dir_all`）。一个失败不影响其余；结果逐项报。
pub fn delete(paths: &Paths, area: &str, names: &[String]) -> DeleteOutcome {
    let mut o = DeleteOutcome::default();
    for n in names {
        match resolve(paths, area, n).and_then(|p| std::fs::remove_file(&p).map_err(|e| e.to_string())) {
            Ok(()) => o.deleted.push(n.clone()),
            Err(e) => o.failed.push((n.clone(), e)),
        }
    }
    o
}

// ── xochitl 书库（只读） ──

#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LibraryDoc {
    pub uuid: String,
    pub name: String,
    /// 所在文件夹名（根目录为空）。
    pub folder: String,
    /// `epub` / `pdf`。
    pub kind: &'static str,
    pub bytes: u64,
    /// xochitl `createdTime`（毫秒）。
    pub created_ms: u64,
    /// 同名（visibleName 完全相同）的活文档数；≥2 前端标"同名"。只是提示，不代表是重复副本。
    pub same_name: usize,
}

/// 书库里所有**活的** EPUB/PDF 文档（非回收站、非已删除、有 `<uuid>.epub|.pdf` 文件）。笔记本等手写文档不列——
/// 那是用户的原始笔迹，不在"清理遗留"范围里。只读 `.metadata` 和 `stat`，不写任何东西。
pub fn list_library(xochitl_dir: &Path) -> Vec<LibraryDoc> {
    let mut docs = Vec::new();
    let mut folders: HashMap<String, String> = HashMap::new();
    for e in std::fs::read_dir(xochitl_dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.extension().and_then(|x| x.to_str()) != Some("metadata") {
            continue;
        }
        let Some(uuid) = p.file_stem().and_then(|s| s.to_str()).map(str::to_string) else { continue };
        if !rmsvc_core::xochitl::is_uuid_shape(&uuid) {
            continue;
        }
        let Some(v) = std::fs::read_to_string(&p).ok().and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok()) else { continue };
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        if s("parent") == "trash" || v.get("deleted").and_then(|x| x.as_bool()) == Some(true) {
            continue;
        }
        match s("type").as_str() {
            "CollectionType" => {
                folders.insert(uuid, s("visibleName"));
            }
            "DocumentType" => {
                let found = ["epub", "pdf"].into_iter().find_map(|k| std::fs::symlink_metadata(xochitl_dir.join(format!("{uuid}.{k}"))).ok().filter(|m| m.is_file()).map(|m| (k, m.len())));
                let Some((kind, bytes)) = found else { continue };
                let created_ms = v.get("createdTime").and_then(|x| x.as_str().and_then(|s| s.parse().ok()).or_else(|| x.as_u64())).unwrap_or(0);
                docs.push((LibraryDoc { uuid, name: s("visibleName"), folder: String::new(), kind, bytes, created_ms, same_name: 0 }, s("parent")));
            }
            _ => {}
        }
    }
    let mut count: HashMap<String, usize> = HashMap::new();
    for (d, _) in &docs {
        *count.entry(d.name.clone()).or_default() += 1;
    }
    let mut out: Vec<LibraryDoc> = docs
        .into_iter()
        .map(|(mut d, parent)| {
            d.folder = folders.get(&parent).cloned().unwrap_or_default();
            d.same_name = count[&d.name];
            d
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.created_ms.cmp(&b.created_ms)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 所有 XDG 目录与 HOME 都指到临时目录（删除类测试的硬规定），并返回真实 HOME 供断言"没被碰"。
    fn sandbox(t: &tempfile::TempDir) -> Paths {
        let h = t.path().join("home").to_str().unwrap().to_string();
        let p = Paths::resolve(move |k| match k {
            "HOME" => Some(h.clone()),
            "XDG_CONFIG_HOME" => Some(format!("{h}/cfg")),
            "XDG_DATA_HOME" => Some(format!("{h}/data")),
            "XDG_STATE_HOME" => Some(format!("{h}/state")),
            "XDG_CACHE_HOME" => Some(format!("{h}/cache")),
            "XDG_RUNTIME_DIR" => Some(format!("{h}/run")),
            _ => None,
        });
        for (_, d) in areas(&p) {
            assert!(d.starts_with(t.path()), "清理目录必须落在临时目录里：{}", d.display());
        }
        p
    }

    /// 真实 HOME 下同名清理目录的快照（不存在则 None）：测试前后必须一致。
    fn real_home_snapshot() -> Option<Vec<String>> {
        let real = Paths::from_env();
        let dir = areas(&real)[0].1.clone();
        std::fs::read_dir(dir).ok().map(|rd| {
            let mut v: Vec<String> = rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
            v.sort();
            v
        })
    }

    #[test]
    fn lists_only_regular_files_and_deletes_listed_ones() {
        let before = real_home_snapshot();
        let t = tempfile::tempdir().unwrap();
        let paths = sandbox(&t);
        let done = areas(&paths)[0].1.clone();
        std::fs::create_dir_all(done.join("sub")).unwrap();
        std::fs::write(done.join("旧书.epub"), b"abc").unwrap();
        std::fs::write(done.join(".旧书.epub.delivered"), b"{}").unwrap();
        std::fs::write(done.join("sub/深处.epub"), b"x").unwrap();
        let outside = t.path().join("outside.epub");
        std::fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, done.join("link.epub")).unwrap();
        let names: Vec<String> = list_files(&paths).into_iter().map(|f| f.name).collect();
        assert_eq!(names, [".旧书.epub.delivered", "旧书.epub"], "子目录、符号链接不列");
        let o = delete(&paths, "books-done", &["旧书.epub".into(), ".旧书.epub.delivered".into()]);
        assert_eq!(o.deleted.len(), 2);
        assert!(o.failed.is_empty());
        assert!(done.join("sub/深处.epub").exists() && outside.exists(), "没列出的一律不动");
        assert_eq!(real_home_snapshot(), before, "真实 HOME 不能被碰");
    }

    #[test]
    fn rejects_traversal_absolute_symlink_dir_and_unknown_area() {
        let before = real_home_snapshot();
        let t = tempfile::tempdir().unwrap();
        let paths = sandbox(&t);
        let done = areas(&paths)[0].1.clone();
        std::fs::create_dir_all(done.join("sub")).unwrap();
        let victim = paths.state_dir().join("books").join("staging.epub"); // done/ 的兄弟：`../staging.epub`
        std::fs::write(&victim, b"keep").unwrap();
        let outside = t.path().join("outside.epub");
        std::fs::write(&outside, b"keep").unwrap();
        std::os::unix::fs::symlink(&outside, done.join("link.epub")).unwrap();
        std::fs::write(done.join("sub/in.epub"), b"keep").unwrap();
        let abs = outside.to_str().unwrap().to_string();
        for bad in ["..", ".", "", "../staging.epub", "sub/in.epub", "sub", "link.epub", abs.as_str(), "a\\b", "nul\0x"] {
            let e = resolve(&paths, "books-done", bad).unwrap_err();
            assert!(!e.is_empty(), "{bad:?} 必须被拒");
        }
        assert!(resolve(&paths, "staging", "x.epub").unwrap_err().contains("未知清理区"));
        let o = delete(&paths, "books-done", &["../staging.epub".into(), "link.epub".into(), abs.clone()]);
        assert!(o.deleted.is_empty() && o.failed.len() == 3);
        assert!(victim.exists() && outside.exists() && done.join("link.epub").symlink_metadata().is_ok() && done.join("sub/in.epub").exists());
        assert_eq!(real_home_snapshot(), before, "真实 HOME 不能被碰");
    }

    #[test]
    fn symlinked_area_dir_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let paths = sandbox(&t);
        let done = areas(&paths)[0].1.clone();
        let elsewhere = t.path().join("elsewhere");
        std::fs::create_dir_all(&elsewhere).unwrap();
        std::fs::write(elsewhere.join("user.epub"), b"keep").unwrap();
        std::fs::create_dir_all(done.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &done).unwrap(); // done/ 本身被换成指向别处的链接
        assert!(list_files(&paths).is_empty(), "链接目录不列");
        assert!(resolve(&paths, "books-done", "user.epub").unwrap_err().contains("不是普通目录"));
        assert!(elsewhere.join("user.epub").exists());
    }

    #[test]
    fn library_lists_live_books_with_folder_and_same_name_count() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let w = |n: &str, s: &str| std::fs::write(d.join(n), s).unwrap();
        let u = |i: u8| format!("{:08}-1111-1111-1111-111111111111", i);
        w(&format!("{}.metadata", u(1)), r#"{"type":"CollectionType","visibleName":"漫画","parent":""}"#);
        w(&format!("{}.metadata", u(2)), r#"{"type":"DocumentType","visibleName":"火影忍者","parent":"00000001-1111-1111-1111-111111111111","createdTime":"200"}"#);
        w(&format!("{}.epub", u(2)), "xx");
        w(&format!("{}.metadata", u(3)), r#"{"type":"DocumentType","visibleName":"火影忍者","parent":"","createdTime":"100"}"#);
        w(&format!("{}.pdf", u(3)), "yyy");
        w(&format!("{}.metadata", u(4)), r#"{"type":"DocumentType","visibleName":"回收站里的","parent":"trash"}"#);
        w(&format!("{}.epub", u(4)), "z");
        w(&format!("{}.metadata", u(5)), r#"{"type":"DocumentType","visibleName":"手写笔记","parent":""}"#); // 笔记本：没有 epub/pdf
        w("not-a-uuid.metadata", r#"{"type":"DocumentType","visibleName":"怪","parent":""}"#);
        let l = list_library(d);
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].uuid.as_str(), l[0].kind, l[0].bytes, l[0].folder.as_str(), l[0].same_name), (u(3).as_str(), "pdf", 3, "", 2));
        assert_eq!((l[1].folder.as_str(), l[1].created_ms), ("漫画", 200));
        assert!(list_library(&d.join("none")).is_empty());
    }
}
