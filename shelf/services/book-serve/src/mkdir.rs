//! 原生书库「建文件夹」队列：外部进程不能直接建文件夹（跟不能直改 `.metadata` 一样，唯一合法路是
//! xochitl 自己的代码路 `Library.createCollection(parentId, name)`）——由注入 MainView 的
//! `shelf/xovi/shelf-mkdir-agent.qmd` 定时拉 `GET /mkdir/pending` 执行。本模块只管队列：`add()` 入队去重，
//! `pending()` 顺手把已经真实存在的文件夹名剔除（QML 端无需 ack，跟 `trash.rs` 的剔除方式对称）。
//! 队列文件 `$XDG_STATE_HOME/shelf/books/mkdir-pending.json`。
//!
//! **2026-09-19 复活**：这套队列+qmd 代理 2026-09-07 最早是给 `note-serve` 生成《书名》一章一本用的，
//! 2026-09-09 note-serve 改成复用书本自己的设备文件夹后没了消费方，2026-09-15 被当死代码物理删除。
//! 这次真正的消费方是网页母版库「加入 xochitl → 文件夹」自由输入框（2026-09-19 用户反馈"填个文件夹
//! 名依然不会创建文件夹"）——`staging::Staging::deliver` 落库前调用，folder 不存在就入队，同步等
//! （`fswatch::watch_until`）agent 真的建出来再继续投递，见 `staging.rs::ensure_folder`。代码本身
//! `git show <删除前的 commit>^:...` 原样捞回，逻辑没变——当年写的时候就已经想清楚了，只是一直没等到
//! 真消费方。**`Library.createCollection` 这条调用链当年只做到"反编译 + 静态调用链一致"，从没有真机
//! 点过新建文件夹按钮做交叉验证**（见 `shelf-mkdir-agent.qmd` 头注原样保留的踩坑记录），这次借着
//! 有了真消费方顺手把这层最后验证补上。
//!
//! 持久化+入队去重+剔除这层通用外壳委托 `pending_queue::PendingQueue<T>`（2026-09-09 消重复，跟
//! `trash.rs` 是同一份基础设施，见该模块文档）；这里只留领域校验（名字合法性/文件夹是否已存在）。
use crate::pending_queue::PendingQueue;
use serde::{Deserialize, Serialize};
use rmsvc_core::xochitl::find_folder_by_name;
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pending {
    pub name: String,
    pub at: u64,
}

pub struct MkdirQueue {
    q: PendingQueue<Pending>,
    lib_dir: PathBuf,
}

impl MkdirQueue {
    pub fn new(state_books_dir: &Path, lib_dir: &Path) -> MkdirQueue {
        MkdirQueue { q: PendingQueue::new(state_books_dir.join("mkdir-pending.json")), lib_dir: lib_dir.to_path_buf() }
    }

    /// 入队一个文件夹名；已经真实存在或已在队列里都不重复加。名字不能为空——`/`、`\` 曾经也被当
    /// "路径分隔符防误传"拦掉，2026-09-19 真机反馈坐实是误伤：这个名字全程只当 JSON `visibleName`
    /// 字符串走（`Library.createCollection(parentId, name)` 收的是普通 JS 字符串，不是文件系统路径，
    /// 本模块也不支持"按路径建多级文件夹"这种语义），真实书名/文件夹名带斜杠很常见（如《乱马1/2》），
    /// 拦它没有技术依据、只会挡合法输入——见 `find_folder_by_name`/`Xochitl::upload` 全程都是按
    /// `visibleName` 字符串整体比较，folder 的文件系统路径只走 uuid，从不落到名字里。
    pub fn add(&self, name: &str) -> Result<usize, String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("文件夹名不能为空".into());
        }
        if find_folder_by_name(&self.lib_dir, name).is_some() {
            return Ok(0); // 已经存在，不用建
        }
        self.q.add(|p| p.name == name, || Pending { name: name.to_string(), at: rmsvc_core::clock::now_secs() })
    }

    /// 待办文件夹名（QML 代理拉取）：顺手清掉已经真实建出来的（QML 端无需 ack）。
    /// 返回 (待办文件夹名列表, 本次清掉几条)。
    pub fn pending(&self) -> Result<(Vec<String>, usize), String> {
        let (kept, pruned) = self.q.prune(|p| find_folder_by_name(&self.lib_dir, &p.name).is_none())?;
        Ok((kept.into_iter().map(|p| p.name).collect(), pruned))
    }

    pub fn list(&self) -> Vec<Pending> {
        self.q.list()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(t: &tempfile::TempDir) -> PathBuf {
        let d = t.path().join("xochitl");
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn add_rejects_bad_names_and_existing_folders_then_pending_prunes_created() {
        let t = tempfile::tempdir().unwrap();
        let lib_dir = lib(&t);
        let q = MkdirQueue::new(&t.path().join("state"), &lib_dir);

        assert!(q.add("").unwrap_err().contains("不能为空"));

        assert_eq!(q.add("《人骨拼圖》").unwrap(), 1);
        assert_eq!(q.add("《人骨拼圖》").unwrap(), 1, "重复入队不翻倍");
        assert_eq!(q.add(" 《人骨拼圖》 ").unwrap(), 1, "trim 后同名，不重复入队"); // add() trims name first

        assert_eq!(q.add("《消失的爱人》").unwrap(), 2);

        let (names, pruned) = q.pending().unwrap();
        assert_eq!((names.len(), pruned), (2, 0));

        // QML 代理把《人骨拼圖》真的建出来了
        std::fs::write(lib_dir.join("f1.metadata"), r#"{"type":"CollectionType","visibleName":"《人骨拼圖》","parent":""}"#).unwrap();
        let (names, pruned) = q.pending().unwrap();
        assert_eq!((names, pruned), (vec!["《消失的爱人》".to_string()], 1));
        assert_eq!(q.list().len(), 1);

        // 已存在的文件夹再入队直接判"不用建"，不落队列
        assert_eq!(q.add("《人骨拼圖》").unwrap(), 0);
        assert_eq!(q.list().len(), 1, "没有新增");
    }

    /// 2026-09-19 真机反馈：《乱马1/2》这类带 `/` 的真实文件夹名曾被当"路径分隔符防误传"拒绝，
    /// 导致 `ensure_folder` 静默放弃、书落回根目录——这个名字全程只当 JSON `visibleName` 字符串走
    /// （`Library.createCollection` 收的是普通 JS 字符串，不是文件系统路径），拦它没有技术依据。
    #[test]
    fn add_accepts_names_with_slash() {
        let t = tempfile::tempdir().unwrap();
        let lib_dir = lib(&t);
        let q = MkdirQueue::new(&t.path().join("state"), &lib_dir);
        assert_eq!(q.add("乱马1/2").unwrap(), 1);
        assert_eq!(q.list()[0].name, "乱马1/2");
        assert_eq!(q.add(r"a\b").unwrap(), 2);
    }
}
