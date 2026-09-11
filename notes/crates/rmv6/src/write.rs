//! **只读解析之外的第一小块写能力**：把一组 (样式, 文本) 段落编码成合法的 `.rm` v6 `RootTextBlock`，
//! 拼进一份真机模板文件（模板提供的其余块——AuthorIds/MigrationInfo/PageInfo/SceneInfo/SceneTree/
//! TreeNode/SceneGroupItem——原样字节保留，我们只重新生成文本块本身）。
//!
//! **为什么是"模板替换"而不是从零造整份文件**：`SceneInfo` 块带一段没解出来的不透明字节（真机样本
//! 155 字节，含 paper_size/background_visible 等已知字段之外还有未知内容），从零精确重建风险高；
//! 而这些块与"页面上打的是什么字"无关，从一份已知能被 xochitl 正常打开的真机文件里原样复用最稳。
//! 只有 `RootTextBlock` 是本项目这几轮真机样本已经吃透 wire 格式的部分（`v6::scene_item::text`），
//! 值得也只值得自己写。
//!
//! **wire 格式来自 2026-09-07 逐字段核对**（`testdata/seven_styles/page.rm`，用 rmscene 0.8.0 交叉验证）：
//! - 顶层块头 = `u32 body_len` + `u8 0`(固定) + `u8 min_version` + `u8 current_version` + `u8 block_type`。
//! - `RootTextBlock` body = 打了 tag 的 `block_id`(index 1) + `Text`；`Text` = 嵌套子块 1（含子块 1→条目表）
//!   + 子块 2（含子块 1→样式表）+ 子块 3（裸 `x,y` 两个 f64，不打 tag）+ 打了 tag 的 `width`(index 4)。
//! - 条目：每段一条，`item_id` 指向本段第一个字符，后续字符隐式 +1；`left_id` = 上一段最后一个字符的
//!   id（第一段用 `(0,0)`）；`right_id` 恒 `(0,0)`；正文自带尾随换行。
//! - 样式：key = "结束上一段的换行符" 的 id（即本段 `left_id` 原样），value 是 `{c:17固定, 样式码}`。
//! - 全新文档没有编辑历史，`deleted_length` 恒 0、id 不留空隙——这是本模块能确定性生成的原因（真机
//!   活文档里那些"隔一个 id"的空隙是打字/删改历史的产物，我们不需要模拟）。
//!
//! ✅ **2026-09-07 真机验证通过**：6 段/5 样式测试文档经 `note-serve::rmdoc` 打包、`rmsvc_core::
//! xochitl::Xochitl::upload` 传到真机，xochitl 自己渲染的缩略图肉眼核对——大标题/加粗小标题/正文
//! 换行/无序两点/空心待办全部渲染正确，无白屏无错位（笔记线白皮书 §03h）。
//!
//! ⚠️ **更正一处早前的误判（§03h 当时写的"NUMBERED 多 7 字节未解码载荷"是错的）**：那 7 字节其实
//! 属于 **Subheading 1**，不是 NumberedList——早前分析漏看了是哪个 char_id 拥有那段多余字节，
//! 错怪到了旁边的 NUMBERED 条目头上。2026-09-07 用户在真机上把这份测试文档手动加了真正的原生
//! "Subheading 1"/"已编号列表"/"复选框(勾选)"，拉回来逐条目核对字节才理清：
//! - **NUMBERED（10）格式子块跟其余样式一样只有 2 字节**（`17` + 样式码），没有隐藏内容——本模块
//!   现在正常支持它，写出来的编号由 xochitl 在渲染时按"连续几个 NUMBERED 段落"自动算，不用自己存序号。
//! - **Subheading 1 与 Subheading 2 真机确实共用同一个码（BOLD=3），区分开关是格式子块末尾多出来的
//!   固定 7 字节 `21 02 34 03 00 00 00`**——真机原生按钮打出的 "Subheading 1" 带这 7 字节、渲染成
//!   大字号；"Subheading 2"（以及裸 `BOLD`）不带，渲染成小字号。跟段落在文档里的位置**无关**（用户
//!   明确验证过）。`Paragraph::subheading1()` 补这 7 字节；`Paragraph::new(BOLD, ..)` 保持不带，
//!   语义上当"小节标题"（Subheading 2）用。
//!
//! ✅ **两条更正当场用本模块自己生成的文档二次真机验证过**：8 段测试文档（含 `subheading1()` 一段、
//! 裸 `BOLD` 一段、`NUMBERED` 两段）传真机，xochitl 渲染缩略图——"真 Subheading 1" 明显大字号、
//! "裸 BOLD" 明显小字号，两段有序列表自动编号 "1." "2." 正确显示，其余样式同前一轮全部正确。
//! 至此 xochitl 3.28 打字格式菜单的 7 种样式（含 Subheading 1/2 两级）本模块全部支持。
use crate::v6::scene_item::text::ParagraphStyle;
use crate::v6::crdt::CrdtId;

/// 真机原生 "Subheading 1" 按钮打出来的段落，格式子块比裸 `BOLD` 多这 7 字节（2026-09-07 真机
/// 双样本逐字节对照坐实，见模块文档）；`Paragraph::subheading1` 用它跟 `new(BOLD, ..)`（=Subheading 2）区分。
pub const SUBHEADING1_MARKER: [u8; 7] = [0x21, 0x02, 0x34, 0x03, 0x00, 0x00, 0x00];

/// 待写入的一段文字。
#[derive(Debug, Clone)]
pub struct Paragraph {
    pub style: ParagraphStyle,
    /// 不含尾随换行——写入时自动补一个 `\n`。
    pub text: String,
    /// 格式子块（`[17, 样式码]`）末尾的额外字节，目前只有 `subheading1()` 会填，其余样式留空。
    extra: Vec<u8>,
}

impl Paragraph {
    pub fn new(style: ParagraphStyle, text: impl Into<String>) -> Self {
        Paragraph { style, text: text.into(), extra: Vec::new() }
    }

    /// 真正的"大字号" Subheading 1（分区头）。裸 `new(ParagraphStyle::BOLD, ..)` 渲染成小字号的
    /// Subheading 2——两者 wire 码相同，全靠这 7 字节区分，见模块文档。
    pub fn subheading1(text: impl Into<String>) -> Self {
        Paragraph { style: ParagraphStyle::BOLD, text: text.into(), extra: SUBHEADING1_MARKER.to_vec() }
    }
}

/// 生成 RootTextBlock 用的 author 索引；必须与模板文件 `AuthorIdsBlock` 里声明的索引一致
/// （真机样本固定为 1，模板不换的话这个值也不用换）。
const AUTHOR: u8 = 1;
/// 文本框位置/宽度：沿用模板真机页面自己的值（页宽 740、水平居中：-370..370）。
const TEXT_X: f64 = -370.0;
const TEXT_Y: f64 = 234.0;
const TEXT_WIDTH: f32 = 740.0;

const TAG_BYTE4: u32 = 0x4;
const TAG_LEN4: u32 = 0xC;
const TAG_ID: u32 = 0x0F;

/// 最小字节写入器：定长小端 + 无符号 LEB128 varuint，和 `bitreader::Bitreader` 的读法一一对应。
struct W(Vec<u8>);
impl W {
    fn new() -> Self {
        W(Vec::new())
    }
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f64(&mut self, v: f64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn bytes(&mut self, b: &[u8]) {
        self.0.extend_from_slice(b);
    }
    fn varuint(&mut self, mut v: u32) {
        loop {
            let mut b = (v & 0x7F) as u8;
            v >>= 7;
            if v != 0 {
                b |= 0x80;
            }
            self.u8(b);
            if v == 0 {
                break;
            }
        }
    }
    fn tag(&mut self, index: u32, ty: u32) {
        self.varuint((index << 4) | ty);
    }
    fn id_tagged(&mut self, index: u32, id: CrdtId) {
        self.tag(index, TAG_ID);
        self.u8(id.part1);
        self.varuint(id.part2);
    }
    fn id_raw(&mut self, id: CrdtId) {
        self.u8(id.part1);
        self.varuint(id.part2);
    }
    fn u32_tagged(&mut self, index: u32, v: u32) {
        self.tag(index, TAG_BYTE4);
        self.u32(v);
    }
    fn f32_tagged(&mut self, index: u32, v: f32) {
        self.tag(index, TAG_BYTE4);
        self.f32(v);
    }
    /// 打 tag + 4 字节长度前缀 + 内容——子块本身不递归構建，调用方先把内容拼好再传进来。
    fn subblock(&mut self, index: u32, content: &[u8]) {
        self.tag(index, TAG_LEN4);
        self.u32(content.len() as u32);
        self.bytes(content);
    }
    fn into_vec(self) -> Vec<u8> {
        self.0
    }
}

fn style_code(s: ParagraphStyle) -> Result<u8, String> {
    Ok(match s {
        ParagraphStyle::PLAIN => 0x01,
        ParagraphStyle::HEADING => 0x02,
        ParagraphStyle::BOLD => 0x03,
        ParagraphStyle::BULLET => 0x04,
        ParagraphStyle::CHECKBOX => 0x06,
        ParagraphStyle::NUMBERED => 0x0a,
        other => return Err(format!("不支持写入这个样式：{other:?}")),
    })
}

/// 编码一份完整的 `RootTextBlock`（含它自己的顶层块头），可以直接拼进一份 `.rm` 文件的块序列里。
pub fn encode_root_text_block(paragraphs: &[Paragraph]) -> Result<Vec<u8>, String> {
    if paragraphs.is_empty() {
        return Err("至少要有一段文字".into());
    }

    let mut items_content = W::new();
    let mut styles_content = W::new();

    let mut next_id: u32 = 1;
    let mut prev_last = CrdtId { part1: 0, part2: 0 }; // 首段用 (0,0)，即 END_MARKER

    for p in paragraphs {
        let code = style_code(p.style)?;
        let text = format!("{}\n", p.text);
        let char_count = text.chars().count() as u32;
        if char_count == 0 {
            return Err("段落文本不能为空".into());
        }
        let item_id = CrdtId { part1: AUTHOR, part2: next_id };
        let left_id = prev_last;

        // 条目：subblock(0, id(2,item_id)+id(3,left_id)+id(4,right_id)+u32(5,0)+subblock(6,文本))
        let mut item_body = W::new();
        item_body.id_tagged(2, item_id);
        item_body.id_tagged(3, left_id);
        item_body.id_tagged(4, CrdtId::default());
        item_body.u32_tagged(5, 0);
        let mut text_body = W::new();
        text_body.varuint(text.len() as u32); // 字节长度，不是字符数
        // ⚠ 这个字段名叫 is_ascii，但真机样本（含中文的段落）也写 1——不是"内容是否纯 ASCII"，
        // 交叉验证用独立的 Python rmscene 复现过：写 0（老实按内容判断）会在它的 assert is_ascii==1
        // 上炸掉，说明这是个真机恒为 1 的字段，不能按字面意思算。写死 1，别"自作聪明"按内容判断。
        text_body.u8(1u8);
        text_body.bytes(text.as_bytes());
        item_body.subblock(6, &text_body.into_vec());
        items_content.subblock(0, &item_body.into_vec());

        // 样式：raw id(段头换行的 id，即本段 left_id) + tagged id(1,timestamp) + subblock(2,[17, code, ...extra])
        styles_content.id_raw(left_id);
        styles_content.id_tagged(1, item_id); // timestamp 只要单调、不冲突即可，用 item_id 本身
        let mut fmt = vec![17u8, code];
        fmt.extend_from_slice(&p.extra);
        styles_content.subblock(2, &fmt);

        next_id += char_count;
        prev_last = CrdtId { part1: AUTHOR, part2: next_id - 1 };
    }

    let mut items_full = W::new();
    items_full.varuint(paragraphs.len() as u32);
    items_full.bytes(&items_content.into_vec());

    let mut styles_full = W::new();
    styles_full.varuint(paragraphs.len() as u32);
    styles_full.bytes(&styles_content.into_vec());

    // subblock3(tag1) 包 items；subblock2(tag1) 包 subblock3——两层严格按真机 wire 格式来，见模块文档。
    let mut sub3 = W::new();
    sub3.subblock(1, &items_full.into_vec());
    let sub2_payload = sub3.into_vec();
    let mut sub2 = W::new();
    sub2.subblock(1, &sub2_payload);

    let mut sub5 = W::new();
    sub5.subblock(1, &styles_full.into_vec());
    let sub4_payload = sub5.into_vec();
    let mut sub4 = W::new();
    sub4.subblock(2, &sub4_payload);

    let mut sub1_payload = sub2.into_vec();
    sub1_payload.extend_from_slice(&sub4.into_vec());
    let mut sub1 = W::new();
    sub1.subblock(2, &sub1_payload);

    // subblock7：裸 x,y（不打 tag），随后打 tag 的 width。
    let mut sub7_payload = W::new();
    sub7_payload.f64(TEXT_X);
    sub7_payload.f64(TEXT_Y);
    let mut sub7 = W::new();
    sub7.subblock(3, &sub7_payload.into_vec());

    let mut body = W::new();
    body.id_tagged(1, CrdtId::default()); // block_id，真机样本恒 (0,0)
    body.bytes(&sub1.into_vec());
    body.bytes(&sub7.into_vec());
    body.f32_tagged(4, TEXT_WIDTH);
    let body = body.into_vec();

    let mut full = W::new();
    full.u32(body.len() as u32);
    full.u8(0);
    full.u8(1); // min_version（真机样本值）
    full.u8(1); // current_version
    full.u8(7); // block_type = RootTextBlock
    full.bytes(&body);
    Ok(full.into_vec())
}

/// 在模板文件的顶层块序列里找 `RootTextBlock`（block_type=7），返回它的字节区间
/// `[起点, 终点)`（含它自己的 8 字节块头）。模板必须已经带一个打字文本块。
fn find_root_text_span(template: &[u8]) -> Result<(usize, usize), String> {
    const HEADER_LEN: usize = 43; // "reMarkable .lines file, version=6" + 空格补齐
    if template.len() < HEADER_LEN {
        return Err("模板文件太短，不像 .rm".into());
    }
    let mut pos = HEADER_LEN;
    while pos + 8 <= template.len() {
        let size = u32::from_le_bytes(template[pos..pos + 4].try_into().unwrap()) as usize;
        let block_type = template[pos + 7];
        let body_start = pos + 8;
        let body_end = body_start.checked_add(size).ok_or("模板块长度溢出")?;
        if body_end > template.len() {
            return Err("模板块声明长度超出文件".into());
        }
        if block_type == 7 {
            return Ok((pos, body_end));
        }
        pos = body_end;
    }
    Err("模板里没有 RootTextBlock（block_type=7），换一份带打字文本的模板".into())
}

/// 拿一份真机模板 `.rm` 字节，把它的 `RootTextBlock` 换成新内容，其余块原样保留。
pub fn build_page_rm(template: &[u8], paragraphs: &[Paragraph]) -> Result<Vec<u8>, String> {
    let (start, end) = find_root_text_span(template)?;
    let new_block = encode_root_text_block(paragraphs)?;
    let mut out = Vec::with_capacity(template.len() - (end - start) + new_block.len());
    out.extend_from_slice(&template[..start]);
    out.extend_from_slice(&new_block);
    out.extend_from_slice(&template[end..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::Page;

    const TEMPLATE: &[u8] = include_bytes!("../../../testdata/seven_styles/page.rm");

    #[test]
    fn rejects_empty() {
        assert!(encode_root_text_block(&[]).is_err());
    }

    #[test]
    fn roundtrips_through_our_own_parser() {
        let paragraphs = vec![
            Paragraph::new(ParagraphStyle::HEADING, "第一章 起点"),
            Paragraph::subheading1("查询"),
            Paragraph::new(ParagraphStyle::PLAIN, "这是转写出来的正文，混着中文和 English。"),
            Paragraph::new(ParagraphStyle::BULLET, "无序要点"),
            Paragraph::new(ParagraphStyle::NUMBERED, "有序要点"),
            Paragraph::new(ParagraphStyle::CHECKBOX, "待办事项"),
        ];
        let rm = build_page_rm(TEMPLATE, &paragraphs).expect("build");
        let page = Page::parse(&rm).expect("我们自己的解析器应该能读回来");
        let text = page.text.expect("打字文本块应该在");
        assert_eq!(text.items.items.len(), paragraphs.len(), "条目数应等于段落数");
        assert_eq!(text.styles.len(), paragraphs.len(), "样式数应等于段落数");

        // 校验每段的文本原样在、样式码原样在。
        let mut expect_start = CrdtId { part1: 1, part2: 1 };
        let mut prev_last = CrdtId::default();
        for p in &paragraphs {
            let item = text.items.items.get(&expect_start).unwrap_or_else(|| panic!("缺条目 {expect_start:?}"));
            assert_eq!(item.left_id, prev_last);
            let want_text = format!("{}\n", p.text);
            match &item.value {
                crate::v6::scene_item::text::TextItem::Text(s) => assert_eq!(s, &want_text),
                other => panic!("应是纯文本条目，得到 {other:?}"),
            }
            let style = text.styles.get(&prev_last).unwrap_or_else(|| panic!("缺样式键 {prev_last:?}"));
            assert_eq!(style.value, p.style);

            let n = want_text.chars().count() as u32;
            prev_last = CrdtId { part1: 1, part2: expect_start.part2 + n - 1 };
            expect_start = CrdtId { part1: 1, part2: expect_start.part2 + n };
        }
    }

    #[test]
    fn missing_root_text_block_in_template_errors() {
        let err = build_page_rm(b"reMarkable .lines file, version=6           ", &[Paragraph::new(ParagraphStyle::PLAIN, "x")]).unwrap_err();
        assert!(err.contains("模板"));
    }
}
