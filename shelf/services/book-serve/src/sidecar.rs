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
    /// 最近一次「优化」的结果（`staging::Staging::spawn_optimize` 异步执行时写）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optimize: Option<OptimizeCheck>,
    /// 最近一次「落库」的结果（`staging::Staging::spawn_deliver` 异步执行时写，2026-09-19）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deliver: Option<DeliverCheck>,
    /// 按书设置的翻页方向（2026-09-25）：`"rtl"`＝从右往左（日漫）/ `"ltr"`＝从左往右；缺省＝自动（保留书里自带的
    /// OPF 标记）。「优化」时写进 OPF spine（`bookconv::direction`），已落库的副本按 `render.uuid` 同步进
    /// `rtl-overrides.json`（`reading_direction.rs`）。**用户的设置，不是结果记录**——放边车是因为它跟着这本书走
    /// （改名一起挪、删书一起删），与其它字段互不影响。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
}

/// 异步优化的结果：`status` = pending（后台线程跑着）/ ok / failed。`message` 是回执文案
/// （成功＝跟原同步接口一样的"已优化《...》（...）"；失败＝错误原因）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct OptimizeCheck {
    pub status: String,
    pub message: String,
    pub at: u64,
    /// 条目级进度（已处理/总条目数，不是字节）——`optimize_epub_file_streaming` 阶段二逐条目写出
    /// 时回调（2026-09-19 用户反馈"进度条一直感觉不会动"，查明根因是 `OptimizeCheck` 从没有过
    /// 这个字段，不管书是不是漫画、流不流式都没有分步进度可报）。跟 [`DeliverCheck::progress`] 同
    /// 一个 [`StepProgress`] 类型，用途一致就不重复定义结构体。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<StepProgress>,
}

/// 异步落库的结果：`status` = pending（后台线程跑着）/ ok / failed。`message` 是回执文案（成功＝跟
/// 原同步接口一样的"已投入原生书库《...》"；失败＝错误原因）。跟 [`OptimizeCheck`] 字段形状一样，
/// 分开成两个类型是因为它们是两件独立的事——不想靠一个字段名影射来区分"这次记录的到底是优化还是
/// 落库"。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct DeliverCheck {
    pub status: String,
    pub message: String,
    pub at: u64,
    /// 超限漫画按卷拆分投递时才有的结构化进度（份数，不是字节）——`message` 一直是给人读的一句话，
    /// 这个字段是给网页画进度条用的数字（2026-09-19 用户反馈"正在处理中请稍候"这种静态文案该换成
    /// 进度条/百分比）。非拆分路径（普通整本落库）没有这个字段，网页据此判断走"有精确进度的百分比
    /// 条"还是"不确定要多久的滚动条"。`total` 只数"预算内、真会尝试上传"的份数——拆到底仍超限、
    /// 注定不投的那几份不计入分母，所以能上传的那些传完百分比就会到 100%，不会因为几份铁定失败的
    /// 卡在中间；那几份的存在与否体现在最终回执文案的"N 未投"里，不影响这个进度条。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<StepProgress>,
}

/// 分步进度：`done`＝已经完成的步数，`total`＝这次操作总共会有多少步。两个消费方：漫画拆分投递
/// （一步＝一份成功上传+等到渲染确认，见 [`DeliverCheck::progress`]）、EPUB 优化（一步＝阶段二
/// 写出一个条目，见 [`OptimizeCheck::progress`]）——形状完全一样，共用一个类型（2026-09-19 加
/// 优化进度时把原来专属落库的 `DeliverProgress` 改名成这个通用名，字段/JSON 序列化形状不变）。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct StepProgress {
    pub done: u32,
    pub total: u32,
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
///
/// 全局互斥：优化进度回调、渲染自检线程、HTTP 线程会并发改同一份边车，`write_atomic` 只保证文件不写一半、
/// 不保证不丢更新（A 读→B 读→A 写→B 写，A 的字段没了）。边车都很小、写得不频繁，一把全局锁足够。
pub fn update(book: &Path, f: impl FnOnce(&mut Delivered)) -> Result<(), String> {
    static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = rmsvc_core::sync::lock(&WRITE);
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
        assert_eq!(read(&book), Some(Delivered { native: Some(1), koreader: Some(2), render: None, source: None, optimize: None, deliver: None, direction: None }));
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
