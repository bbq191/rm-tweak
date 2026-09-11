//! 记"每本书每章上次导出 md 时的内容指纹"：一书一文件 `$XDG_STATE_HOME/notes/exports/<uuid>.json`。
//! 跟 `notebooks.rs` 共用同一个存取泛型（见 `crate::chapter_store::ChapterStore`），这里只定义自己的
//! 记录形状——三期落盘导出一直是"每次全量重写"，没有跳过逻辑；整理区第三轮反馈（用户追问"生成完成
//! 后是不是应该移出列表"）要求"整理"页能分辨"这一章已经跟当前内容同步了"，落设备笔记本那边本来就有
//! 这份记账（`notebooks::ChapterRecord.fingerprint`），导出这边一直没有——现在补上，两条投影路径用
//! 同一套"指纹没变就跳过+记账"纪律。
use crate::chapter_store::ChapterStore;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExportRecord {
    pub fingerprint: String,
    pub exported_at: u64,
}

pub type ExportState = ChapterStore<ExportRecord>;

#[cfg(test)]
mod tests {
    use super::*;

    /// 泛型机制（含 clear）在 `chapter_store.rs` 已经测过；这里补一个用真实 `ExportRecord`/
    /// `ExportState` 类型别名走一遍的冒烟测试。
    #[test]
    fn export_state_set_get_clear_with_real_record_type() {
        let t = tempfile::tempdir().unwrap();
        let st = ExportState::new(t.path().to_path_buf());
        let rec = ExportRecord { fingerprint: "fp1".into(), exported_at: 100 };
        st.set("b1", 0, rec.clone()).unwrap();
        assert_eq!(st.get("b1", 0), Some(rec));
        st.clear("b1", 0).unwrap();
        assert!(st.get("b1", 0).is_none());
    }
}
