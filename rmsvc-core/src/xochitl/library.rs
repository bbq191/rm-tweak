//! 原生书库的**只读元数据模型**：`<uuid>.metadata`/`.content` 的查询（找文件夹、列文件夹、文档去重命名、
//! 按创建时间圈"刚进库的那本"、渲染页数）。纯文件系统读取，不碰 HTTP——和上传客户端（父模块 [`super::Xochitl`]）
//! 分开，各自单一职责；对外路径仍是 `rmsvc_core::xochitl::*`（父模块 `pub use` 再导出）。
use std::path::Path;

/// 书库目录里所有可解析的 `<uuid>.metadata` → (uuid, JSON)。只读；解析失败的跳过。
fn metadata_entries(dir: &Path) -> Vec<(String, serde_json::Value)> {
    metadata_entries_since(dir, None)
}

/// 同 [`metadata_entries`]，`min_mtime` 给定时**只打开 mtime 不早于它的**：先用目录项自带的 stat 挡掉旧文件，
/// 不再对整个书库（几十上百份）逐个 open+读+解析 JSON——渲染自检/占位等待这类"找刚进库的那本"的调用会在
/// 一个 3 秒防抖 / 200ms 轮询循环里反复扫描（2026-09-22 审计）。
fn metadata_entries_since(dir: &Path, min_mtime: Option<std::time::SystemTime>) -> Vec<(String, serde_json::Value)> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    rd.flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("metadata") {
                return None;
            }
            if let Some(floor) = min_mtime {
                if e.metadata().ok()?.modified().ok()? < floor {
                    return None;
                }
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

/// 是不是 xochitl 文档 uuid 的形状（36 字符，只含十六进制与 `-`）。拿来当文件名片段之前先过一遍，
/// 防路径注入（`../`）；只看形状，不代表书库里真有这份文档。
pub fn is_uuid_shape(s: &str) -> bool {
    s.len() == 36 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

pub fn find_folder_by_name(dir: &Path, name: &str) -> Option<String> {
    metadata_entries(dir).into_iter().find(|(_, v)| str_of(v, "type") == "CollectionType" && is_live(v) && str_of(v, "visibleName") == name).map(|(uuid, _)| uuid)
}

/// 原生书库里所有活文件夹的名字（去重、按名排序）——给网页「加入原生书库 → 文件夹」下拉候选用，
/// 跟 koreader-serve 给 KOReader 目录下拉候选同一个道理：反映设备上**真实存在**的文件夹，不是
/// 写死的预设列表（2026-09-19 用户反馈：原来的「书库/批注/自定义」三选一预设看不出真实文件夹，
/// 批注那档还常年跟书库撞成一样，见书架白皮书对应记录）。
pub fn list_folders(dir: &Path) -> Vec<String> {
    metadata_entries(dir)
        .into_iter()
        .filter(|(_, v)| str_of(v, "type") == "CollectionType" && is_live(v))
        .map(|(_, v)| str_of(&v, "visibleName").to_string())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
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
    // `createdTime >= since` 的文档，其 `.metadata` 一定是创建时或之后写的，mtime 不会更早（留 5 秒余量给文件系统
    // 时间戳粒度/时钟取整）；`since_ms == 0`（补记全库）不设下限。
    let floor = (since_ms > 0).then(|| std::time::UNIX_EPOCH + std::time::Duration::from_millis(since_ms.saturating_sub(5_000)));
    let mut out: Vec<DocInfo> = metadata_entries_since(dir, floor)
        .into_iter()
        .filter(|(_, v)| str_of(v, "type") == "DocumentType" && is_live(v))
        .filter_map(|(uuid, v)| {
            let created_ms = v.get("createdTime").and_then(|x| x.as_str().and_then(|s| s.parse::<u64>().ok()).or_else(|| x.as_u64())).unwrap_or(0);
            (created_ms >= since_ms).then(|| DocInfo { uuid, visible_name: str_of(&v, "visibleName").to_string(), created_ms })
        })
        .collect();
    out.sort_by_key(|d| std::cmp::Reverse(d.created_ms));
    out
}

/// `<uuid>.content` 的 `pageCount`：xochitl 渲染完（导入 / 打开）才写；缺或 0 → None。
pub fn page_count(dir: &Path, uuid: &str) -> Option<u64> {
    let t = std::fs::read_to_string(dir.join(format!("{uuid}.content"))).ok()?;
    let v: serde_json::Value = serde_json::from_str(&t).ok()?;
    v.get("pageCount").and_then(|x| x.as_u64()).filter(|&n| n > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uuid_shape_accepts_real_uuid_rejects_traversal() {
        assert!(is_uuid_shape("0a1b2c3d-4e5f-6789-abcd-ef0123456789"));
        assert!(!is_uuid_shape("../../etc/passwd"));
        assert!(!is_uuid_shape("0a1b2c3d-4e5f-6789-abcd-ef012345678"), "少一位");
        assert!(!is_uuid_shape("0a1b2c3d-4e5f-6789-abcd-ef012345678g"), "非十六进制");
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
    fn lists_folders_deduped_sorted_skipping_trash_and_documents() {
        let t = tempfile::tempdir().unwrap();
        let w = |n: &str, j: &str| std::fs::write(t.path().join(n), j).unwrap();
        w("a.metadata", r#"{"type":"CollectionType","visibleName":"雪人","parent":""}"#);
        w("b.metadata", r#"{"type":"CollectionType","visibleName":"批注","parent":""}"#);
        w("c.metadata", r#"{"type":"CollectionType","visibleName":"批注","parent":""}"#); // 同名文件夹去重
        w("d.metadata", r#"{"type":"CollectionType","visibleName":"回收站里的","parent":"trash"}"#);
        w("e.metadata", r#"{"type":"DocumentType","visibleName":"这是本书不是文件夹","parent":""}"#);
        assert_eq!(list_folders(t.path()), vec!["批注".to_string(), "雪人".to_string()]);
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

    /// `find_documents_since` 用 mtime 下限先挡旧文件：结果语义不变（仍以 createdTime 为准），只是不再打开旧文件。
    #[test]
    fn find_documents_since_skips_stale_metadata_by_mtime_but_keeps_result_semantics() {
        let t = tempfile::tempdir().unwrap();
        let now = crate::clock::now_ms();
        let w = |n: &str, created: u64, age_secs: u64| {
            let p = t.path().join(n);
            std::fs::write(&p, format!(r#"{{"type":"DocumentType","visibleName":"{n}","parent":"","createdTime":"{created}"}}"#)).unwrap();
            let f = std::fs::File::options().write(true).open(&p).unwrap();
            f.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs)).unwrap();
        };
        w("fresh.metadata", now, 0);
        w("shelved.metadata", now - 3_600_000, 3_600); // 一小时前进库、没再动过
        w("touched.metadata", now - 3_600_000, 0); // 老书但刚被 xochitl 改写过 .metadata：过 mtime 门，被 createdTime 挡掉
        let got: Vec<String> = find_documents_since(t.path(), now - 1_000).into_iter().map(|d| d.uuid).collect();
        assert_eq!(got, ["fresh"]);
        let all: Vec<String> = find_documents_since(t.path(), 0).into_iter().map(|d| d.uuid).collect();
        assert_eq!(all.len(), 3, "since=0（补记全库）不设 mtime 下限");
    }
}
