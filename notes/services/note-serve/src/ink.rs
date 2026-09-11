//! 条目库的只读访问口（本服务只读，不改字段——改字段是 ink-serve/transcribe-serve/mind-serve 的事）。
//! 生产走注册表找 ink-serve；测试用内存桩。同款套路见 `transcribe-serve::ink`。
//!
//! 传输层委托 `rmsvc_core::registry::SvcClient`（2026-09-09 消重复，见该模块文档）。
use notecore::model::Book;
use rmsvc_core::paths::Paths;
use rmsvc_core::registry::{enc, SvcClient};

#[derive(serde::Deserialize, Debug, Clone)]
pub struct BookBrief {
    pub uuid: String,
}

pub trait EntryStore: Send + Sync {
    fn list_books(&self) -> Result<Vec<BookBrief>, String>;
    fn book(&self, uuid: &str) -> Result<Book, String>;
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
}
