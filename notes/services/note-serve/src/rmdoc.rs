//! 打包一份 xochitl 认的原生文档 zip（reMarkable 官方叫 `.rmdoc`）：`<uuid>.metadata` +
//! `<uuid>.content` + `<uuid>/<page-uuid>.rm`。**格式不是自己猜的**：`reading/protocol/inject.py`
//! （2026-08-16 真机验证）+ `device-core/src/inject.rs` + `knowledge/pkm/src/cardnote.rs` 都验证过
//! `POST /upload`（`rmsvc_core::xochitl::Xochitl::upload`，本模块直接复用这个共享底座，不是"旧代码"）
//! 除 EPUB/PDF 外也吃 `.rmdoc`：multipart 字段名 `file`、`.rmdoc` 用 `application/zip`；导入端**会
//! 重新分配设备 UUID**（不是包里写的那个），落库后免重启出现，调用方按 `visibleName` 事后认领。
//! 按"不引入任何旧代码到 notes/"的红线，本模块是照这些事实全新写的，不是搬运（笔记线白皮书 §03h）。
//!
//! 首期只做"一章一页"：一份文档正好一页，`.content` 走当前固件（3.28）的 `cPages`/`formatVersion 2`
//! 结构，字段集合参照真机样本 `testdata/seven_styles/book.content` 与上面三处旧代码的最小可用集
//! （`extraMetadata` 可以是空对象——不用把所有画笔工具状态字段都填一遍）。
//!
//! ✅ 打包器本身已经 host 双实现（zipfile+rmscene）交叉验证过，且 **2026-09-07 真机验证通过**（笔记线
//! 白皮书 §03h：6 段 5 样式测试文档传到真机、xochitl 自己渲染的缩略图肉眼核对全对）。
//!
//! `TEMPLATE`/`TEMPLATE_AUTHOR` 不只是测试用的——生产投影（`publish.rs`）复用同一份真机模板：模板里
//! `RootTextBlock` 之外的块（纸张大小/场景树等）跟"页面上打的是什么字"无关，从这份已知能被 xochitl
//! 正常打开的真机文件里原样复用最稳（见 `rmv6::write` 模块文档"为什么是模板替换"）。
use std::io::Write;
use zip::write::SimpleFileOptions;
use zip::CompressionMethod;

/// 每份生成的原生笔记本文档共用的真机模板：一份最小的、xochitl 打开正常的单页 `.rm`；
/// 生成时只替换其中的 `RootTextBlock`（`rmv6::write::build_page_rm`）。
pub const TEMPLATE: &[u8] = include_bytes!("../../../testdata/seven_styles/page.rm");
/// 模板 `AuthorIdsBlock` 里声明的作者 uuid——`build_page_rm` 复用模板的其它块，写入的 `RootTextBlock`
/// 引用的作者索引必须跟它一致，这里固定用模板自己的值。
pub const TEMPLATE_AUTHOR: &str = "94980865-163a-5b59-a2d1-cd702a59e989";

pub struct Page {
    pub uuid: String,
    pub rm_bytes: Vec<u8>,
}

/// 一份最小合规的 `.content`（`fileType:"notebook"`，一页）。
fn content_json(page_uuid: &str, author_uuid: &str, now_ms: u64, size_bytes: usize) -> serde_json::Value {
    serde_json::json!({
        "cPages": {
            "lastOpened": {"timestamp": "1:1", "value": page_uuid},
            "original": {"timestamp": "0:0", "value": -1},
            "pages": [{
                "id": page_uuid,
                "idx": {"timestamp": "1:2", "value": "ba"},
                "modifed": now_ms.to_string(),
                "template": {"timestamp": "1:2", "value": "Blank"},
            }],
            "uuids": [{"first": author_uuid, "second": 1}],
        },
        "coverPageNumber": -1,
        "customZoomCenterX": 0,
        "customZoomCenterY": 936,
        "customZoomOrientation": "portrait",
        "customZoomPageHeight": 1872,
        "customZoomPageWidth": 1404,
        "customZoomScale": 1,
        "documentMetadata": {},
        "extraMetadata": {},
        "fileType": "notebook",
        "fontName": "",
        "formatVersion": 2,
        "lineHeight": 100,
        "orientation": "portrait",
        "pageCount": 1,
        "pageTags": [],
        "sizeInBytes": size_bytes.to_string(),
        "tags": [],
        "textAlignment": "left",
        "textScale": 1,
        "zoomMode": "bestFit",
    })
}

fn metadata_json(visible_name: &str, now_ms: u64, parent: &str) -> serde_json::Value {
    serde_json::json!({
        "createdTime": now_ms.to_string(),
        "lastModified": now_ms.to_string(),
        "lastOpened": "0",
        "lastOpenedPage": 0,
        "new": false,
        "parent": parent,
        "pinned": false,
        "source": "",
        "type": "DocumentType",
        "visibleName": visible_name,
    })
}

/// 打包一份单页原生文档。`doc_uuid` 只是包内占位（导入端会换新的，见模块文档），`author_uuid`
/// 必须跟 `page.rm_bytes` 里 `AuthorIdsBlock` 声明的作者一致（用模板拼页时，用模板自己的作者）。
pub fn pack(doc_uuid: &str, visible_name: &str, parent: &str, page: &Page, author_uuid: &str, now_ms: u64) -> Result<Vec<u8>, String> {
    let content = content_json(&page.uuid, author_uuid, now_ms, page.rm_bytes.len());
    let metadata = metadata_json(visible_name, now_ms, parent);

    let mut out = Vec::new();
    {
        let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut out));
        let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        z.start_file(format!("{doc_uuid}.metadata"), opts).map_err(|e| e.to_string())?;
        z.write_all(serde_json::to_string(&metadata).map_err(|e| e.to_string())?.as_bytes()).map_err(|e| e.to_string())?;
        z.start_file(format!("{doc_uuid}.content"), opts).map_err(|e| e.to_string())?;
        z.write_all(serde_json::to_string(&content).map_err(|e| e.to_string())?.as_bytes()).map_err(|e| e.to_string())?;
        z.start_file(format!("{doc_uuid}/{}.rm", page.uuid), opts).map_err(|e| e.to_string())?;
        z.write_all(&page.rm_bytes).map_err(|e| e.to_string())?;
        z.finish().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmv6::write::{build_page_rm, Paragraph};
    use rmv6::v6::scene_item::text::ParagraphStyle;

    #[test]
    fn packs_a_valid_zip_with_three_entries() {
        let rm = build_page_rm(TEMPLATE, &[Paragraph::new(ParagraphStyle::HEADING, "第一章")]).unwrap();
        let page = Page { uuid: "1111".into(), rm_bytes: rm };
        let bytes = pack("doc-uuid", "《测试书》第一章", "", &page, TEMPLATE_AUTHOR, 1_700_000_000_000).unwrap();

        let mut z = zip::ZipArchive::new(std::io::Cursor::new(&bytes)).unwrap();
        let names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_string()).collect();
        assert_eq!(names, vec!["doc-uuid.metadata", "doc-uuid.content", "doc-uuid/1111.rm"]);

        let mut meta = String::new();
        std::io::Read::read_to_string(&mut z.by_name("doc-uuid.metadata").unwrap(), &mut meta).unwrap();
        let meta: serde_json::Value = serde_json::from_str(&meta).unwrap();
        assert_eq!(meta["visibleName"], "《测试书》第一章");
        assert_eq!(meta["type"], "DocumentType");

        let mut content = String::new();
        std::io::Read::read_to_string(&mut z.by_name("doc-uuid.content").unwrap(), &mut content).unwrap();
        let content: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert_eq!(content["fileType"], "notebook");
        assert_eq!(content["pageCount"], 1);
        assert_eq!(content["cPages"]["pages"][0]["id"], "1111");
    }

    /// 不是真正的单测——一次性生成真机验证用的测试文档，落到 scratchpad 供手动 scp+上传。
    /// `CANGJIE_DUMP_RMDOC=<目录> cargo test -p note-serve -- --ignored dump_device_test_doc`
    #[test]
    #[ignore]
    fn dump_device_test_doc() {
        let Ok(dir) = std::env::var("CANGJIE_DUMP_RMDOC") else { return };
        let paragraphs = vec![
            Paragraph::new(ParagraphStyle::HEADING, "笔记线真机测试二轮"),
            Paragraph::subheading1("真 Subheading 1（带 7 字节标记，应大字号）"),
            Paragraph::new(ParagraphStyle::BOLD, "裸 BOLD＝Subheading 2（不带标记，应小字号）"),
            Paragraph::new(ParagraphStyle::PLAIN, "正文样式，验证 rmv6::write 二轮生成的 RootTextBlock。"),
            Paragraph::new(ParagraphStyle::BULLET, "无序要点"),
            Paragraph::new(ParagraphStyle::NUMBERED, "有序要点一"),
            Paragraph::new(ParagraphStyle::NUMBERED, "有序要点二"),
            Paragraph::new(ParagraphStyle::CHECKBOX, "待办事项：确认样式渲染正常"),
        ];
        let rm = build_page_rm(TEMPLATE, &paragraphs).unwrap();
        let doc_uuid = uuid::Uuid::new_v4().to_string();
        let page_uuid = uuid::Uuid::new_v4().to_string();
        let page = Page { uuid: page_uuid, rm_bytes: rm };
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as u64;
        let bytes = pack(&doc_uuid, "cangjie 笔记线真机测试二轮", "", &page, TEMPLATE_AUTHOR, now).unwrap();
        std::fs::write(std::path::Path::new(&dir).join("device_test.rmdoc"), &bytes).unwrap();
        eprintln!("wrote device_test.rmdoc, {} bytes, doc_uuid={doc_uuid}", bytes.len());
    }
}
