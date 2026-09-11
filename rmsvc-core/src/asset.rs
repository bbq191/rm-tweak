//! 资产仓库（Repository）与上传流程模板（Template Method）。
//! 字体、壁纸、KOReader 字体/词典、母版库都是"一个目录里的一堆文件"：上传→暂存→扩展名门→校验→安装→回执
//! 这一套只在 [`AssetUploadFlow`] 写一次，各仓库只实现差异（`validate`/`install`/`list`/`remove`），
//! 拒收 / 成功文案也由仓库按需覆盖（`reject_message`/`success_message`），不再各服务手搓 multipart 循环。
use crate::formats;
use crate::multipart::{safe_basename, MultipartReader};
use crate::paths::Paths;
use serde::Serialize;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct AssetItem {
    pub name: String,
    pub bytes: u64,
    /// 各仓库自定义附加信息（字体家族名 / 壁纸尺寸…）。
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub extra: serde_json::Value,
}

impl AssetItem {
    pub fn plain(name: impl Into<String>, bytes: u64) -> AssetItem {
        AssetItem { name: name.into(), bytes, extra: serde_json::Value::Null }
    }
}

/// 单项上传结果（网页 uploader / CLI `print_receipts` 共同的契约：`name` + `ok` + `message`）。
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct UploadOutcome {
    pub name: String,
    pub ok: bool,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<AssetItem>,
}

impl UploadOutcome {
    pub fn fail(name: impl Into<String>, message: impl Into<String>) -> UploadOutcome {
        UploadOutcome { name: name.into(), ok: false, message: message.into(), item: None }
    }
}

pub trait AssetStore {
    /// 资产类型（日志/暂存文件名用），如 "font" / "wallpaper" / "book"。
    fn kind(&self) -> &'static str;
    /// 允许的扩展名（小写、不带点）；**空＝任意**。
    fn allowed_ext(&self) -> &'static [&'static str];
    /// 校验暂存文件（格式/尺寸/与内建冲突…）。缺省不校验。
    fn validate(&self, _name: &str, _staged: &Path) -> Result<(), String> {
        Ok(())
    }
    /// 安装暂存文件（移动/转换到最终位置），返回条目。暂存文件之后由流程删除（已被 rename 走也无妨）。
    fn install(&self, name: &str, staged: &Path) -> Result<AssetItem, String>;
    fn list(&self) -> Vec<AssetItem>;
    fn remove(&self, name: &str) -> Result<(), String>;
    /// 扩展名不在白名单时的回执文案。
    fn reject_message(&self) -> String {
        format!("不支持的扩展名（允许：{}）", self.allowed_ext().join(" / "))
    }
    /// 安装成功的回执文案（`requested`=上传时的文件名，`item.name` 可能被仓库改名）。
    fn success_message(&self, _requested: &str, _item: &AssetItem) -> String {
        "已安装".into()
    }
}

/// 整批是否全成功（非空且逐项 ok）。
pub fn all_ok(items: &[UploadOutcome]) -> bool {
    !items.is_empty() && items.iter().all(|i| i.ok)
}

/// 标准回执 `{ok, items, …extra}`——各上传处理器在此之上只加自己的字段（note / activated …）。
pub fn receipt(items: &[UploadOutcome], extra: serde_json::Value) -> serde_json::Value {
    let mut v = serde_json::json!({"ok": all_ok(items), "items": items});
    if let (Some(dst), Some(src)) = (v.as_object_mut(), extra.as_object()) {
        for (k, val) in src {
            dst.insert(k.clone(), val.clone());
        }
    }
    v
}

/// 上传流程模板：multipart 逐文件流式落暂存 → 扩展名门 → validate → install；逐项独立成败。
pub struct AssetUploadFlow {
    tmp_dir: PathBuf,
}

impl AssetUploadFlow {
    /// 暂存在运行时目录 `$XDG_RUNTIME_DIR/shelf/upload`（字体 / 壁纸 / 词典这类小文件）。
    pub fn new(paths: &Paths) -> Self {
        AssetUploadFlow { tmp_dir: paths.upload_tmp_dir() }
    }
    /// 暂存在指定目录——大文件（书）应与最终目录同分区，install 才能 rename 而不是拷贝。
    pub fn in_dir(dir: PathBuf) -> Self {
        AssetUploadFlow { tmp_dir: dir }
    }

    /// 处理一整个 multipart 请求体。
    pub fn run<R: Read>(&self, store: &dyn AssetStore, body: R, boundary: &str) -> Result<Vec<UploadOutcome>, String> {
        std::fs::create_dir_all(&self.tmp_dir).map_err(|e| e.to_string())?;
        let mut mp = MultipartReader::new(body, boundary);
        let mut out = Vec::new();
        loop {
            let mut part = match mp.next_part() {
                Ok(Some(p)) => p,
                Ok(None) => break,
                Err(e) => return Err(format!("multipart 解析失败: {e}")),
            };
            let Some(fname) = part.filename.clone() else { continue };
            let name = safe_basename(&fname, "upload.bin");
            if !formats::has_ext(&name, store.allowed_ext()) {
                out.push(UploadOutcome::fail(name, store.reject_message()));
                continue;
            }
            let staged = self.tmp_dir.join(format!(".{}.{}.part", uuid::Uuid::new_v4().simple(), store.kind()));
            let outcome = match crate::multipart::receive_part_to(&staged, &mut part) {
                Err(e) => UploadOutcome::fail(name, format!("接收失败: {e}")),
                Ok(0) => UploadOutcome::fail(name, "空文件"),
                Ok(_) => match store.validate(&name, &staged).and_then(|_| store.install(&name, &staged)) {
                    // 成功项的 name 用落地名（仓库可能改名 / 加前缀），回执与列表一致。
                    Ok(item) => UploadOutcome { message: store.success_message(&name, &item), name: item.name.clone(), ok: true, item: Some(item) },
                    Err(e) => UploadOutcome::fail(name, e),
                },
            };
            let _ = std::fs::remove_file(&staged);
            out.push(outcome);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct MemStore {
        dir: PathBuf,
        installed: Mutex<Vec<String>>,
    }
    impl AssetStore for MemStore {
        fn kind(&self) -> &'static str {
            "test"
        }
        fn allowed_ext(&self) -> &'static [&'static str] {
            &["txt"]
        }
        fn validate(&self, name: &str, staged: &Path) -> Result<(), String> {
            if name.starts_with("bad") {
                return Err("名字不合法".into());
            }
            if std::fs::metadata(staged).map(|m| m.len()).unwrap_or(0) > 10 {
                return Err("太大".into());
            }
            Ok(())
        }
        fn install(&self, name: &str, staged: &Path) -> Result<AssetItem, String> {
            let dest = self.dir.join(name);
            std::fs::copy(staged, &dest).map_err(|e| e.to_string())?;
            self.installed.lock().unwrap().push(name.to_string());
            Ok(AssetItem::plain(name, std::fs::metadata(&dest).unwrap().len()))
        }
        fn list(&self) -> Vec<AssetItem> {
            vec![]
        }
        fn remove(&self, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn success_message(&self, requested: &str, item: &AssetItem) -> String {
            format!("装好 {requested}→{}", item.name)
        }
    }

    #[test]
    fn flow_reports_per_item_and_cleans_tmp() {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        let store = MemStore { dir: t.path().join("dst"), installed: Mutex::new(vec![]) };
        std::fs::create_dir_all(&store.dir).unwrap();
        let mut body = Vec::new();
        for (f, d) in [("ok.txt", "hello"), ("bad.txt", "x"), ("big.txt", "0123456789ab"), ("nope.bin", "z"), ("../evil.txt", "e")] {
            body.extend_from_slice(format!("--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{f}\"\r\n\r\n{d}\r\n").as_bytes());
        }
        body.extend_from_slice(b"--B--\r\n");
        let out = AssetUploadFlow::new(&paths).run(&store, &body[..], "B").unwrap();
        let summary: Vec<(String, bool)> = out.iter().map(|o| (o.name.clone(), o.ok)).collect();
        assert_eq!(
            summary,
            vec![("ok.txt".into(), true), ("bad.txt".into(), false), ("big.txt".into(), false), ("nope.bin".into(), false), ("evil.txt".into(), true)]
        );
        assert_eq!(out[0].message, "装好 ok.txt→ok.txt", "成功文案由仓库定");
        assert_eq!(out[3].message, "不支持的扩展名（允许：txt）");
        assert_eq!(*store.installed.lock().unwrap(), vec!["ok.txt", "evil.txt"]);
        assert!(std::fs::read_dir(paths.upload_tmp_dir()).unwrap().next().is_none(), "暂存应清空");
        let r = receipt(&out, serde_json::json!({"note": "n"}));
        assert_eq!(r["ok"], false);
        assert_eq!(r["items"].as_array().unwrap().len(), 5);
        assert_eq!(r["note"], "n");
    }
}
