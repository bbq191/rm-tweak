//! 带位置的文字抽取（`pdf-extract` 的 OutputDev）与公式区域检测。2026-09-23 起
//! 依赖本地 fork `pdf-extract-cj`（见该 crate `src/lib.rs` 头注释）：颜色 + 图片位置随字符流一起拿，
//! `seq` 是两类事件的公共顺序坐标，`to_epub.rs` 按 `seq` 把文字与图片按文档真实先后交织输出（不再是
//! "图片统一放段末/页末"的近似）。字体/glyph 解码没有动，只是 fork 多传了两样东西出来。

/// 一个提取出的字符 + 它在页面设备空间（pt）里的基线位置 + 设备空间字号 + 定位序号 `line`。
/// ⚠ `line` 是 pdf-extract `end_line()` 的计数（每次 `Td`/`Tm`/`T*` 加一），**不等于视觉行**：
/// 逐字定位的排版（calibre 导出的 PDF 每个字形各一次 `Td`）同一视觉行里每个字的 `line` 都不同。
/// 判断"是不是换了一行"要看基线 `y` 有没有变。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PositionedChar {
    pub ch: char,
    pub x: f64,
    pub y: f64,
    pub font_size: f64,
    pub line: usize,
    /// 文字填充色（RGB）。`None` = fork 没能可靠解析（Pattern/Separation/DeviceN/Lab 这类，见
    /// `pdf_extract_cj::resolve_fill_rgb` 头注释）——调用方遇到 `None` 应该当"不确定"处理，
    /// 不要瞎填一个默认色顶上去。
    pub color: Option<[u8; 3]>,
    /// 全页单调递增的顺序坐标，跟 [`ImageEvent::seq`] 共用同一个计数器——不是字符在文字流里的
    /// 第几个（那用 Vec 下标就够了），是"这个字符在 content stream 里排在第几个会被
    /// `output_character`/`output_image` 报告的事件"，用来给图片在文字流里找回它真实的插入点。
    pub seq: usize,
}

/// 一次 `Do` 算子命中图片 XObject 的记录（`pdf_extract_cj::OutputDev::output_image`）。`ctm` 是
/// 单位正方形 (0,0)-(1,1) 到页面实际矩形的变换（行主序 6 元组，跟 PDF `cm` 算子操作数顺序一致），
/// 用来算图片在页面上的视觉位置（上沿、左边，见 `to_epub::image_visual_pos`），不用来算显示宽高——
/// xochitl 不认内联/精确尺寸，全局 `img{max-width:100%;height:auto}` 已经兜底缩放。
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ImageEvent {
    pub seq: usize,
    pub ctm: [f64; 6],
    /// 页面 `/Resources/XObject` 字典里的资源名（如 `Im1`），不是 PDF 对象 id——`to_epub.rs` 拿它
    /// 反查对象 id 再解码像素，见 `page_image_ids`。
    pub xobject_name: Vec<u8>,
}

/// 一页的抽取结果：字符流 + 图片事件，两者共用 `seq` 保证能按文档真实顺序合并。
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct PageContent {
    pub chars: Vec<PositionedChar>,
    pub images: Vec<ImageEvent>,
}

pub(super) struct TextCollector {
    pages: Vec<PageContent>,
    line: usize,
    /// 逐字符插空格用——逻辑照抄 pdf-extract 自带 `PlainTextOutput` 的做法（不是自己发明的）：
    /// `begin_word()` 只是标一个"下一个字符要检查有没有跳空"的标志位，真正判定空格靠比较
    /// `last_end`（上一个字符右边缘的预期位置）和当前字符实际 x 的差。**踩过的坑**：最初
    /// 版本在 `end_word()` 里无条件插一个空格，结果同一个词内部因为字距微调（kerning）被
    /// PDF 内容流拆成好几个 `Tj`/`TJ` 片段时（专业排版的 PDF 非常常见），每个片段边界都会
    /// 被误当成词边界，插出"In tro duction"这种词中间带空格的乱码（2026-09-19 拿真实
    /// pdflatex 样本跑出来才发现，不是凭空想到的）。改成这套"只在有实际跳空缺口时才插空格"
    /// 的判定后，同一份样本恢复成正常的 "1 Introduction"。
    first_char: bool,
    last_end: f64,
    last_y: f64,
    /// 全页单调递增，字符事件和图片事件共用，见 [`PositionedChar::seq`]。
    seq: usize,
    /// 上一个字符是不是中日韩文字（见 `output_character` 里"中文字之间不补空格"）。
    last_cjk: bool,
}

/// 中日韩统一表意文字、假名、谚文、中日韩标点与全角形式——这些字符之间不用空格分词。
pub(crate) fn is_cjk(c: char) -> bool {
    matches!(c as u32,
        0x3000..=0x303F | 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF
        | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF | 0x20000..=0x2FA1F)
}

impl pdf_extract::OutputDev for TextCollector {
    fn begin_page(&mut self, _page_num: u32, _media_box: &pdf_extract::MediaBox, _art_box: Option<(f64, f64, f64, f64)>) -> Result<(), pdf_extract::OutputError> {
        self.pages.push(PageContent::default());
        self.line = 0;
        self.first_char = false;
        self.last_end = f64::MAX / 2.0;
        self.last_y = 0.0;
        self.seq = 0;
        self.last_cjk = false;
        Ok(())
    }
    fn end_page(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
    fn output_character(&mut self, trm: &pdf_extract::Transform, width: f64, spacing: f64, font_size: f64, ch: &str, color: Option<[u8; 3]>) -> Result<(), pdf_extract::OutputError> {
        let (x, y) = (trm.m31, trm.m32);
        // 换算到设备空间（跟 x/y 同一坐标系）：pdf-extract 给的 `font_size` 是 `Tf` 的文字空间字号、
        // `width` 是千分之一字号单位的字宽，都没乘文本矩阵×CTM 的缩放。calibre 导出的 PDF 整页带
        // `0.742` 缩放（2026-09-23《T.E.双语》），不换算的话字号/前进量全偏大三成，字间缝隙判定失真。
        // 字间距 `Tc`（`spacing`）也是前进量的一部分（PDF 32000-1 §9.4.4）。
        let sx = trm.m11.hypot(trm.m12);
        let sy = trm.m21.hypot(trm.m22);
        let text_fs = font_size;
        let font_size = if sy > 0.0 { text_fs * sy } else { text_fs };
        let advance = (width * text_fs + spacing) * if sx > 0.0 { sx } else { 1.0 };
        let this_cjk = ch.chars().next().map(is_cjk).unwrap_or(false);
        if let Some(page) = self.pages.last_mut() {
            // 中文字之间不因位置缝隙补空格：中文不用空格分词，原书真有的空格是显式字符、原样保留；
            // 逐字定位的排版（calibre 每个字形各一次 Td）字距略松就会被误判成词间空格。
            let both_cjk = this_cjk && self.last_cjk;
            if self.first_char && !both_cjk && x > self.last_end + font_size * 0.1 {
                let line = self.line;
                let seq = self.seq;
                self.seq += 1;
                page.chars.push(PositionedChar { ch: ' ', x: self.last_end, y, font_size, line, color, seq });
            }
            let line = self.line;
            for c in ch.chars() {
                let seq = self.seq;
                self.seq += 1;
                page.chars.push(PositionedChar { ch: c, x, y, font_size, line, color, seq });
            }
        }
        self.first_char = false;
        self.last_y = y;
        self.last_end = x + advance;
        self.last_cjk = ch.chars().last().map(is_cjk).unwrap_or(false);
        Ok(())
    }
    fn begin_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        self.first_char = true;
        Ok(())
    }
    fn end_word(&mut self) -> Result<(), pdf_extract::OutputError> {
        Ok(())
    }
    fn end_line(&mut self) -> Result<(), pdf_extract::OutputError> {
        self.line += 1;
        Ok(())
    }
    fn output_image(&mut self, ctm: &pdf_extract::Transform, xobject_name: &[u8]) -> Result<(), pdf_extract::OutputError> {
        if let Some(page) = self.pages.last_mut() {
            let seq = self.seq;
            self.seq += 1;
            page.images.push(ImageEvent { seq, ctm: [ctm.m11, ctm.m12, ctm.m21, ctm.m22, ctm.m31, ctm.m32], xobject_name: xobject_name.to_vec() });
        }
        Ok(())
    }
}

/// 驱动 pdf-extract 跑一遍 `OutputDev`，拿到每页的逐字符位置流 + 图片事件流（从字节自己解析一遍；
/// 仅测试用；生产调用方手上都已有解析好的 `lopdf::Document`，用 [`extract_positioned_text_doc`]，别再解析第二遍）。
#[cfg(test)]
pub(crate) fn extract_positioned_text(bytes: &[u8]) -> Result<Vec<PageContent>, String> {
    extract_positioned_text_doc(&super::parse_pdf(bytes)?)
}

/// 同 [`extract_positioned_text`]，复用调用方已解析的文档。2026-09-24 起 `pdf-extract-cj` 与本 crate 用同一版
/// lopdf（0.45），类型相同可以直接传；此前两边版本不同，每本 PDF 要整份解析两遍、二进制里也编进两份 lopdf。
pub(crate) fn extract_positioned_text_doc(doc: &lopdf::Document) -> Result<Vec<PageContent>, String> {
    let mut collector = TextCollector { pages: Vec::new(), line: 0, first_char: false, last_end: f64::MAX / 2.0, last_y: 0.0, seq: 0, last_cjk: false };
    pdf_extract::output_doc(doc, &mut collector).map_err(|e| format!("PDF 文字提取失败: {e}"))?;
    Ok(collector.pages)
}

// ============================================================================
// 公式区域探测（纯函数，输入逐字符位置流，输出包围盒——不碰 PDF/渲染，可独立单测）
// ============================================================================

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BBox {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

/// 字符"公式味"判定：**只能靠 Unicode 码位**（pdf-extract 的 `OutputDev` 不暴露字体名，
/// 见模块文档），数学运算符/字母数字符号/希腊字母/常见数学箭头这几个区块命中即算。拿真实
/// pdflatex+amsmath 样本（`tests/fixtures/sample.pdf`）验证过：`∫ ∑ √ ∞ π α β γ ±` 这类
/// 符号确实能提取出正确的 Unicode（Computer Modern 数学字体在这份样本里 ToUnicode 映射
/// 正常），矩阵/分数里的普通字母数字（`a b c d x y`）本身不会被单独命中——这些字符靠下面
/// [`merge_formula_lines`] 的"行合并"机制跟着相邻的强信号符号一起被圈进公式块，不需要
/// 每个字符单独判定。
pub(crate) fn is_formula_char(ch: char) -> bool {
    let cp = ch as u32;
    matches!(cp,
        0x2200..=0x22FF   // Mathematical Operators
        | 0x2A00..=0x2AFF // Supplemental Mathematical Operators
        | 0x27C0..=0x27EF // Miscellaneous Mathematical Symbols-A
        | 0x2980..=0x29FF // Miscellaneous Mathematical Symbols-B
        | 0x2190..=0x21FF // Arrows（数学里常见的 → ⇒ 等）
        | 0x1D400..=0x1D7FF // Mathematical Alphanumeric Symbols
        | 0x0370..=0x03FF // Greek and Coptic（公式里的希腊字母变量）
        | 0x2032..=0x2037 // 撇号类（导数记号 ′ ″）
    )
}

/// 按 `line` 分组：行号升序、行内保持字符原顺序（与"逐个行号 `filter(c.line == n)`"结果完全相同）。
/// 此前公式探测/标题识别都是 `for n in 0..=max_line { chars.iter().filter(|c| c.line == n) }`，复杂度是
/// "行数 × 字符数"——calibre 导出的 PDF 每个字形各一次 `Td`，`line` 几乎逐字递增，一页 2000 字就是 400 万次比较，
/// 整本几百页要白跑几十亿次（2026-09-25 审计）。一遍分组后是线性的。
pub(super) fn group_by_line(chars: &[PositionedChar]) -> std::collections::BTreeMap<usize, Vec<&PositionedChar>> {
    let mut m: std::collections::BTreeMap<usize, Vec<&PositionedChar>> = std::collections::BTreeMap::new();
    for c in chars {
        m.entry(c.line).or_default().push(c);
    }
    m
}

/// 一行是否判定为"公式行"：至少一个字符命中 [`is_formula_char`]。
pub(super) fn line_is_formula(chars: &[&PositionedChar]) -> bool {
    chars.iter().any(|c| is_formula_char(c.ch))
}

pub(super) fn bbox_of(chars: &[&PositionedChar]) -> Option<BBox> {
    if chars.is_empty() {
        return None;
    }
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for c in chars {
        x0 = x0.min(c.x);
        y0 = y0.min(c.y);
        x1 = x1.max(c.x + c.font_size.max(1.0));
        y1 = y1.max(c.y + c.font_size.max(1.0));
    }
    Some(BBox { x0, y0, x1, y1 })
}

/// 每行的平均字号，用来估算"相邻行"的判定阈值（行高的若干倍）。
pub(super) fn avg_font_size(chars: &[&PositionedChar]) -> f64 {
    if chars.is_empty() {
        return 10.0;
    }
    chars.iter().map(|c| c.font_size).sum::<f64>() / chars.len() as f64
}

/// 探测这一页里的公式区域：按 `line` 分组→挑出公式行→纵向相邻（垂直间距小于约 1.5 倍平均
/// 字号）且水平范围有重叠的公式行合并成一个块（覆盖分数/矩阵/多行 `align` 这类跨行公式）。
pub(crate) fn detect_formula_regions(chars: &[PositionedChar]) -> Vec<BBox> {
    if chars.is_empty() {
        return Vec::new();
    }
    let mut formula_lines: Vec<(usize, BBox, f64)> = Vec::new(); // (line_no, bbox, avg_font_size)
    for (line_no, line_chars) in group_by_line(chars) {
        if !line_is_formula(&line_chars) {
            continue;
        }
        if let Some(bbox) = bbox_of(&line_chars) {
            formula_lines.push((line_no, bbox, avg_font_size(&line_chars)));
        }
    }
    if formula_lines.is_empty() {
        return Vec::new();
    }
    // 按行号顺序合并相邻公式行（行号本身就是文档顺序，不需要再按 y 排序）。
    let mut blocks: Vec<BBox> = Vec::new();
    let mut cur = formula_lines[0].1;
    let mut cur_font = formula_lines[0].2;
    let mut prev_line = formula_lines[0].0;
    for (line_no, bbox, font) in formula_lines.into_iter().skip(1) {
        let gap_lines = line_no.saturating_sub(prev_line);
        let x_overlap = bbox.x0 <= cur.x1 + cur_font * 4.0 && bbox.x1 >= cur.x0 - cur_font * 4.0;
        if gap_lines <= 2 && x_overlap {
            cur.x0 = cur.x0.min(bbox.x0);
            cur.y0 = cur.y0.min(bbox.y0);
            cur.x1 = cur.x1.max(bbox.x1);
            cur.y1 = cur.y1.max(bbox.y1);
            cur_font = cur_font.max(font);
        } else {
            blocks.push(cur);
            cur = bbox;
            cur_font = font;
        }
        prev_line = line_no;
    }
    blocks.push(cur);
    // 每个块留一点边距，避免刚好裁掉括号/上下限的边缘。
    for b in blocks.iter_mut() {
        // 留白系数刻意调小（不是 0——分数/矩阵的括号/上下限边缘还是需要一点余量）：真实样本
        // 核对时发现行内公式紧贴正文（如"identity $e^{i\pi}$ is"）时，留白太大会啃掉公式
        // 两侧紧邻的正文字符（2026-09-19 用 sample.pdf 跑出来才发现，不是理论推演）——公式块
        // 边界目前只按整行字符包围盒算，天然比较粗，留白只能保守给一点，不能靠它兜底精确边界。
        let pad = cur_font.max(4.0) * 0.08;
        b.x0 -= pad;
        b.y0 -= pad;
        b.x1 += pad;
        b.y1 += pad;
    }
    blocks
}

// ============================================================================
// 字号识别标题（没有 /Outlines 书签目录时的 TOC 兜底）
// ============================================================================
