//! 一页解析结果 → 条目草稿 → 按增量规则并入书的条目库。
//! 增量规则（用户 2026-09-06 的担心"二次识别把您好覆盖回你好"）：
//! 1. 簇指纹不变 → 条目原样（不重转写、不动 `text`）；
//! 2. 簇与旧条目共享笔画但指纹变了（补了几笔）→ 同一条目，更新 `ink`，等新草稿作建议，`text` 不动；
//! 3. 旧条目的笔画全没了 → `Revoked`（留痕不删）；
//! 4. 全新簇 → 新条目，id 由 (书, 页, 最小笔画 id) 一次算定。
use crate::geom::{cluster, pair, Thresholds};
use crate::hash::{cluster_hash, entry_id};
use crate::model::{Entry, Ink, Quote, Status, Style};
use rmv6::page::Page;
use rmv6::v6::crdt::CrdtId;

fn sid(id: &CrdtId) -> String {
    format!("{}:{}", id.part1, id.part2)
}

fn color_name(c: &rmv6::shared::pen_color::PenColor, rgba: Option<(u8, u8, u8, u8)>) -> String {
    match (c, rgba) {
        (rmv6::shared::pen_color::PenColor::Unknown(_), Some((r, g, b, _))) => format!("#{r:02x}{g:02x}{b:02x}"),
        (c, _) => format!("{c:?}").to_ascii_lowercase(),
    }
}

/// 一页里认出的"一片手写（+ 它旁边的勾画）"，或"一条没配到手写的纯勾画"（`ink: None`，2026-09-07
/// 二期真机验证时发现的缺口：只勾线不写字的页原来整页被跳过，见下方 `drafts_of_page` 尾部补的一段）。
#[derive(Debug, Clone, PartialEq)]
pub struct PageDraft {
    pub ink: Option<Ink>,
    pub quote: Option<Quote>,
}

fn quote_of(h: &rmv6::page::Highlight) -> Quote {
    Quote { id: sid(&h.id), text: h.text.clone(), color: color_name(&h.color, h.rgba), rects: h.rects.clone() }
}

pub fn drafts_of_page(page: &Page, th: &Thresholds) -> Vec<PageDraft> {
    let clusters = cluster(&page.strokes, th);
    let pairs = pair(&clusters, &page.highlights, th);
    let mut paired: std::collections::BTreeSet<usize> = Default::default();
    let mut out: Vec<PageDraft> = clusters
        .iter()
        .zip(pairs)
        .map(|(c, hl)| {
            let items: Vec<(String, usize, (f32, f32, f32, f32))> = c.strokes.iter().map(|&i| { let s = &page.strokes[i]; (sid(&s.id), s.points.len(), (s.bbox.x0, s.bbox.y0, s.bbox.x1, s.bbox.y1)) }).collect();
            let refs: Vec<(&str, usize, (f32, f32, f32, f32))> = items.iter().map(|(id, n, b)| (id.as_str(), *n, *b)).collect();
            let mut strokes: Vec<String> = items.iter().map(|(id, _, _)| id.clone()).collect();
            strokes.sort();
            if let Some(i) = hl {
                paired.insert(i);
            }
            PageDraft {
                ink: Some(Ink { strokes, bbox: (c.bbox.x0, c.bbox.y0, c.bbox.x1, c.bbox.y1), hash: cluster_hash(&refs), crop: String::new() }),
                quote: hl.map(|i| quote_of(&page.highlights[i])),
            }
        })
        .collect();
    // 没被任何簇认领的勾画：单独落一条"纯勾画"草稿（没有旁边手写，内容就是勾画本身，不需要转写）。
    for (i, h) in page.highlights.iter().enumerate() {
        if !paired.contains(&i) {
            out.push(PageDraft { ink: None, quote: Some(quote_of(h)) });
        }
    }
    out
}

/// 页级上下文（摄取时由 ink-serve 从 epubmap 算好）。
pub struct PageCtx<'a> {
    pub book: &'a str,
    pub page: &'a str,
    pub page_index: usize,
    pub chapter: Option<usize>,
    pub chapter_title: &'a str,
    pub subhead: Option<&'a str>,
    pub now: u64,
}

/// 合并结果统计（日志/事件用）。
#[derive(Debug, Default, PartialEq, Eq)]
pub struct MergeStats {
    pub added: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub revoked: usize,
}

/// 把本页新草稿并入 `entries`（只动本页的条目）。**两条认领路径**：有手写的草稿按笔画指纹/共享笔画
/// 认领（原逻辑不变）；纯勾画草稿（`ink: None`）没有笔画可比，按勾画自己的 `Quote.id`（`GlyphRange`
/// 的 CRDT id）认领——2026-09-07 二期真机验证时发现"只勾线不写字"整页被跳过，补的这条路径。
pub fn merge_page(entries: &mut Vec<Entry>, ctx: &PageCtx, drafts: Vec<PageDraft>) -> MergeStats {
    let mut st = MergeStats::default();
    let mut claimed = vec![false; entries.len()];
    for d in drafts {
        let same_page = |e: &Entry| e.page == ctx.page && e.status != Status::Revoked;
        let hit = match &d.ink {
            Some(dink) => entries.iter().enumerate().find(|(i, e)| !claimed[*i] && same_page(e) && e.ink.as_ref().map(|k| k.hash == dink.hash).unwrap_or(false)).map(|(i, _)| i)
                .or_else(|| entries.iter().enumerate().find(|(i, e)| !claimed[*i] && same_page(e) && e.ink.as_ref().map(|k| k.strokes.iter().any(|s| dink.strokes.contains(s))).unwrap_or(false)).map(|(i, _)| i)),
            None => {
                let dq_id = d.quote.as_ref().map(|q| q.id.as_str());
                entries.iter().enumerate().find(|(i, e)| !claimed[*i] && same_page(e) && e.ink.is_none() && e.quote.as_ref().map(|q| q.id.as_str()) == dq_id).map(|(i, _)| i)
            }
        };
        match hit {
            Some(i) => {
                claimed[i] = true;
                let e = &mut entries[i];
                match &d.ink {
                    Some(dink) => {
                        if e.ink.as_ref().map(|k| k.hash == dink.hash).unwrap_or(false) {
                            st.unchanged += 1;
                        } else {
                            let crop = e.ink.as_ref().map(|k| k.crop.clone()).unwrap_or_default();
                            e.ink = Some(Ink { crop, ..dink.clone() });
                            e.updated = ctx.now;
                            st.changed += 1;
                        }
                        if e.quote.is_none() {
                            e.quote = d.quote; // 后补的勾画认上
                        }
                    }
                    None => {
                        st.unchanged += 1; // 纯勾画：内容随 quote id 走，认领到了就是没变（勾画画下不会再改）
                        e.quote = d.quote; // 保险起见仍然刷新一遍（颜色等字段理论上可能变）
                    }
                }
            }
            None => {
                let id_seed = d.ink.as_ref().map(|k| k.strokes.first().cloned().unwrap_or_default()).or_else(|| d.quote.as_ref().map(|q| q.id.clone())).unwrap_or_default();
                entries.push(Entry {
                    id: entry_id(ctx.book, ctx.page, &id_seed),
                    page: ctx.page.to_string(),
                    page_index: ctx.page_index,
                    chapter: ctx.chapter,
                    chapter_title: ctx.chapter_title.to_string(),
                    subhead: ctx.subhead.map(str::to_string),
                    quote: d.quote,
                    ink: d.ink,
                    drafts: vec![],
                    text: None,
                    style: Style::Body,
                    ask_ai: false,
                    question: None,
                    answer: None,
                    status: Status::Mined, // 只是探测到，还没被用户要求转笔记——见浏览态设计（2026-09-07 二期）
                    destination: crate::model::Destination::default(), // 落设备笔记本——三期新字段，默认不变行为
                    source: crate::model::Source::default(), // xochitl 摄取线，默认不变行为
                    created: ctx.now,
                    updated: ctx.now,
                });
                claimed.push(true);
                st.added += 1;
            }
        }
    }
    for (i, e) in entries.iter_mut().enumerate() {
        // "排除法"曾经只挡 `!= Revoked`（白皮书 §04 记过的反模式，2026-09-09 那轮审计改了 project.rs/
        // export.rs/三处写入口，唯独漏了这里）：`Skipped`/`Archived` 也是终态，笔画被擦掉不该把它们
        // 悄悄改判成 `Revoked`——那样 `restore()` 会走错分支（`Skipped` 该固定回 `Mined`，被错判成
        // `Revoked` 后会按内容倒推，ink 还在字段里就恢复成 `Pending`，用户明确"不需要"过的内容被拉回
        // 转写队列）。改用 `is_terminal()` 单一事实源，三态终态一起排除。
        //
        // 注意：只改这一处，不改上面 `same_page` 的匹配判据（仍是 `!= Revoked`）——匹配判据管的是"这
        // 是不是同一份还在原地的内容，别重复建条目"，Skipped/Archived 但笔迹没动过的条目理应继续被
        // 匹配上（保持原状不动），如果连匹配都排除掉，笔迹没变但整页因为别处改动触发重扫时，会给同一份
        // 已经"不需要"过的内容重新生成一条 `Mined`，那是另一个新 bug，不是这里要修的。
        if !claimed[i] && e.page == ctx.page && !e.is_terminal() && (e.ink.is_some() || e.quote.is_some()) {
            e.status = Status::Revoked;
            e.updated = ctx.now;
            st.revoked += 1;
        }
    }
    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::fixtures::*;
    use rmv6::shared::tool::Tool;

    fn page(strokes: Vec<rmv6::page::Stroke>, hls: Vec<rmv6::page::Highlight>) -> Page {
        Page { strokes, highlights: hls, text: None }
    }
    fn ctx<'a>(now: u64) -> PageCtx<'a> {
        PageCtx { book: "b", page: "p1", page_index: 7, chapter: Some(2), chapter_title: "三", subhead: None, now }
    }

    #[test]
    fn incremental_rules_keep_reviewed_text() {
        let th = Thresholds::default();
        let p1 = page(vec![stroke(1, Tool::BallPoint, 900.0, 300.0, 1000.0, 340.0)], vec![hl(9, "勾画", 100.0, 320.0, 780.0, 30.0)]);
        let mut entries = vec![];
        let st = merge_page(&mut entries, &ctx(10), drafts_of_page(&p1, &th));
        assert_eq!(st, MergeStats { added: 1, ..Default::default() });
        let id = entries[0].id.clone();
        assert_eq!(entries[0].quote.as_ref().map(|q| q.text.as_str()), Some("勾画"));
        // 人校对
        entries[0].text = Some("您好".into());
        entries[0].status = Status::Reviewed;
        // 同样的页再摄取一次 → 不变
        let st = merge_page(&mut entries, &ctx(20), drafts_of_page(&p1, &th));
        assert_eq!(st, MergeStats { unchanged: 1, ..Default::default() });
        assert_eq!(entries[0].updated, 10);
        // 补了一笔（共享笔画 1）→ 同一条目、ink 变、text 不动
        let p2 = page(vec![stroke(1, Tool::BallPoint, 900.0, 300.0, 1000.0, 340.0), stroke(2, Tool::BallPoint, 1005.0, 300.0, 1040.0, 340.0)], vec![]);
        let st = merge_page(&mut entries, &ctx(30), drafts_of_page(&p2, &th));
        assert_eq!(st, MergeStats { changed: 1, ..Default::default() });
        assert_eq!(entries.len(), 1);
        assert_eq!((entries[0].id.as_str(), entries[0].text.as_deref()), (id.as_str(), Some("您好")));
        assert!(entries[0].needs_transcribe(), "新指纹没有草稿 → 作为建议再转写");
        assert_eq!(entries[0].quote.as_ref().map(|q| q.text.as_str()), Some("勾画"), "已认的勾画不因这页少了高亮而丢");
        // 笔画全擦掉 → 撤销，不删
        let st = merge_page(&mut entries, &ctx(40), drafts_of_page(&page(vec![], vec![]), &th));
        assert_eq!(st, MergeStats { revoked: 1, ..Default::default() });
        assert_eq!(entries[0].status, Status::Revoked);
        // 别页的条目不受影响
        let mut other = entries.clone();
        other[0].page = "p2".into();
        other[0].status = Status::Reviewed;
        let st = merge_page(&mut other, &ctx(50), vec![]);
        assert_eq!(st, MergeStats::default());
    }

    #[test]
    fn two_clusters_one_with_quote_one_page_note() {
        let th = Thresholds::default();
        let p = page(vec![stroke(1, Tool::BallPoint, 900.0, 300.0, 1000.0, 340.0), stroke(5, Tool::BallPoint, 100.0, 1500.0, 200.0, 1530.0)], vec![hl(9, "第二段", 100.0, 320.0, 780.0, 30.0)]);
        let ds = drafts_of_page(&p, &th);
        assert_eq!(ds.len(), 2);
        assert_eq!(ds[0].quote.as_ref().map(|q| q.text.as_str()), Some("第二段"));
        assert_eq!(ds[1].quote, None);
        assert_eq!(ds[0].ink.as_ref().unwrap().strokes, vec!["1:1"]);
        assert_eq!(ds[0].quote.as_ref().unwrap().color, "yellow");
    }

    /// 2026-09-07 二期真机验证时发现的缺口：只勾线不写字的页，原来整页被跳过；补上"纯勾画"路径。
    #[test]
    fn quote_only_page_creates_a_no_ink_entry_and_survives_rescans() {
        let th = Thresholds::default();
        // 一条勾画，旁边完全没有手写。
        let p = page(vec![], vec![hl(9, "纯勾画的原文", 100.0, 320.0, 780.0, 30.0)]);
        let ds = drafts_of_page(&p, &th);
        assert_eq!(ds.len(), 1);
        assert!(ds[0].ink.is_none());
        assert_eq!(ds[0].quote.as_ref().unwrap().text, "纯勾画的原文");
        assert!(!ds[0].quote.as_ref().unwrap().id.is_empty(), "勾画自己的 CRDT id 要落进去，重扫认领靠它");

        let mut entries = vec![];
        let st = merge_page(&mut entries, &ctx(10), ds);
        assert_eq!(st, MergeStats { added: 1, ..Default::default() });
        assert_eq!(entries.len(), 1);
        assert!(entries[0].ink.is_none());
        assert_eq!(entries[0].status, Status::Mined);
        let id = entries[0].id.clone();

        // 同一页再摄取一次（页没变）：认领到同一条，不变、不重复新建。
        let st2 = merge_page(&mut entries, &ctx(20), drafts_of_page(&p, &th));
        assert_eq!(st2, MergeStats { unchanged: 1, ..Default::default() });
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, id);

        // 勾画被擦掉（页上啥都没了）：撤销，不物理删——跟手写条目同一套终态。
        let st3 = merge_page(&mut entries, &ctx(30), drafts_of_page(&page(vec![], vec![]), &th));
        assert_eq!(st3, MergeStats { revoked: 1, ..Default::default() });
        assert_eq!(entries[0].status, Status::Revoked);
    }

    /// 回归：`Skipped`/`Archived` 是终态，笔迹被擦掉不该被"排除法"漏判改成 `Revoked`——
    /// 那样 `restore()` 会走错分支（详见 merge_page 里 `is_terminal()` 那段注释）。
    #[test]
    fn skipped_and_archived_entries_are_not_silently_flipped_to_revoked_when_ink_disappears() {
        let th = Thresholds::default();
        let p1 = page(vec![stroke(1, Tool::BallPoint, 900.0, 300.0, 1000.0, 340.0)], vec![hl(9, "勾画", 100.0, 320.0, 780.0, 30.0)]);
        let mut entries = vec![];
        merge_page(&mut entries, &ctx(10), drafts_of_page(&p1, &th));
        entries[0].status = Status::Skipped; // 用户点了「不需要」

        let mut archived_entries = entries.clone();
        archived_entries[0].status = Status::Archived; // 独立一份对照「不要了」

        // 笔迹被擦掉，页上啥都没了 → 重扫。
        let empty_drafts = drafts_of_page(&page(vec![], vec![]), &th);
        let st = merge_page(&mut entries, &ctx(20), empty_drafts.clone());
        assert_eq!(st, MergeStats::default(), "终态条目不该再被计入 revoked 统计");
        assert_eq!(entries[0].status, Status::Skipped, "该保持 Skipped，不是被错判成 Revoked");

        let st2 = merge_page(&mut archived_entries, &ctx(20), empty_drafts);
        assert_eq!(archived_entries[0].status, Status::Archived, "同理，Archived 也不该被改判");
        assert_eq!(st2, MergeStats::default());
    }
}
