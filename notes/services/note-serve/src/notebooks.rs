//! 记"每本书每章上次生成了哪份设备文档"：一书一文件 `$XDG_STATE_HOME/notes/notebooks/<uuid>.json`。
//! 存取逻辑见 `crate::chapter_store::ChapterStore`（跟 `export_state.rs` 共用同一个泛型，这里只定义
//! 自己的记录形状）。
use crate::chapter_store::ChapterStore;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChapterRecord {
    pub doc_uuid: String,
    /// 生成时写进 `.metadata` 的 visibleName——旧版本要排回收站时，`book-serve` 按这个名字核对 uuid。
    pub visible_name: String,
    pub fingerprint: String,
    pub generated_at: u64,
}

pub type NotebookState = ChapterStore<ChapterRecord>;

#[cfg(test)]
mod tests {
    use super::*;

    /// 泛型机制本身在 `chapter_store.rs` 里已经测过（含真机记录形状回归）；这里只补一个用真实
    /// `ChapterRecord`/`NotebookState` 类型别名走一遍的冒烟测试，确认类型别名接线没接错。
    #[test]
    fn notebook_state_set_and_get_with_real_record_type() {
        let t = tempfile::tempdir().unwrap();
        let st = NotebookState::new(t.path().to_path_buf());
        let rec = ChapterRecord { doc_uuid: "d1".into(), visible_name: "第1章".into(), fingerprint: "fp1".into(), generated_at: 100 };
        st.set("b1", 0, rec.clone()).unwrap();
        assert_eq!(st.get("b1", 0), Some(rec));
    }
}
