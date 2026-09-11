//! 一份文档的摄取编排：只扫**变更页**（页 `.rm` mtime 大于上次记录）→ rmv6 解析 → notecore 配对/合并 → 裁图 → 落条目库。
//! 只处理活的 EPUB 文档（首期）；页→章由 epubmap 给。
use crate::bookdb::BookDb;
use crate::config::IngestConfig;
use crate::crop::render_ink;
use crate::doc::Doc;
use epubmap::BookMap;
use notecore::ingest::{drafts_of_page, merge_page, MergeStats, PageCtx};
use notecore::model::{Book, Status};
use std::path::Path;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct DocStats {
    pub pages: usize,
    pub merge: MergeStats,
}

/// 摄取一份文档。返回 None = 不该管（非 EPUB / 回收站 / 没有手写页）。
pub fn ingest_doc(lib: &Path, crops_dir: &Path, db: &BookDb, cfg: &IngestConfig, uuid: &str, now: u64) -> Result<Option<DocStats>, String> {
    let doc = Doc::new(lib, uuid);
    let Some(meta) = doc.metadata().filter(|m| m.is_live_document()) else {
        // 书被移进回收站，或彻底删除（连 .metadata 都没了）：撤销条目库里这本书还没撤销的条目。
        // 不这么做的话 `BookDb::list_active` 找不到理由把它从列表摘掉——它只看条目状态，
        // 从没在这条路径上被通知过"书本身没了"（真机验证时发现，2026-09-07；清空勾画走
        // `merge_page` 那条撤销路径管的是"页还在、笔画没了"，这里是另一条"书不见了"的路径）。
        return revoke_stale(db, uuid, now);
    };
    let Some(content) = doc.content().filter(|c| c.file_type == "epub") else { return Ok(None) };
    let pages = doc.annotated_pages();
    if pages.is_empty() {
        return Ok(None);
    }
    let prev = db.load(uuid);
    let changed: Vec<(String, u64)> = pages.into_iter().filter(|(id, mt)| prev.as_ref().and_then(|b| b.page_mtimes.get(id)).map(|&old| *mt > old).unwrap_or(true)).collect();
    if changed.is_empty() {
        return Ok(Some(DocStats::default()));
    }
    // 页→章：每次摄取现读（.epubindex 在 xochitl 重排后会变）
    let map = match (doc.epub_bytes(), doc.epubindex_bytes()) {
        (Some(e), Some(i)) => BookMap::from_epub(&e, &i),
        _ => BookMap::default(),
    };
    let th = cfg.thresholds();
    let title = meta.visible_name.clone();
    let chapters: Vec<String> = map.chapters().into_iter().map(|(_, t)| t.to_string()).collect();
    let mut stats = DocStats::default();
    let mut errors: Vec<String> = vec![];
    db.update(
        uuid,
        || Book { uuid: uuid.to_string(), title: title.clone(), ..Default::default() },
        |book| {
            book.title = title.clone();
            book.chapters = chapters.clone();
            for (page_id, mtime) in &changed {
                let bytes = match std::fs::read(doc.page_rm(page_id)) {
                    Ok(b) => b,
                    Err(e) => {
                        errors.push(format!("{page_id}: 读 .rm 失败 {e}"));
                        continue;
                    }
                };
                let page = match rmv6::page::Page::parse(&bytes) {
                    Ok(p) => p,
                    Err(e) => {
                        errors.push(format!("{page_id}: 解析失败 {e:?}"));
                        continue;
                    }
                };
                let page_index = content.pages.iter().position(|p| p == page_id).unwrap_or(0);
                let ch = map.chapter_of(page_index);
                let ctx = PageCtx { book: uuid, page: page_id, page_index, chapter: ch.as_ref().map(|c| c.index), chapter_title: ch.as_ref().map(|c| c.title).unwrap_or(""), subhead: ch.as_ref().and_then(|c| c.subhead), now };
                let drafts = drafts_of_page(&page, &th);
                let m = merge_page(&mut book.entries, &ctx, drafts);
                stats.merge.added += m.added;
                stats.merge.changed += m.changed;
                stats.merge.unchanged += m.unchanged;
                stats.merge.revoked += m.revoked;
                stats.pages += 1;
                // 裁图：本页所有有手写、且裁图缺失或指纹变了的条目。自渲染（`render_ink`）直接吃这一页
                // 已经解析好的 `page.strokes`，不依赖 xochitl 缩略图——写多靠下都画得出来，也不会混进印刷体
                // （2026-09-07 二期真机验证发现的两个问题，见白皮书 §03o）。
                for e in book.entries.iter_mut().filter(|e| e.page == *page_id) {
                    let Some(ink) = e.ink.as_mut() else { continue };
                    let want = format!("{}-{}.png", e.id, ink.hash);
                    if ink.crop == want && crops_dir.join(&want).is_file() {
                        continue;
                    }
                    match render_ink(&page.strokes, &ink.strokes, ink.bbox, cfg.crop_margin) {
                        Ok(png) => match rmsvc_core::fs::write_atomic(&crops_dir.join(&want), &png) {
                            Ok(()) => ink.crop = want,
                            Err(err) => errors.push(format!("{page_id}: 写裁图失败 {err}")),
                        },
                        Err(err) => errors.push(format!("{page_id}: {err}")),
                    }
                }
                book.page_mtimes.insert(page_id.clone(), *mtime);
            }
        },
    )?;
    if !errors.is_empty() {
        println!("[ink-serve] {uuid} 摄取告警: {}", errors.join("; "));
    }
    Ok(Some(stats))
}

/// 书不再活了（回收站/已删）：条目库里如果还有没撤销的条目，全标 `Revoked`（不物理删，历史留痕）。
/// 没追平摄取过的书（条目库压根没有）是 no-op；已经全撤销过也是 no-op（幂等，事件重复触发不白做功）。
fn revoke_stale(db: &BookDb, uuid: &str, now: u64) -> Result<Option<DocStats>, String> {
    if db.load(uuid).is_none() {
        return Ok(None);
    }
    let revoked = db.update(uuid, || Default::default(), |b| {
        let mut n = 0usize;
        // 同一个"排除法"漏洞（见 notecore::ingest::merge_page 的注释）：`Skipped`/`Archived` 也是
        // 终态，书被删/进回收站不该把它们悄悄改判成 `Revoked`——那样以后 `restore()` 会走错分支。
        // 用 `is_terminal()` 排除全部三种终态，只把还活着（Mined/Pending/Draft/Reviewed）的条目
        // 因"书不在了"而转 Revoked。
        for e in b.entries.iter_mut().filter(|e| !e.is_terminal()) {
            e.status = Status::Revoked;
            e.updated = now;
            n += 1;
        }
        n
    })?;
    Ok((revoked > 0).then(|| DocStats { pages: 0, merge: MergeStats { revoked, ..Default::default() } }))
}

/// 书库里所有活的 EPUB 且有手写页的文档 uuid（启动追平用）。
pub fn candidate_docs(lib: &Path) -> Vec<String> {
    let Ok(rd) = std::fs::read_dir(lib) else { return vec![] };
    let mut out: Vec<String> = rd
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if !p.is_dir() {
                return None;
            }
            let uuid = p.file_name()?.to_str()?.to_string();
            crate::doc::uuid_of_event(&uuid)?;
            let d = Doc::new(lib, &uuid);
            (d.metadata()?.is_live_document() && d.content()?.file_type == "epub" && !d.annotated_pages().is_empty()).then_some(uuid)
        })
        .collect();
    out.sort();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // 原测试 skips_non_epub_and_ingests_only_changed_pages 整个依赖真机《人骨拼圖》fixture
    // （testdata/renggu/ 的 book.content/book.epubindex/page.rm）验证摄取管线对真实书页的处理，
    // 公开发行版不带这份含真实版权小说原文的夹具，这个测试没法在不重新造一份等价 fixture 的
    // 前提下继续跑，整个删掉——不是功能改了，是测试数据来源变了；私有开发仓库这份测试原样保留。

    fn seeded_entry(id: &str, status: Status) -> notecore::model::Entry {
        notecore::model::Entry { id: id.into(), page: "p".into(), page_index: 0, chapter: None, chapter_title: String::new(), subhead: None, quote: None, ink: None, drafts: vec![], text: None, style: Default::default(), ask_ai: false, question: None, answer: None, status, destination: Default::default(), created: 0, updated: 0 }
    }

    /// 真机验证时发现的 bug（2026-09-07）：书被移进回收站、甚至彻底删除，条目库里的旧条目
    /// 一直没人管，`BookDb::list_active` 就一直找得到理由把这本书留在网页选择器里。
    #[test]
    fn trashed_or_deleted_book_revokes_its_stale_entries_but_untracked_book_is_a_no_op() {
        let t = tempfile::tempdir().unwrap();
        let lib = t.path().join("xochitl");
        let crops = t.path().join("crops");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&crops).unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        let cfg = IngestConfig::default();

        // 场景一：条目库里已经追平过、有活条目的书，被移进回收站（.metadata 还在，parent=trash）。
        let u1 = "11111111-5e28-4969-8926-c8973d49020d";
        db.update(u1, || Book { uuid: u1.into(), title: "测试书一".into(), ..Default::default() }, |b| b.entries.push(seeded_entry("e1", Status::Reviewed))).unwrap();
        std::fs::write(lib.join(format!("{u1}.metadata")), r#"{"visibleName":"测试书一","type":"DocumentType","parent":"trash"}"#).unwrap();
        let s1 = ingest_doc(&lib, &crops, &db, &cfg, u1, 10).unwrap().unwrap();
        assert_eq!((s1.pages, s1.merge.revoked), (0, 1));
        assert_eq!(db.load(u1).unwrap().entries[0].status, Status::Revoked);
        // 再摄取一遍（比如又收到一次事件）：已经全撤销 → 幂等 no-op，不重复计数
        assert!(ingest_doc(&lib, &crops, &db, &cfg, u1, 11).unwrap().is_none());

        // 场景二：另一本书直接被彻底删除——.metadata 都没写过（真机上是文件被删掉）。
        let u2 = "22222222-5e28-4969-8926-c8973d49020d";
        db.update(u2, || Book { uuid: u2.into(), title: "测试书二".into(), ..Default::default() }, |b| b.entries.push(seeded_entry("e2", Status::Pending))).unwrap();
        let s2 = ingest_doc(&lib, &crops, &db, &cfg, u2, 12).unwrap().unwrap();
        assert_eq!((s2.pages, s2.merge.revoked), (0, 1));
        assert_eq!(db.load(u2).unwrap().entries[0].status, Status::Revoked);

        // 场景三：条目库里压根没追平过的书被删——no-op，不建幽灵记录、不 panic。
        let u3 = "33333333-5e28-4969-8926-c8973d49020d";
        assert!(ingest_doc(&lib, &crops, &db, &cfg, u3, 13).unwrap().is_none());
        assert!(db.load(u3).is_none());
    }

    /// 回归：书被删/进回收站时，`Skipped`/`Archived` 这两种终态不该被"排除法"漏判成 `Revoked`
    /// （只有 Mined/Pending/Draft/Reviewed 这些还活着的条目才该因为"书不在了"转 Revoked）。
    #[test]
    fn revoke_stale_leaves_already_terminal_entries_alone() {
        let t = tempfile::tempdir().unwrap();
        let lib = t.path().join("xochitl");
        let crops = t.path().join("crops");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&crops).unwrap();
        let db = BookDb::new(t.path().join("books"));
        db.ensure().unwrap();
        let cfg = IngestConfig::default();

        let u = "44444444-5e28-4969-8926-c8973d49020d";
        db.update(
            u,
            || Book { uuid: u.into(), title: "测试书四".into(), ..Default::default() },
            |b| {
                b.entries.push(seeded_entry("skipped", Status::Skipped));
                b.entries.push(seeded_entry("archived", Status::Archived));
                b.entries.push(seeded_entry("pending", Status::Pending));
            },
        )
        .unwrap();
        let s = ingest_doc(&lib, &crops, &db, &cfg, u, 10).unwrap().unwrap();
        assert_eq!(s.merge.revoked, 1, "只有 pending 那条该被转 Revoked");
        let entries = db.load(u).unwrap().entries;
        assert_eq!(entries[0].status, Status::Skipped, "Skipped 不该被改判");
        assert_eq!(entries[1].status, Status::Archived, "Archived 不该被改判");
        assert_eq!(entries[2].status, Status::Revoked, "真正活着的条目该转 Revoked");
    }
}
