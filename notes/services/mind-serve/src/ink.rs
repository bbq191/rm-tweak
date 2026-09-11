//! 条目库的访问口（只经 ink-serve 的 HTTP，**不直接碰文件**：条目库唯一写者是 ink-serve）。
//! 比 transcribe-serve 的同名模块简单——不用列书/取裁图（没有批量扫描），只要读一本书（拿到问题所在
//! 那条的书名/章节/勾画/转写文本）和写回 `answer` 两个动作。
//!
//! 传输层（查注册表/建 agent/取 JSON）委托 `rmsvc_core::registry::SvcClient`——2026-09-09 审计发现
//! mind/note/transcribe-serve 这三个 `ink.rs` 此前各自把这层样板重写了一遍，收进共享骨架消重复；
//! `EntryStore` trait 本身（业务方法签名）不变，各服务需要的动作不一样，不该合并。
use notecore::model::{Answer, Book};
use rmsvc_core::paths::Paths;
use rmsvc_core::registry::{enc, SvcClient};

pub trait EntryStore: Send + Sync {
    fn book(&self, uuid: &str) -> Result<Book, String>;
    fn post_answer(&self, uuid: &str, id: &str, answer: &Answer) -> Result<(), String>;
}

pub struct InkHttp(SvcClient);

impl InkHttp {
    pub fn new(paths: Paths) -> InkHttp {
        InkHttp(SvcClient::new(paths, "ink-serve", 30))
    }
}

impl EntryStore for InkHttp {
    fn book(&self, uuid: &str) -> Result<Book, String> {
        let v = self.0.get_json(&format!("/books/{}", enc(uuid)))?;
        serde_json::from_value(v).map_err(|e| format!("book 形状不对: {e}"))
    }
    fn post_answer(&self, uuid: &str, id: &str, answer: &Answer) -> Result<(), String> {
        self.0.post_json(&format!("/books/{}/entries/{}", enc(uuid), enc(id)), &serde_json::json!({"answer": answer}))
    }
}
