//! 落库记录边车（Repository）：母版库每本书旁的隐藏 JSON `.<文件名>.delivered`——各读器最近一次落库的 unix 秒 +
//! 最近一次投原生的渲染自检结果 + （CLI push 洗书产物才有的）原始输入身份。只管"读 / 改 / 删这份记录"，
//! 母版库动作（入库/优化/落库）在 `staging`，自检逻辑在 `render_check`；两边都通过这里落盘，谁也不碰
//! 对方的字段语义（2026-09-06 从 staging.rs 拆出）。
use serde::{Deserialize, Serialize};
use rmsvc_core::fs::write_atomic;
use std::path::{Path, PathBuf};

/// 落库记录：各读器最近一次落库的 unix 秒；`render`=最近一次投原生的渲染自检结果；`source`=这份母版库
/// 文件是由哪个原始输入处理出来的（见下）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Delivered {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub koreader: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub render: Option<RenderCheck>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceRef>,
}

/// "这份母版库文件是由哪个原始输入处理出来的"——host `shelf push` 洗书/重排/转 CBZ 前的原始文件
/// (文件名, 字节数)，上传时随 `?srcName=&srcBytes=` 查询参数带来（2026-09-14 补，见书架白皮书 §04）。
/// 只有 CLI 洗书产物才带；网页原样上传、旧版本 CLI 上传的条目没有这个字段（`None`）。
/// 存在的意义：让下次 `shelf push` 同一份原始文件时，能在**处理之前**（不是处理完的产物之后）就
/// 查到"这份原始输入已经处理成功过"，真正省掉重排/洗书的处理时间，不只是省上传流量——处理产物的
/// 字节数会变（洗书/优化改体积），原始输入的字节数不会，所以判重必须落在这一层，不能只比处理后
/// 产物的 (name, bytes)（那是 `StagingEntry` 本身已有的、独立的另一层判重，处理完之后才用得上）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SourceRef {
    pub name: String,
    pub bytes: u64,
}

/// 渲染自检结果：`status` = pending（等 xochitl 渲染）/ ok / warn（页数远低于期望＝整章渲染失败）/ timeout。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct RenderCheck {
    pub uuid: String,
    pub pages: u64,
    pub expected: u64,
    pub status: String,
    pub at: u64,
}

/// 边车路径：`.<文件名>.delivered`（同目录、隐藏名，母版库列表按点开头跳过）。
pub fn path_for(book: &Path) -> PathBuf {
    let name = book.file_name().and_then(|s| s.to_str()).unwrap_or("book");
    book.with_file_name(format!(".{name}.delivered"))
}

pub fn read(book: &Path) -> Option<Delivered> {
    serde_json::from_slice(&std::fs::read(path_for(book)).ok()?).ok()
}

/// 读—改—原子写。没有边车从空记录起。
pub fn update(book: &Path, f: impl FnOnce(&mut Delivered)) -> Result<(), String> {
    let mut d = read(book).unwrap_or_default();
    f(&mut d);
    let s = serde_json::to_vec(&d).map_err(|e| e.to_string())?;
    write_atomic(&path_for(book), &s).map_err(|e| format!("写落库记录失败: {e}"))
}

/// 删边车（书删了连带删；不存在不算错）。
pub fn remove(book: &Path) {
    let _ = std::fs::remove_file(path_for(book));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_reads_back_and_tolerates_old_records() {
        let t = tempfile::tempdir().unwrap();
        let book = t.path().join("b.epub");
        assert_eq!(path_for(&book).file_name().unwrap(), ".b.epub.delivered");
        assert!(read(&book).is_none());
        update(&book, |d| d.native = Some(7)).unwrap();
        update(&book, |d| d.render = Some(RenderCheck { uuid: "u".into(), pages: 3, expected: 4, status: "ok".into(), at: 1 })).unwrap();
        let d = read(&book).unwrap();
        assert_eq!((d.native, d.koreader), (Some(7), None));
        assert_eq!(d.render.as_ref().map(|r| r.pages), Some(3));
        // 旧版边车（无 render/source 字段）照读
        std::fs::write(path_for(&book), br#"{"native":1,"koreader":2}"#).unwrap();
        assert_eq!(read(&book), Some(Delivered { native: Some(1), koreader: Some(2), render: None, source: None }));
        remove(&book);
        assert!(read(&book).is_none());
        remove(&book);
    }

    #[test]
    fn source_ref_roundtrips_alongside_other_fields() {
        let t = tempfile::tempdir().unwrap();
        let book = t.path().join("b.epub");
        update(&book, |d| d.native = Some(1)).unwrap();
        update(&book, |d| d.source = Some(SourceRef { name: "原始.pdf".into(), bytes: 81_514_344 })).unwrap();
        let d = read(&book).unwrap();
        assert_eq!(d.native, Some(1), "改 source 不该覆盖已有字段");
        assert_eq!(d.source, Some(SourceRef { name: "原始.pdf".into(), bytes: 81_514_344 }));
    }
}
