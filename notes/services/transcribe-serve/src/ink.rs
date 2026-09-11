//! 条目库的访问口（只经 ink-serve 的 HTTP，**不直接碰文件**：条目库唯一写者是 ink-serve）。
//! `EntryStore` 抽象出四个动作，生产走注册表找 ink-serve，测试用内存桩。
//!
//! 传输层委托 `rmsvc_core::registry::SvcClient`（2026-09-09 消重复，见该模块文档）；`crop` 要下载
//! 原始字节不是 JSON，用 `SvcClient::agent()` 逃生舱自己发请求。
use notecore::marker::Marker;
use notecore::model::{Book, Draft};
use serde::Deserialize;
use rmsvc_core::paths::Paths;
use rmsvc_core::registry::{enc, SvcClient};
use std::io::Read;

#[derive(Deserialize, Debug, Clone)]
pub struct BookBrief {
    pub uuid: String,
    #[serde(default)]
    pub pending: usize,
}

pub trait EntryStore: Send + Sync {
    fn list_books(&self) -> Result<Vec<BookBrief>, String>;
    fn book(&self, uuid: &str) -> Result<Book, String>;
    fn crop(&self, uuid: &str, file: &str) -> Result<Vec<u8>, String>;
    /// 写回草稿（与可选的行首标记：样式修正，或 `##`/`###` 挂分区/覆盖小节）。ink-serve 保证不覆盖已校对 `text`。
    fn post_draft(&self, uuid: &str, id: &str, draft: &Draft, marker: Option<Marker>) -> Result<(), String>;
}

pub struct InkHttp(SvcClient);

impl InkHttp {
    pub fn new(paths: Paths) -> InkHttp {
        InkHttp(SvcClient::new(paths, "ink-serve", 30))
    }
}

impl EntryStore for InkHttp {
    fn list_books(&self) -> Result<Vec<BookBrief>, String> {
        let v = self.0.get_json("/books")?;
        serde_json::from_value(v.get("items").cloned().unwrap_or_default()).map_err(|e| format!("books 形状不对: {e}"))
    }
    fn book(&self, uuid: &str) -> Result<Book, String> {
        serde_json::from_value(self.0.get_json(&format!("/books/{}", enc(uuid)))?).map_err(|e| format!("book 形状不对: {e}"))
    }
    fn crop(&self, uuid: &str, file: &str) -> Result<Vec<u8>, String> {
        let mut out = Vec::new();
        self.0.agent().get(&format!("{}/books/{}/crops/{}", self.0.base()?, enc(uuid), enc(file))).call().map_err(|e| format!("取裁图 {file}: {e}"))?.into_reader().read_to_end(&mut out).map_err(|e| e.to_string())?;
        Ok(out)
    }
    fn post_draft(&self, uuid: &str, id: &str, draft: &Draft, marker: Option<Marker>) -> Result<(), String> {
        let mut body = serde_json::json!({"draft": draft});
        match marker {
            Some(Marker::Style(s)) => body["style"] = serde_json::to_value(s).unwrap_or_default(),
            Some(Marker::Subhead(name)) => body["subheadHint"] = serde_json::Value::String(name),
            None => {}
        }
        self.0.post_json(&format!("/books/{}/entries/{}", enc(uuid), enc(id)), &body)
    }
}
