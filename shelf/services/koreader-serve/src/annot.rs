//! KOReader 高亮标注读取：`books/` 下每本书旁边的 `<basename>.sdr/metadata.<ext>.lua` sidecar（KOReader
//! 原生标注存储，见 `frontend/docsettings.lua` 的 `getSidecarDir`/`getSidecarFilename`）。跟配置同步
//! （`config.rs`）同一套"交给 KOReader 自带 luajit 跑"策略，Rust 侧不解析 Lua 语法——`annot.lua`
//! （单一事实源 `shelf/koreader/annot.lua`）负责 `dofile` 出真表再转 JSON，这边只管起进程+反序列化。
//! 只认默认的"文档旁 sidecar"存储位置（KOReader 的 `HISTORY_DIR`/hash 目录两种备用位置不认），
//! 见白皮书 §03al 的范围说明。
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const ANNOT_LUA: &str = include_str!("../../../koreader/annot.lua");

/// 单条标注，字段名照抄 KOReader 自己的 `annotations` 表（下划线蛇形，来自 `readerhighlight.lua`）。
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq)]
pub struct RawItem {
    pub text: Option<String>,
    pub note: Option<String>,
    pub chapter: Option<String>,
    pub datetime: Option<String>,
    pub color: Option<String>,
    pub pos0: Option<serde_json::Value>,
    pub pos1: Option<serde_json::Value>,
}

#[derive(Deserialize, Debug, Default)]
struct SidecarJson {
    title: Option<String>,
    annotations: Vec<RawItem>,
}

/// 一本书的标注（有内容才会出现在 [`scan`] 的结果里——没有 sidecar/sidecar 里没有带 `text` 的高亮都跳过）。
#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct BookAnnotations {
    /// 相对 `books/` 的路径（正斜杠），当书的稳定标识用——KOReader 书本身没有 uuid，见白皮书 §03al。
    pub path: String,
    pub title: String,
    pub items: Vec<RawItem>,
}

fn luajit_bin(ko_root: &Path) -> PathBuf {
    let p = ko_root.join("luajit");
    if p.is_file() {
        p
    } else {
        PathBuf::from("luajit") // host 测试/无捆绑 luajit 时走 PATH
    }
}

/// 单本书的 sidecar 路径：`<dir>/<stem>.sdr/metadata.<ext>.lua`（`docsettings.lua` 的默认存储位置）。
fn sidecar_path(book: &Path) -> Option<PathBuf> {
    let stem = book.file_stem()?.to_str()?;
    let ext = book.extension()?.to_str()?;
    Some(book.with_file_name(format!("{stem}.sdr")).join(format!("metadata.{ext}.lua")))
}

fn read_one(luajit: &Path, tmp_script: &Path, sidecar: &Path) -> Result<SidecarJson, String> {
    let out = std::process::Command::new(luajit).arg(tmp_script).arg(sidecar).output().map_err(|e| format!("起 luajit 失败: {e}"))?;
    if !out.status.success() {
        return Err(format!("annot.lua 失败: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    serde_json::from_slice(&out.stdout).map_err(|e| format!("annot.lua 输出不可解析: {e}"))
}

/// 递归列出 `books/` 下所有书文件（跳过隐藏项/`.sdr` 元数据目录，同 `koreader::KoReader::list_books`
/// 的过滤规则，但这里要全树不只一层——标注可能在任意深度的子目录里）。
fn walk_books(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        let Ok(md) = e.metadata() else { continue };
        if md.is_dir() {
            if name.ends_with(".sdr") {
                continue;
            }
            walk_books(&path, base, out);
        } else if md.is_file() && path.extension().is_some() {
            out.push(path);
        }
    }
    let _ = base;
}

/// 扫全部书，返回有标注内容的那些（没有 sidecar、或 sidecar 里没有带 `text` 的条目都不出现在结果里）。
/// 单本书 luajit 解析失败只跳过那一本（打印告警），不影响其它书——见 `annot.lua` 顶部注释同一条原则。
pub fn scan(ko_root: &Path, books_dir: &Path, tmp_dir: &Path) -> Result<Vec<BookAnnotations>, String> {
    std::fs::create_dir_all(tmp_dir).map_err(|e| e.to_string())?;
    let script = tmp_dir.join("annot.lua");
    std::fs::write(&script, ANNOT_LUA).map_err(|e| e.to_string())?;
    let luajit = luajit_bin(ko_root);

    let mut books = Vec::new();
    walk_books(books_dir, books_dir, &mut books);
    books.sort();

    let mut out = Vec::new();
    for book in books {
        let Some(sidecar) = sidecar_path(&book) else { continue };
        if !sidecar.is_file() {
            continue;
        }
        let rel = book.strip_prefix(books_dir).unwrap_or(&book).to_string_lossy().replace('\\', "/");
        match read_one(&luajit, &script, &sidecar) {
            Ok(j) => {
                let items: Vec<RawItem> = j.annotations.into_iter().filter(|a| a.text.as_deref().is_some_and(|t| !t.trim().is_empty())).collect();
                if !items.is_empty() {
                    let title = j.title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| book.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| rel.clone()));
                    out.push(BookAnnotations { path: rel, title, items });
                }
            }
            Err(e) => eprintln!("[koreader-serve] 标注读取失败 {}: {e}", sidecar.display()),
        }
    }
    let _ = std::fs::remove_file(&script);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_luajit() -> bool {
        std::process::Command::new("luajit").arg("-v").output().map(|o| o.status.success()).unwrap_or(false)
    }

    fn write_sidecar(books: &Path, rel_book: &str, lua: &str) {
        let book = books.join(rel_book);
        std::fs::create_dir_all(book.parent().unwrap()).unwrap();
        std::fs::write(&book, "fake book bytes").unwrap();
        let stem = book.file_stem().unwrap().to_str().unwrap();
        let ext = book.extension().unwrap().to_str().unwrap();
        let sdr = book.with_file_name(format!("{stem}.sdr"));
        std::fs::create_dir_all(&sdr).unwrap();
        std::fs::write(sdr.join(format!("metadata.{ext}.lua")), lua).unwrap();
    }

    #[test]
    fn scan_finds_annotated_books_recursively_skips_bookmark_only_and_missing_sidecars() {
        if !has_luajit() {
            eprintln!("跳过：host 无 luajit");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let books = t.path().join("books");
        write_sidecar(
            &books,
            "小说/人骨拼图.epub",
            r#"return { doc_props = { title = "人骨拼图" }, annotations = {
                [1] = { text = "高亮原文", chapter = "第一章", color = "yellow" },
                [2] = { text = "", chapter = "纯书签没有文字" },
            } }"#,
        );
        // 没有标注 sidecar 的书：不出现在结果里。
        std::fs::create_dir_all(&books).unwrap();
        std::fs::write(books.join("没标注过.epub"), "x").unwrap();

        let tmp = t.path().join("tmp");
        let out = scan(t.path(), &books, &tmp).unwrap();
        assert_eq!(out.len(), 1);
        assert_eq!((out[0].path.as_str(), out[0].title.as_str(), out[0].items.len()), ("小说/人骨拼图.epub", "人骨拼图", 1), "空文字的纯书签该被过滤掉");
        assert_eq!(out[0].items[0].text.as_deref(), Some("高亮原文"));
    }

    #[test]
    fn scan_falls_back_to_filename_when_doc_props_title_missing() {
        if !has_luajit() {
            eprintln!("跳过：host 无 luajit");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let books = t.path().join("books");
        write_sidecar(&books, "无标题书.epub", r#"return { annotations = { [1] = { text = "有内容" } } }"#);
        let out = scan(t.path(), &books, &t.path().join("tmp")).unwrap();
        assert_eq!(out[0].title, "无标题书");
    }

    #[test]
    fn sidecar_path_strips_last_extension_only() {
        assert_eq!(sidecar_path(Path::new("/a/b/mybook.epub")).unwrap(), Path::new("/a/b/mybook.sdr/metadata.epub.lua"));
    }
}
