//! 高层视图：一页 `.rm` = 笔画 + 勾画 + 打字文本（墓碑已剔除，坐标原样、同一坐标系）。
//! ink-serve 只看这层：勾画矩形与笔画点在同一页坐标系里，几何配对不需要任何换算。
use crate::shared::pen_color::PenColor;
use crate::shared::tool::Tool;
use crate::v6::block::Block;
use crate::v6::crdt::CrdtId;
use crate::v6::scene_item::text::Text;
use crate::{ParseError, RmFile};

/// 轴对齐包围盒（页坐标）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl BBox {
    pub fn of_points<I: IntoIterator<Item = (f32, f32)>>(pts: I) -> Option<BBox> {
        let mut b: Option<BBox> = None;
        for (x, y) in pts {
            b = Some(match b {
                None => BBox { x0: x, y0: y, x1: x, y1: y },
                Some(c) => BBox { x0: c.x0.min(x), y0: c.y0.min(y), x1: c.x1.max(x), y1: c.y1.max(y) },
            });
        }
        b
    }
    pub fn union(self, o: BBox) -> BBox {
        BBox { x0: self.x0.min(o.x0), y0: self.y0.min(o.y0), x1: self.x1.max(o.x1), y1: self.y1.max(o.y1) }
    }
    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn center(&self) -> (f32, f32) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
    /// 两盒的最短间距（相交/相接为 0）。
    pub fn gap(&self, o: &BBox) -> f32 {
        let dx = (o.x0 - self.x1).max(self.x0 - o.x1).max(0.0);
        let dy = (o.y0 - self.y1).max(self.y0 - o.y1).max(0.0);
        (dx * dx + dy * dy).sqrt()
    }
}

/// 一条笔画（`SceneLineItem`，未删除）。
#[derive(Debug, Clone)]
pub struct Stroke {
    pub id: CrdtId,
    /// 所属组（图层 / 锚定组）。
    pub parent: CrdtId,
    pub tool: Tool,
    pub color: PenColor,
    pub thickness: f64,
    pub points: Vec<(f32, f32)>,
    pub bbox: BBox,
}

/// 一段勾画（`SceneGlyphItem` = GlyphRange，未删除）：原文 + 在页文本里的偏移 + 覆盖矩形。
#[derive(Debug, Clone)]
pub struct Highlight {
    pub id: CrdtId,
    pub color: PenColor,
    pub rgba: Option<(u8, u8, u8, u8)>,
    pub start: u32,
    pub length: u32,
    pub text: String,
    /// 每行一个矩形（x, y, w, h）。
    pub rects: Vec<(f32, f32, f32, f32)>,
    pub bbox: BBox,
}

#[derive(Debug, Default)]
pub struct Page {
    pub strokes: Vec<Stroke>,
    pub highlights: Vec<Highlight>,
    /// 打字文本（笔记本页才有；EPUB 书页无）。
    pub text: Option<Text>,
}

impl Page {
    pub fn parse(bytes: &[u8]) -> Result<Page, ParseError> {
        Ok(Page::from_file(&RmFile::read(bytes)?))
    }

    pub fn from_file(f: &RmFile) -> Page {
        let mut p = Page::default();
        for b in &f.blocks {
            match b {
                Block::SceneLineItem(it) => {
                    if let Some(line) = &it.item.value {
                        let points: Vec<(f32, f32)> = line.points.iter().map(|pt| (pt.x, pt.y)).collect();
                        let Some(bbox) = BBox::of_points(points.iter().copied()) else { continue };
                        p.strokes.push(Stroke { id: it.item.item_id, parent: it.parent_id, tool: line.tool.clone(), color: line.color.clone(), thickness: line.thickness_scale, points, bbox });
                    }
                }
                Block::SceneGlyphItem(it) => {
                    if let Some(g) = &it.item.value {
                        let rects: Vec<(f32, f32, f32, f32)> = g.rectangles.iter().map(|r| (r.x as f32, r.y as f32, r.w as f32, r.h as f32)).collect();
                        let Some(bbox) = BBox::of_points(rects.iter().flat_map(|&(x, y, w, h)| [(x, y), (x + w, y + h)])) else { continue };
                        p.highlights.push(Highlight { id: it.item.item_id, color: g.color.clone(), rgba: g.color_rgba, start: g.start, length: g.length, text: g.text.clone(), rects, bbox });
                    }
                }
                Block::RootText(t) => p.text = Some(t.text.clone()),
                _ => {}
            }
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bbox_math() {
        let a = BBox::of_points([(0.0, 0.0), (10.0, 5.0)]).unwrap();
        let b = BBox { x0: 13.0, y0: 9.0, x1: 20.0, y1: 12.0 };
        assert_eq!((a.width(), a.height(), a.center()), (10.0, 5.0, (5.0, 2.5)));
        assert_eq!(a.gap(&b), 5.0, "dx=3 dy=4 → 5");
        assert_eq!(a.gap(&a), 0.0);
        assert_eq!(a.union(b), BBox { x0: 0.0, y0: 0.0, x1: 20.0, y1: 12.0 });
        assert!(BBox::of_points(std::iter::empty()).is_none());
    }

    // 原测试 parses_tombstone_only_page_to_empty / parses_real_highlight_and_handwriting_page
    // 都依赖真机《人骨拼圖》fixture（testdata/renggu(_marks)/page.rm）验证墓碑页/高亮+手写页解析，
    // 公开发行版不带这份含真实版权小说原文的夹具，两个测试的 fixture 相关部分都删掉；前一个测试
    // 末尾不依赖 fixture 的版本号校验单独留一个测试。私有开发仓库这两份测试原样保留。
    #[test]
    fn parse_rejects_non_v6_version_string() {
        assert!(Page::parse(b"reMarkable .lines file, version=5           ").is_err(), "只认 v6");
    }

    #[test]
    fn parses_seven_style_notebook_page() {
        // 真机 2026-09-07 步骤 0 样本：格式菜单逐行打 Title/Subheading 1/Subheading 2/Body/
        // Bulletpoint/NumberedList/Checkbox(×2)。坐实 NumberedList 码=10；且 Subheading 1/2
        // 在 .rm 层用的是**同一个码**（BOLD=3）——原生靠别的机制区分两级大小，不能只凭这个码分层级。
        use crate::v6::scene_item::text::ParagraphStyle as PS;
        let bytes = include_bytes!("../../../testdata/seven_styles/page.rm");
        let p = Page::parse(bytes).unwrap();
        let styles: Vec<&PS> = p.text.as_ref().unwrap().styles.values().map(|lww| &lww.value).collect();
        let count = |want: &PS| styles.iter().filter(|s| std::mem::discriminant(**s) == std::mem::discriminant(want)).count();
        assert_eq!(count(&PS::HEADING), 1, "Title");
        assert_eq!(count(&PS::BOLD), 2, "Subheading 1 + Subheading 2 共用一个码");
        assert_eq!(count(&PS::BULLET), 1, "Bulletpoint");
        assert_eq!(count(&PS::NUMBERED), 1, "NumberedList");
        assert_eq!(count(&PS::CHECKBOX), 2, "Checkbox 未勾选 ×2（打字打不出勾上号 7）");
        assert_eq!(count(&PS::CHECKBOX_CHECKED), 0);
    }
}
