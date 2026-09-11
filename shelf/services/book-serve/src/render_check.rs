//! 投原生后的**渲染自检**：xochitl 导入 EPUB 后渲染（真机：导入当下同步渲染），渲染完在 `<uuid>.content` 写 `pageCount`。
//! 整章渲染失败（同一标签双 id 等，《消失的爱人》只出 7 页）以前要用户翻到才发现；现在投书后起一条线程，
//! **限时**监听书库目录（`fswatch::watch_until`，最长 [`TIMEOUT`]，结束即撤、不常驻），等到页数就与
//! `bookconv::stats` 的期望页数比：低于 [`WARN_RATIO`] → `warn`。结果写进母版库边车 `.<name>.delivered` 的
//! `render` 字段（事件是有损信号，状态必须落盘），并推 `books/render` 事件（带 name/status/pages）。
//! 认书：`/upload` 不回 uuid，visibleName 取自 EPUB 元数据不等于文件名 → 按 `createdTime >= 投书时刻` 圈候选，
//! 书名（dc:title / 文件名 stem）相符者优先，否则取最新一本。只读 `.metadata/.content`，绝不写 xochitl 目录。
use crate::sidecar::RenderCheck;
use crate::staging::{RenderPlan, Staging};
use rmsvc_core::clock::now_secs as now;
use rmsvc_core::events::EventBus;
use rmsvc_core::fswatch::watch_until;
use rmsvc_core::xochitl::{find_documents_since, page_count, DocInfo};
use std::path::Path;
use std::time::Duration;

/// 等渲染的上限；超时状态 `timeout`（不是错误：xochitl 可能延后渲染，设备上打开一次就有）。
pub const TIMEOUT: Duration = Duration::from_secs(600);
/// 书库目录写入防抖（xochitl 导入时连写 metadata/content/缩略图）。
pub const DEBOUNCE: Duration = Duration::from_secs(3);
/// 实际页数 / 期望页数低于此 → warn。自检发生在导入当下，xochitl 用的是缺省字号/边距（`.content` 里 textScale 1 /
/// margins 56），页数只随书的文字密度浮动（真机：两本真书 0.99，随机词探针 0.86）；坏章在 xochitl 里各占 1 页空白，
/// 4 章坏 3 章的探针 10/29=0.34——30% 抓不住，定 50%（2026-09-06 真机标定）。
pub const WARN_RATIO: f64 = 0.5;

pub fn run(staging: &Staging, bus: &EventBus, lib_dir: &Path, plan: &RenderPlan) {
    let write = |uuid: &str, pages: u64, status: &str| {
        let rc = RenderCheck { uuid: uuid.to_string(), pages, expected: plan.expected, status: status.to_string(), at: now() };
        if let Err(e) = staging.set_render(&plan.name, rc) {
            println!("[book-serve] 渲染自检记录《{}》失败: {e}", plan.name);
        }
        bus.publish_raw(&serde_json::json!({"area":"books","kind":"render","name":plan.name,"status":status,"pages":pages,"expected":plan.expected,"at":now()}).to_string());
        println!("[book-serve] 渲染自检《{}》: {status} pages={pages} expected={} uuid={uuid}", plan.name, plan.expected);
    };
    write("", 0, "pending");
    let check = || probe(lib_dir, plan).map(|(uuid, pages)| write(&uuid, pages, verdict(pages, plan.expected)));
    if check().is_some() {
        return;
    }
    let found = watch_until(lib_dir, DEBOUNCE, TIMEOUT, |_| check().is_some());
    if !found && check().is_none() {
        write("", 0, "timeout");
    }
}

/// 在书库里找这本刚投的书并读页数；没渲染完 → None。
pub fn probe(lib_dir: &Path, plan: &RenderPlan) -> Option<(String, u64)> {
    let docs = find_documents_since(lib_dir, plan.since_ms);
    let doc = pick(&docs, plan.title.as_deref(), &plan.name)?;
    page_count(lib_dir, &doc.uuid).map(|n| (doc.uuid.clone(), n))
}

/// 候选（新→旧）里挑：visibleName 与 dc:title / 文件名 stem 相符（忽略大小写）者优先，否则最新一本。
pub fn pick<'a>(docs: &'a [DocInfo], title: Option<&str>, name: &str) -> Option<&'a DocInfo> {
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    let eq = |a: &str, b: &str| a.trim().eq_ignore_ascii_case(b.trim());
    docs.iter().find(|d| title.is_some_and(|t| eq(&d.visible_name, t)) || eq(&d.visible_name, stem)).or(docs.first())
}

pub fn verdict(pages: u64, expected: u64) -> &'static str {
    if expected > 0 && (pages as f64) < (expected as f64) * WARN_RATIO {
        "warn"
    } else {
        "ok"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(uuid: &str, name: &str, t: u64) -> DocInfo {
        DocInfo { uuid: uuid.into(), visible_name: name.into(), created_ms: t }
    }

    #[test]
    fn picks_by_title_then_stem_then_newest() {
        let docs = vec![d("n", "Other", 3), d("t", "tell me your dreams", 2), d("s", "Tell Me Your Dreams (v6)", 1)];
        assert_eq!(pick(&docs, Some("Tell Me Your Dreams"), "Tell Me Your Dreams (v6).epub").unwrap().uuid, "t", "dc:title 优先（忽略大小写）");
        assert_eq!(pick(&docs, None, "Tell Me Your Dreams (v6).epub").unwrap().uuid, "s", "退而按文件名 stem");
        assert_eq!(pick(&docs, Some("没有的"), "x.epub").unwrap().uuid, "n", "都不符取最新");
        assert!(pick(&[], Some("a"), "a.epub").is_none());
    }

    #[test]
    fn verdict_thresholds() {
        assert_eq!(verdict(7, 300), "warn");
        assert_eq!(verdict(10, 29), "warn", "真机探针：4 章坏 3 章");
        assert_eq!(verdict(149, 300), "warn");
        assert_eq!(verdict(150, 300), "ok");
        assert_eq!(verdict(25, 29), "ok", "真机探针：好书 0.86");
        assert_eq!(verdict(600, 300), "ok");
        assert_eq!(verdict(1, 0), "ok", "没期望值不判");
    }
}
