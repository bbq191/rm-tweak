//! 原生 xochitl 书库免重启注入。**剥离移植**自旧项目 device-core `inject.rs` 的真机验证结论
//! （书架不引用旧 crate，此处独立实现）：
//! - `POST http://<host>/upload`（multipart 字段 `file`）免重启进库；xochitl web 只绑 USB 网口，
//!   设备端靠 lo/usb1 别名让 `10.11.99.1` 常驻可达。
//! - **GET-then-upload 归档**：`GET /documents/<folder-uuid>` 设"当前文件夹"是全局服务端状态，
//!   之后的 `/upload` 落进该文件夹（metadata.parent 会被忽略）。
//! - **防复制风暴**：大书 `/upload` 处理慢 → 408/读超时但文档已创建，此类错误**绝不重试**。
use std::io::Write;
use std::path::Path;

pub const DEFAULT_HOST: &str = "10.11.99.1";

/// 上传结果：`Delivered`=确认成功；`LikelyDelivered`=超时但很可能已创建（别重试）。
#[derive(Debug, PartialEq)]
pub enum Delivery {
    Delivered(String),
    LikelyDelivered(String),
}

pub struct Xochitl {
    agent: ureq::Agent,
    host: String,
    library_dir: std::path::PathBuf,
}

impl Xochitl {
    /// `library_dir`=书库目录（用于按名找文件夹）；`timeout_secs` 建议 300（大书）。
    pub fn new(host: &str, library_dir: &Path, timeout_secs: u64) -> Xochitl {
        // 连接 10s 即判"未送达"（:80 没绑/USB 未就绪，可安全重试）；整体 timeout 给大书处理留足（超时但已送达
        // 由 upload_likely_delivered 识别、绝不重试）。
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(10))
            .timeout(std::time::Duration::from_secs(timeout_secs))
            .build();
        Xochitl { agent, host: host.to_string(), library_dir: library_dir.to_path_buf() }
    }

    /// `/upload` 是否可达（不真上传，GET 根页）。
    pub fn reachable(&self) -> bool {
        self.agent.get(&format!("http://{}/", self.host)).timeout(std::time::Duration::from_secs(3)).call().is_ok()
    }

    /// 按 visibleName 找非回收站文件夹 uuid。
    pub fn find_folder(&self, name: &str) -> Option<String> {
        find_folder_by_name(&self.library_dir, name)
    }

    /// 给定文档 uuid，查它当前所在的设备文件夹 uuid（空串＝根）；查不到／在回收站 → `None`。
    pub fn parent_folder(&self, uuid: &str) -> Option<String> {
        parent_folder_of(&self.library_dir, uuid)
    }

    /// 在 `folder` 范围内给 `base_name` 去重，撞名就加数字后缀。
    pub fn unique_name(&self, folder: &str, base_name: &str) -> String {
        unique_document_name(&self.library_dir, folder, base_name)
    }

    /// 书库目录（`<uuid>.{metadata,content,epub,pdf}` 所在）。
    pub fn library_dir(&self) -> &Path {
        &self.library_dir
    }

    fn set_folder(&self, folder_uuid: &str) -> bool {
        let path = if folder_uuid.is_empty() { "documents/".to_string() } else { format!("documents/{folder_uuid}") };
        self.agent.get(&format!("http://{}/{}", self.host, path)).call().is_ok()
    }

    /// 上传进指定名字的文件夹（找不到→书库根，best-effort）。
    pub fn upload(&self, data: &[u8], filename: &str, content_type: &str, folder_name: &str) -> Result<Delivery, String> {
        let folder = if folder_name.is_empty() { String::new() } else { self.find_folder(folder_name).unwrap_or_default() };
        self.set_folder(&folder);
        match upload_document(&self.agent, &self.host, data, filename, content_type) {
            Ok(body) => Ok(Delivery::Delivered(body)),
            Err(e) if upload_likely_delivered(&e) => Ok(Delivery::LikelyDelivered(e)),
            Err(e) => Err(e),
        }
    }
}

/// 书库目录里所有可解析的 `<uuid>.metadata` → (uuid, JSON)。只读；解析失败的跳过。
fn metadata_entries(dir: &Path) -> Vec<(String, serde_json::Value)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("metadata") {
                return None;
            }
            let uuid = p.file_stem()?.to_str()?.to_string();
            let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&p).ok()?).ok()?;
            Some((uuid, v))
        })
        .collect()
}

fn str_of<'a>(v: &'a serde_json::Value, k: &str) -> &'a str {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("")
}

/// 非回收站、未删除的条目（文件夹与文档共用的过滤）。
fn is_live(v: &serde_json::Value) -> bool {
    str_of(v, "parent") != "trash" && v.get("deleted").and_then(|x| x.as_bool()) != Some(true)
}

pub fn find_folder_by_name(dir: &Path, name: &str) -> Option<String> {
    metadata_entries(dir).into_iter().find(|(_, v)| str_of(v, "type") == "CollectionType" && is_live(v) && str_of(v, "visibleName") == name).map(|(uuid, _)| uuid)
}

/// 给定一份文档的 uuid，读它 `.metadata` 的 `parent` 字段——就是它当前所在的设备文件夹 uuid
/// （空串＝书库根）。找不到 `.metadata`、解析失败、或书在回收站（`parent=="trash"`），一律返回
/// `None`，调用方按 best-effort 落书库根处理（2026-09-09 补：`note-serve` 生成章节笔记本时不再
/// 新建/确保文件夹，改成直接复用书本自己已经在的文件夹）。
pub fn parent_folder_of(dir: &Path, uuid: &str) -> Option<String> {
    let t = std::fs::read_to_string(dir.join(format!("{uuid}.metadata"))).ok()?;
    let v: serde_json::Value = serde_json::from_str(&t).ok()?;
    let parent = v.get("parent").and_then(|x| x.as_str())?;
    (parent != "trash").then(|| parent.to_string())
}

/// 在 `folder`（文件夹 uuid，空串＝根）范围内，如果 `base_name` 已经被别的活文档占用，就在末尾加
/// 数字后缀（`"标题"` → `"标题 2"` → `"标题 3"` ...）直到不冲突；没冲突就原样返回。只读 `.metadata`，
/// 不写、不建任何东西。
pub fn unique_document_name(dir: &Path, folder: &str, base_name: &str) -> String {
    let names: std::collections::HashSet<String> = metadata_entries(dir)
        .into_iter()
        .filter(|(_, v)| str_of(v, "type") == "DocumentType" && is_live(v) && str_of(v, "parent") == folder)
        .map(|(_, v)| str_of(&v, "visibleName").to_string())
        .collect();
    if !names.contains(base_name) {
        return base_name.to_string();
    }
    let mut n = 2u32;
    loop {
        let candidate = format!("{base_name} {n}");
        if !names.contains(&candidate) {
            return candidate;
        }
        n += 1;
    }
}

/// 书库里一份文档（非文件夹、非回收站）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocInfo {
    pub uuid: String,
    pub visible_name: String,
    /// xochitl `createdTime`（毫秒字符串）。
    pub created_ms: u64,
}

/// `createdTime >= since_ms` 的文档，新→旧。投原生后找"刚进库的那本"用（`/upload` 不回 uuid；visibleName
/// 取自 EPUB 元数据不等于文件名，所以按时间圈候选、再按书名挑）。只读 `.metadata`，不写。
pub fn find_documents_since(dir: &Path, since_ms: u64) -> Vec<DocInfo> {
    let mut out: Vec<DocInfo> = metadata_entries(dir)
        .into_iter()
        .filter(|(_, v)| str_of(v, "type") == "DocumentType" && is_live(v))
        .filter_map(|(uuid, v)| {
            let created_ms = v.get("createdTime").and_then(|x| x.as_str().and_then(|s| s.parse::<u64>().ok()).or_else(|| x.as_u64())).unwrap_or(0);
            (created_ms >= since_ms).then(|| DocInfo { uuid, visible_name: str_of(&v, "visibleName").to_string(), created_ms })
        })
        .collect();
    out.sort_by(|a, b| b.created_ms.cmp(&a.created_ms));
    out
}

/// `<uuid>.content` 的 `pageCount`：xochitl 渲染完（导入 / 打开）才写；缺或 0 → None。
pub fn page_count(dir: &Path, uuid: &str) -> Option<u64> {
    let t = std::fs::read_to_string(dir.join(format!("{uuid}.content"))).ok()?;
    let v: serde_json::Value = serde_json::from_str(&t).ok()?;
    v.get("pageCount").and_then(|x| x.as_u64()).filter(|&n| n > 0)
}

fn upload_document(agent: &ureq::Agent, host: &str, data: &[u8], filename: &str, content_type: &str) -> Result<String, String> {
    let boundary = format!("----shelf{}", uuid::Uuid::new_v4().simple());
    let mut body: Vec<u8> = Vec::with_capacity(data.len() + 256);
    write!(body, "--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\nContent-Type: {content_type}\r\n\r\n")
        .map_err(|e| e.to_string())?;
    body.extend_from_slice(data);
    write!(body, "\r\n--{boundary}--\r\n").map_err(|e| e.to_string())?;
    let resp = agent
        .post(&format!("http://{host}/upload"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .send_bytes(&body);
    match resp {
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(c, r)) => Err(format!("HTTP {c}: {}", r.into_string().unwrap_or_default())),
        Err(e) => Err(format!("上传失败: {e}")),
    }
}

/// 错误是否属于"很可能已送达"（408/读超时且非连接阶段）。
pub fn upload_likely_delivered(err: &str) -> bool {
    let e = err.to_ascii_lowercase();
    (e.contains("408") || e.contains("timed out") || e.contains("timeout")) && !e.contains("connect")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_upload_errors() {
        assert!(upload_likely_delivered("HTTP 408: 408 request timeout"));
        assert!(upload_likely_delivered("上传失败: timed out reading response"));
        assert!(!upload_likely_delivered("上传失败: Connection refused (os error 111)"));
        assert!(!upload_likely_delivered("上传失败: connect timed out"));
    }

    #[test]
    fn finds_folder_skipping_trash_and_documents() {
        let t = tempfile::tempdir().unwrap();
        let w = |n: &str, j: &str| std::fs::write(t.path().join(n), j).unwrap();
        w("a.metadata", r#"{"type":"CollectionType","visibleName":"library","parent":"trash"}"#);
        w("b.metadata", r#"{"type":"DocumentType","visibleName":"library","parent":""}"#);
        w("c.metadata", r#"{"type":"CollectionType","visibleName":"library","parent":""}"#);
        w("d.content", r#"{}"#);
        assert_eq!(find_folder_by_name(t.path(), "library"), Some("c".into()));
        assert_eq!(find_folder_by_name(t.path(), "none"), None);
    }

    #[test]
    fn parent_folder_of_reads_parent_field_and_treats_trash_as_none() {
        let t = tempfile::tempdir().unwrap();
        let w = |n: &str, j: &str| std::fs::write(t.path().join(n), j).unwrap();
        w("book-in-folder.metadata", r#"{"type":"DocumentType","visibleName":"人骨拼图","parent":"folder-uuid"}"#);
        w("book-at-root.metadata", r#"{"type":"DocumentType","visibleName":"飘","parent":""}"#);
        w("book-in-trash.metadata", r#"{"type":"DocumentType","visibleName":"删了","parent":"trash"}"#);
        assert_eq!(parent_folder_of(t.path(), "book-in-folder"), Some("folder-uuid".into()));
        assert_eq!(parent_folder_of(t.path(), "book-at-root"), Some(String::new()), "根目录是空串，不是 None");
        assert_eq!(parent_folder_of(t.path(), "book-in-trash"), None, "书在回收站，别把笔记也生成进去");
        assert_eq!(parent_folder_of(t.path(), "no-such-uuid"), None, "查不到就 None，调用方 best-effort 落根");
    }

    #[test]
    fn unique_document_name_appends_suffix_only_within_same_folder() {
        let t = tempfile::tempdir().unwrap();
        let w = |n: &str, j: &str| std::fs::write(t.path().join(n), j).unwrap();
        w("a.metadata", r#"{"type":"DocumentType","visibleName":"楔子","parent":"f1"}"#);
        w("b.metadata", r#"{"type":"DocumentType","visibleName":"楔子 2","parent":"f1"}"#);
        w("c.metadata", r#"{"type":"DocumentType","visibleName":"楔子","parent":"f2"}"#);
        w("trashed.metadata", r#"{"type":"DocumentType","visibleName":"楔子 3","parent":"trash"}"#);
        assert_eq!(unique_document_name(t.path(), "f1", "楔子"), "楔子 3", "f1 下已有「楔子」和「楔子 2」（后者活着占用），下一个该是 3");
        assert_eq!(unique_document_name(t.path(), "f2", "楔子"), "楔子 2", "f2 只有一份同名，跟 f1 的计数互不影响");
        assert_eq!(unique_document_name(t.path(), "f3", "楔子"), "楔子", "f3 没有同名文档，原样返回");
        assert_eq!(unique_document_name(t.path(), "trash", "楔子 3"), "楔子 3", "回收站里的同名文档不算占用（is_live 过滤掉）");
    }

    #[test]
    fn finds_documents_since_newest_first_and_reads_page_count() {
        let t = tempfile::tempdir().unwrap();
        let w = |n: &str, j: &str| std::fs::write(t.path().join(n), j).unwrap();
        w("old.metadata", r#"{"type":"DocumentType","visibleName":"旧书","parent":"","createdTime":"1000"}"#);
        w("new.metadata", r#"{"type":"DocumentType","visibleName":"New Book","parent":"","createdTime":"3000"}"#);
        w("mid.metadata", r#"{"type":"DocumentType","visibleName":"Mid","parent":"","createdTime":"2000"}"#);
        w("tr.metadata", r#"{"type":"DocumentType","visibleName":"Trash","parent":"trash","createdTime":"5000"}"#);
        w("del.metadata", r#"{"type":"DocumentType","visibleName":"Del","parent":"","deleted":true,"createdTime":"5000"}"#);
        w("dir.metadata", r#"{"type":"CollectionType","visibleName":"Folder","parent":"","createdTime":"5000"}"#);
        w("new.content", r#"{"pageCount": 352, "fileType": "epub"}"#);
        w("mid.content", r#"{"pageCount": 0}"#);
        let docs = find_documents_since(t.path(), 2000);
        assert_eq!(docs.iter().map(|d| d.uuid.as_str()).collect::<Vec<_>>(), ["new", "mid"]);
        assert_eq!(docs[0].visible_name, "New Book");
        assert_eq!(page_count(t.path(), "new"), Some(352));
        assert_eq!(page_count(t.path(), "mid"), None, "0 页＝还没渲染");
        assert_eq!(page_count(t.path(), "old"), None, "没有 .content");
    }
}
