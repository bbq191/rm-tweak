//! 数据模型。所有字段可序列化（条目库落 JSON，网页/CLI 直接吃同一形状）。
use serde::{Deserialize, Serialize};

/// 笔记本里一行的段落样式（对应 xochitl 3.28 打字格式；Title/Subheading 由投影自动给，条目只在这四种里）。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    #[default]
    Body,
    Bullet,
    Numbered,
    Checkbox,
}

impl Style {
    /// 对应 `.rm` 段落样式码（2026-09-07 真机样本 `testdata/seven_styles` 坐实，见 `rmv6::v6::scene_item::text::ParagraphStyle`）。
    /// NumberedList(10) 格式子块跟其余样式一样只有 2 字节，没有隐藏内容（早前"多 7 字节未解码载荷"的
    /// 说法是分析失误，那 7 字节其实属于 Subheading 1，见 `rmv6::write` 模块文档与白皮书 §03i）——
    /// `rmv6::write` 已支持写 NUMBERED 且真机验证过编号正确自动生成。
    pub fn wire_code(self) -> u8 {
        match self {
            Style::Body => 0x01,     // PLAIN
            Style::Bullet => 0x04,   // BULLET
            Style::Numbered => 0x0a, // NUMBERED，真机验证过，见 rmv6::write
            Style::Checkbox => 0x06, // CHECKBOX（未勾选；勾上号 7 要点方框，打字给不出，写入器别用）
        }
    }
}

/// 转写/校对/问答完之后，这条内容最终要投影去哪（三期，2026-09-08）：默认 `Both`——两处都要，跟这个
/// 字段加之前"两个投影都只看 `chapter`+`status`、来者不拒"的行为完全一致（`project.rs` 该收的还收，
/// `export.rs` 新功能对已有内容立刻可用，不用先给每条条目手动选一遍才肯导出）。用户想收窄再改成
/// `Notebook`（只留设备）/`Obsidian`（只导出，不回落设备笔记本）。`project.rs` 只收
/// `wants_notebook()`；`export.rs` 只收 `wants_obsidian()`。
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Destination {
    Notebook,
    Obsidian,
    #[default]
    Both,
}

impl Destination {
    pub fn wants_notebook(self) -> bool {
        matches!(self, Destination::Notebook | Destination::Both)
    }
    pub fn wants_obsidian(self) -> bool {
        matches!(self, Destination::Obsidian | Destination::Both)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    /// ink-serve 刚探测到（有画线/手写），还没被用户要求转笔记——**不会**被自动转写。
    /// 2026-09-07 二期定案：探测（挖到了）与转笔记（真想要）拆成两个独立动作，见浏览态设计；
    /// 之前版本摄取时给的初始态是 `Pending`，现在收窄到"用户在浏览列表点了『转入笔记』才算数"。
    #[default]
    Mined,
    /// 用户点了「转入笔记」，等转写。
    Pending,
    /// 有转写草稿、等人校对。
    Draft,
    /// 人已定稿（`text` 有值）。
    Reviewed,
    /// 用户在浏览列表点了「不需要」——跟 `Mined` 一样不会被自动转写，区别只是不再出现在待办列表里。
    Skipped,
    /// 笔画已从书页删除（不物理删，留痕）。
    Revoked,
    /// 用户在「整理」里点了「不要了」（三期，2026-09-08）：转写/问答都看完了，两处投影
    /// （设备笔记本 `project.rs` / Obsidian `export.rs`）都不要再出现——跟 `Revoked` 一样是终态、
    /// 不物理删（留痕，靠 `Book::purge_terminal` 手动清），区别只是触发方是用户主动"删除"而不是
    /// 笔画被擦掉。
    Archived,
}

impl Status {
    /// 真被要求转笔记、该出现在两处投影（设备笔记本 `project.rs` / Obsidian `export.rs`）里的状态。
    /// **单一事实源**（2026-09-09 审计补）：这条判据之前在 `project::live_entries`/
    /// `export::live_entries` 里各写一遍 `matches!`，网关 `app.js::renderBook` 又单独抄了一份
    /// `['pending','draft','reviewed'].includes(...)`——状态机还在演进（已经加到 7 个变体），
    /// `Mined`/`Skipped` 引入时就真的漏改过一处（`!= Revoked` 那次真机 bug，见白皮书 §03r），
    /// 收成一处避免下次再漏。前端那份因为是另一种语言/另一个仓库位置，做不到直接复用，
    /// 只能在旁边留注释指回这里。
    pub fn is_live_for_projection(self) -> bool {
        matches!(self, Status::Pending | Status::Draft | Status::Reviewed)
    }
}

/// 配对到的勾画（GlyphRange）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Quote {
    /// 勾画自己的稳定 id（`.rm` 里 `GlyphRange` 的 CRDT id，不是笔画 id）：给"纯勾画条目"（没有旁边
    /// 手写，`Entry.ink` 是 `None`）做重扫认领用，笔画哈希那套配不上它。`#[serde(default)]` 兼容
    /// 二期这个字段加之前落盘的旧条目库（旧数据反序列化成空串，不影响已有的手写配对逻辑）。
    #[serde(default)]
    pub id: String,
    pub text: String,
    pub color: String,
    /// 每行一个矩形 (x, y, w, h)，页坐标。
    pub rects: Vec<(f32, f32, f32, f32)>,
}

/// 一片手写：笔画 id 集合、包围盒、指纹、裁图文件名。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Ink {
    pub strokes: Vec<String>,
    pub bbox: (f32, f32, f32, f32),
    pub hash: String,
    #[serde(default)]
    pub crop: String,
}

/// 转写草稿（可多次，最新在前）。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Draft {
    pub text: String,
    pub backend: String,
    pub at: u64,
    /// 这份草稿对应的簇指纹（指纹变了旧草稿失效）。
    pub hash: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Answer {
    pub text: String,
    pub backend: String,
    pub at: u64,
    /// 生成时用的分区简述（简述改了要重跑）。
    pub brief: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Entry {
    /// 稳定 id：创建时按 (书, 页, 首笔 id) 生成，之后**永不重算**（簇变了靠笔画重叠认领同一条目）。
    pub id: String,
    pub page: String,
    pub page_index: usize,
    #[serde(default)]
    pub chapter: Option<usize>,
    #[serde(default)]
    pub chapter_title: String,
    #[serde(default)]
    pub subhead: Option<String>,
    #[serde(default)]
    pub quote: Option<Quote>,
    #[serde(default)]
    pub ink: Option<Ink>,
    #[serde(default)]
    pub drafts: Vec<Draft>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub style: Style,
    /// 用户勾了「问AI」——二期改按条目单发（`mind-serve`，2026-09-07 二期）。三期（2026-09-08）
    /// 砍掉了"分区"这个概念——AI 触发早就是这个字段的事了，分区兼职的"笔记本排版分组"角色也
    /// 一并砍掉，改成整章条目按页序平铺（各自的 `Style` 就是唯一的格式区分，见 `project.rs`/`export.rs`）。
    #[serde(default)]
    pub ask_ai: bool,
    /// 用户输的问题（`ask_ai` 为真时才有意义）；答案写回 `answer`，`Answer.brief` 存的就是这句问题的存档。
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub answer: Option<Answer>,
    #[serde(default)]
    pub status: Status,
    /// 落设备笔记本 / 落 Obsidian / 两处都要（三期）。`#[serde(default)]` 兼容三期之前落盘的旧条目库。
    #[serde(default)]
    pub destination: Destination,
    pub created: u64,
    pub updated: u64,
}

impl Entry {
    /// 投影用文本：校对文本优先，其次最新草稿。
    pub fn display_text(&self) -> Option<&str> {
        self.text.as_deref().or_else(|| self.drafts.first().map(|d| d.text.as_str()))
    }
    /// 指纹已变、还没有对应新指纹的草稿 → 需要（再）转写。**只认用户已经表态要转的条目**
    /// （`Mined`/`Skipped` 都不算——探测到不等于想转笔记，见浏览态设计，2026-09-07 二期）；
    /// 已经在 `Pending`/`Draft`/`Reviewed` 的条目如果笔画又变了（补了几笔），仍然继续认，
    /// 不会因为已经校对过就不再建议新草稿（校对文本本身不会被覆盖，见增量规则）。
    pub fn needs_transcribe(&self) -> bool {
        matches!((&self.ink, self.status), (Some(ink), s) if !matches!(s, Status::Revoked | Status::Mined | Status::Skipped | Status::Archived) && !self.drafts.iter().any(|d| d.hash == ink.hash))
    }
    /// 终态：`Skipped`/`Revoked`/`Archived`，跟 `restore()` 认定"需要恢复"的三种状态完全一致——
    /// 用户已经明确表态"不需要/已撤销/不要了"，除了 `restore()` 之外的写操作不该再碰它（继续转写/
    /// 继续问 AI/被网页通用改字端点静默拉回活跃态）。2026-09-09 审计发现：`set_triage`/`restore`
    /// 本身有守卫，但通用 PATCH 端点、强制重转写、`mind-serve` 问 AI 三个写入口当初没检查这个，
    /// 会绕开业务规则把终态条目悄悄拉回 `Draft`（见三处调用点新增的守卫）。
    pub fn is_terminal(&self) -> bool {
        matches!(self.status, Status::Skipped | Status::Revoked | Status::Archived)
    }

    /// 浏览态动作：转成 `Pending`（转入笔记）/`Skipped`（不需要）/`Archived`（三期"不要了"）。
    /// 已撤销/已归档的条目是终态，操作没有意义，拒绝。**纯勾画条目**（`ink` 是 `None`，内容全是
    /// `quote`）没有手写可转写——勾画文字是 `GlyphRange` 原生给的精确文字，不需要过一遍视觉模型；
    /// 这种条目"转入笔记"就直接落定（`text = quote.text`、状态跳到 `Reviewed`），不经过
    /// `Pending`/`Draft` 那两步，不然会卡在 `Pending` 里——`needs_transcribe()` 要求 `ink` 是
    /// `Some`，永远不会被自动转写捡走（2026-09-07 二期真机验证时发现的缺口，见笔记线白皮书 §03o）。
    pub fn set_triage(&mut self, target: Status, now: u64) -> Result<(), String> {
        if matches!(self.status, Status::Revoked | Status::Archived) {
            return Err("这条已撤销/已删除，不能再操作".into());
        }
        if target == Status::Pending && self.ink.is_none() {
            if let Some(q) = &self.quote {
                self.text = Some(q.text.clone());
                self.status = Status::Reviewed;
                self.updated = now;
                return Ok(());
            }
        }
        self.status = target;
        self.updated = now;
        Ok(())
    }

    /// 回收站「恢复」（整理区第二轮反馈点 3，2026-09-08）：`Skipped`/`Revoked`/`Archived` 三种终态都能
    /// 恢复——落点按"这条本来走到哪一步"倒推，不额外存一份"删除前的状态"：校对过的文本还在（`text`
    /// 有值）就回 `Reviewed`；没校对但有转写草稿就回 `Draft`；有手写还没转写过就回 `Pending`；纯勾画
    /// 或什么都没留下就回 `Mined`（退回「浏览」重新决定）。`Skipped` 单独处理，永远回 `Mined`——它当初
    /// 就是从 `Mined` 点「不需要」过来的，恢复也该回那一步，不套上面那套"有没有转写内容"的推断。
    ///
    /// `Revoked` 恢复只是找回条目库里已经存好的内容（裁图/草稿/校对文本）——不代表设备原页面的笔迹会
    /// 重新出现。如果笔迹其实还在，下次 `ink-serve` 重扫会照常认出来，不受这次恢复影响；如果笔迹真的
    /// 被用户擦掉了，这条恢复后就是一条脱离设备实时状态的"孤儿条目"，正常参与两处投影，直到下次被操作。
    pub fn restore(&mut self, now: u64) -> Result<(), String> {
        if !matches!(self.status, Status::Skipped | Status::Revoked | Status::Archived) {
            return Err("这条不是已跳过/已撤销/已删除，用不着恢复".into());
        }
        self.status = if self.status == Status::Skipped {
            Status::Mined
        } else if self.text.is_some() {
            Status::Reviewed
        } else if !self.drafts.is_empty() {
            Status::Draft
        } else if self.ink.is_some() {
            Status::Pending
        } else {
            Status::Mined
        };
        self.updated = now;
        Ok(())
    }

    /// 用户在浏览器文本框里直接改字（PATCH `text`）：跟转写草稿写回时（`transcribe-serve::worker`）
    /// 走的是同一套 `crate::marker::split_leading_marker` 规则——行首 `-`/`1.`/`口`/`##`/`### ` 都认，
    /// 不需要再给一个手动选样式的下拉框（整理区第二轮反馈点 1，2026-09-08：「文本规则由 md 符号对标至
    /// rm 笔记符号＝手写识别符号」）。识别到分区/小节标记覆盖 `subhead`；识别到样式标记覆盖 `style` 并把
    /// 标记从正文剥掉（笔记本样式自带项目符号/编号，正文里再留一份会重复）。空文本＝清空校对，状态退回
    /// 有草稿则 `Draft`、没有则 `Pending`。
    pub fn apply_marked_text(&mut self, raw: &str, now: u64) {
        let (marker, clean) = crate::marker::split_leading_marker(raw);
        if let Some(m) = marker {
            match m {
                crate::marker::Marker::Style(s) => self.style = s,
                crate::marker::Marker::Subhead(name) => self.subhead = Some(name),
            }
        }
        let clean = clean.trim();
        self.text = (!clean.is_empty()).then(|| clean.to_string());
        self.status = if self.text.is_some() { Status::Reviewed } else if self.drafts.is_empty() { Status::Pending } else { Status::Draft };
        self.updated = now;
    }
}

/// 一本书的条目库文件形状。
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Book {
    pub uuid: String,
    pub title: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub chapters: Vec<String>,
    #[serde(default)]
    pub entries: Vec<Entry>,
    /// 页 id → 上次摄取时页 `.rm` 的 mtime（秒），只扫变更页。
    #[serde(default)]
    pub page_mtimes: std::collections::BTreeMap<String, u64>,
}

impl Book {
    /// 清空回收站：物理移除 `Archived`/`Revoked`/`Skipped` 这三种"终态、不再活跃"的条目——软删会
    /// 无限攒（每条撤销/跳过/归档的条目永远留痕），这是唯一真正腾空间的操作。**手动触发，不自动跑**，
    /// 跟书级回收站"不自动清空回收站"是同一条纪律（见笔记线白皮书 §05 明确不做清单）；一旦清掉就是
    /// 真删除，不可恢复——调用方（ink-serve）该在网页上给一个需要用户主动点的按钮，不要在别的操作
    /// 里顺手带上。返回删了几条。
    pub fn purge_terminal(&mut self) -> usize {
        let before = self.entries.len();
        self.entries.retain(|e| !matches!(e.status, Status::Archived | Status::Revoked | Status::Skipped));
        before - self.entries.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_defaults() {
        let e = Entry { id: "e1".into(), page: "p".into(), page_index: 3, chapter: Some(1), chapter_title: "一".into(), subhead: None, quote: None, ink: Some(Ink { strokes: vec!["1:2".into()], bbox: (0.0, 0.0, 1.0, 1.0), hash: "h".into(), crop: String::new() }), drafts: vec![], text: None, style: Style::Checkbox, ask_ai: false, question: None, answer: None, status: Status::Pending, destination: Default::default(), created: 1, updated: 1 };
        let j = serde_json::to_string(&e).unwrap();
        assert!(j.contains(r#""style":"checkbox""#) && j.contains(r#""status":"pending""#));
        let back: Entry = serde_json::from_str(&j).unwrap();
        assert_eq!(back, e);
        assert!(back.needs_transcribe());
        let b: Book = serde_json::from_str(r#"{"uuid":"u","title":"t"}"#).unwrap();
        assert!(b.entries.is_empty());
    }

    #[test]
    fn style_wire_codes_match_device_sample() {
        assert_eq!(Style::Body.wire_code(), 0x01);
        assert_eq!(Style::Bullet.wire_code(), 0x04);
        assert_eq!(Style::Numbered.wire_code(), 0x0a);
        assert_eq!(Style::Checkbox.wire_code(), 0x06);
    }

    #[test]
    fn display_text_prefers_reviewed() {
        let mut e: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0}"#).unwrap();
        assert_eq!(e.display_text(), None);
        e.drafts.push(Draft { text: "你好".into(), backend: "qwen".into(), at: 1, hash: "h".into() });
        assert_eq!(e.display_text(), Some("你好"));
        e.text = Some("您好".into());
        e.status = Status::Reviewed; // 已经有草稿+校对文本，说明这条早就被请求过、不再是 Mined 了
        assert_eq!(e.display_text(), Some("您好"), "校对文本压过草稿");
        e.ink = Some(Ink { strokes: vec![], bbox: (0.0, 0.0, 0.0, 0.0), hash: "h".into(), crop: String::new() });
        assert!(!e.needs_transcribe(), "草稿指纹与簇指纹一致");
        e.ink.as_mut().unwrap().hash = "h2".into();
        assert!(e.needs_transcribe(), "簇变了要再转写（作为建议，不动 text）：已经在 Reviewed 态，笔画再变仍然要再认");
        e.status = Status::Mined;
        assert!(!e.needs_transcribe(), "但如果这条从没被请求过（Mined），笔画再怎么变也不自动转写");
    }

    #[test]
    fn set_triage_moves_mined_to_pending_or_skipped_but_refuses_revoked() {
        let mut e: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0}"#).unwrap();
        assert_eq!(e.status, Status::Mined, "缺省态就是 Mined");

        e.set_triage(Status::Pending, 10).unwrap();
        assert_eq!((e.status, e.updated), (Status::Pending, 10), "转入笔记");

        e.status = Status::Mined;
        e.set_triage(Status::Skipped, 20).unwrap();
        assert_eq!((e.status, e.updated), (Status::Skipped, 20), "不需要");

        e.status = Status::Revoked;
        let err = e.set_triage(Status::Pending, 30).unwrap_err();
        assert!(err.contains("已撤销"));
        assert_eq!(e.status, Status::Revoked, "拒绝后状态不变");
    }

    /// 纯勾画条目（`ink: None`）没有手写可转写：转入笔记直接落定成 `Reviewed`，不经过
    /// `Pending`/`Draft`（不然会卡住——`needs_transcribe()` 要求 `ink` 是 `Some`，永远不会被
    /// 自动转写捡走），文本直接取勾画原文。「不需要」还是走普通的 `Skipped`，不特殊。
    #[test]
    fn set_triage_finalizes_quote_only_entries_straight_to_reviewed() {
        let mut e: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0,"quote":{"id":"q1","text":"勾画原文","color":"yellow","rects":[]}}"#).unwrap();
        assert!(e.ink.is_none() && e.quote.is_some());

        e.set_triage(Status::Pending, 10).unwrap();
        assert_eq!((e.status, e.text.as_deref(), e.updated), (Status::Reviewed, Some("勾画原文"), 10), "转入笔记直接定稿，跳过 Pending/Draft");

        // 「不需要」不受影响，还是 Skipped。
        e.status = Status::Mined;
        e.text = None;
        e.set_triage(Status::Skipped, 20).unwrap();
        assert_eq!(e.status, Status::Skipped);
    }

    #[test]
    fn destination_default_is_both_and_truth_table_is_correct() {
        assert_eq!(Destination::default(), Destination::Both, "默认两处都要——新功能对已有内容立刻可用");
        assert!(Destination::Notebook.wants_notebook() && !Destination::Notebook.wants_obsidian());
        assert!(!Destination::Obsidian.wants_notebook() && Destination::Obsidian.wants_obsidian());
        assert!(Destination::Both.wants_notebook() && Destination::Both.wants_obsidian());
    }

    #[test]
    fn set_triage_can_archive_and_refuses_further_operations_on_archived() {
        let mut e: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0,"status":"reviewed"}"#).unwrap();
        e.set_triage(Status::Archived, 5).unwrap();
        assert_eq!((e.status, e.updated), (Status::Archived, 5));
        let err = e.set_triage(Status::Pending, 10).unwrap_err();
        assert!(err.contains("已删除"), "{err}");
        assert_eq!(e.status, Status::Archived, "拒绝后状态不变");
    }

    #[test]
    fn archived_entries_are_never_auto_transcribed() {
        let mut e: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0,"ink":{"strokes":["1:1"],"bbox":[0,0,1,1],"hash":"h"}}"#).unwrap();
        e.status = Status::Archived;
        assert!(!e.needs_transcribe(), "已归档的条目跟已撤销/已跳过一样不该被自动转写捡走");
    }

    #[test]
    fn restore_maps_skipped_to_mined_and_others_by_content_present() {
        fn entry(status: Status, text: Option<&str>, has_draft: bool, has_ink: bool) -> Entry {
            let mut e: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0}"#).unwrap();
            e.status = status;
            e.text = text.map(str::to_string);
            if has_draft { e.drafts.push(Draft { text: "草稿".into(), backend: "b".into(), at: 0, hash: "h".into() }); }
            if has_ink { e.ink = Some(Ink { strokes: vec![], bbox: (0.0, 0.0, 0.0, 0.0), hash: "h".into(), crop: String::new() }); }
            e
        }
        let mut e = entry(Status::Skipped, Some("已校对文本还在"), true, true);
        e.restore(10).unwrap();
        assert_eq!(e.status, Status::Mined, "Skipped 永远回 Mined，不看有没有内容");

        let mut e = entry(Status::Archived, Some("校对文本"), true, true);
        e.restore(10).unwrap();
        assert_eq!(e.status, Status::Reviewed, "有校对文本优先回 Reviewed");

        let mut e = entry(Status::Archived, None, true, true);
        e.restore(10).unwrap();
        assert_eq!(e.status, Status::Draft, "没校对文本但有草稿回 Draft");

        let mut e = entry(Status::Revoked, None, false, true);
        e.restore(10).unwrap();
        assert_eq!(e.status, Status::Pending, "只有手写没转写回 Pending");

        let mut e = entry(Status::Revoked, None, false, false);
        e.restore(10).unwrap();
        assert_eq!((e.status, e.updated), (Status::Mined, 10), "什么都没留下回 Mined 重新走浏览");

        let mut live: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0,"status":"reviewed"}"#).unwrap();
        let err = live.restore(1).unwrap_err();
        assert!(err.contains("用不着恢复"), "{err}");
    }

    #[test]
    fn is_live_for_projection_is_exactly_pending_draft_reviewed() {
        // project.rs/export.rs 的 live_entries 判据单一事实源——2026-09-09 收进来之前，`Mined`/
        // `Skipped` 混进两条投影是真出过的 bug（白皮书 §03r），这条测试钉住"只有这三个状态算活跃"，
        // 以后状态机再加变体，忘了在这里更新的话至少这条测试会先崩，不会悄悄漏判。
        let live = [Status::Pending, Status::Draft, Status::Reviewed];
        let not_live = [Status::Mined, Status::Skipped, Status::Revoked, Status::Archived];
        for s in live {
            assert!(s.is_live_for_projection(), "{s:?} 该算活跃");
        }
        for s in not_live {
            assert!(!s.is_live_for_projection(), "{s:?} 不该算活跃");
        }
    }

    #[test]
    fn is_terminal_matches_exactly_what_restore_would_accept() {
        // is_terminal() 的定义就是"restore() 会接受的三种状态"，两者必须完全对应——否则一处改了
        // 忘记改另一处，终态守卫又会重新出现旁路。
        for s in [Status::Mined, Status::Pending, Status::Draft, Status::Reviewed] {
            let mut e: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0}"#).unwrap();
            e.status = s;
            assert!(!e.is_terminal(), "{s:?} 不是终态");
        }
        for s in [Status::Skipped, Status::Revoked, Status::Archived] {
            let mut e: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0}"#).unwrap();
            e.status = s;
            assert!(e.is_terminal(), "{s:?} 是终态");
            assert!(e.restore(0).is_ok(), "is_terminal() 为真的状态 restore() 也该接受，两者定义必须一致");
        }
    }

    #[test]
    fn apply_marked_text_infers_style_and_subhead_from_markdown_style_markers() {
        let mut e: Entry = serde_json::from_str(r#"{"id":"e","page":"p","page_index":0,"created":0,"updated":0}"#).unwrap();
        e.apply_marked_text("- 查作者", 5);
        assert_eq!((e.style, e.text.as_deref(), e.status, e.updated), (Style::Bullet, Some("查作者"), Status::Reviewed, 5), "行首 - 自动判无序，标记剥掉");

        e.apply_marked_text("### 人物关系", 6);
        assert_eq!((e.subhead.as_deref(), e.text.as_deref()), (Some("人物关系"), Some("人物关系")), "分区标记覆盖 subhead，正文也剥了标记");

        e.apply_marked_text("普通一句话", 7);
        assert_eq!(e.text.as_deref(), Some("普通一句话"), "没有标记，样式/小节都不动（还是上一步设的）");
        assert_eq!(e.subhead.as_deref(), Some("人物关系"), "没有新标记不清空旧 subhead");

        e.apply_marked_text("", 8);
        assert_eq!((e.text.as_deref(), e.status), (None, Status::Pending), "清空文本、没有草稿时退回 Pending");

        e.drafts.push(Draft { text: "草稿".into(), backend: "b".into(), at: 0, hash: "h".into() });
        e.apply_marked_text("  ", 9);
        assert_eq!((e.text.as_deref(), e.status), (None, Status::Draft), "有草稿时清空文本退回 Draft 而不是 Pending");
    }

    #[test]
    fn purge_terminal_removes_only_archived_revoked_skipped() {
        fn entry(id: &str, status: Status) -> Entry {
            let mut e: Entry = serde_json::from_str(&format!(r#"{{"id":"{id}","page":"p","page_index":0,"created":0,"updated":0}}"#)).unwrap();
            e.status = status;
            e
        }
        let mut b = Book {
            uuid: "u".into(),
            title: "t".into(),
            entries: vec![entry("keep-mined", Status::Mined), entry("keep-pending", Status::Pending), entry("keep-reviewed", Status::Reviewed), entry("drop-skipped", Status::Skipped), entry("drop-revoked", Status::Revoked), entry("drop-archived", Status::Archived)],
            ..Default::default()
        };
        let removed = b.purge_terminal();
        assert_eq!(removed, 3);
        let remaining: Vec<&str> = b.entries.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(remaining, ["keep-mined", "keep-pending", "keep-reviewed"]);
        assert_eq!(b.purge_terminal(), 0, "再清一次没东西可清");
    }
}
