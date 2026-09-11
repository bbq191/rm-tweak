//! 几何：同一页坐标系里，把手写笔画聚成"一片"（簇），再给每簇找它旁边的勾画。
//! 只用包围盒间距，不看笔序、不看时间（用户会回头补笔）。阈值以 `.rm` 原始页坐标单位计（**不是像素**——
//! EPUB 页的坐标系是排版引擎自己的虚拟画布，真机实测 960×1280，见 `ink-serve::crop` 与白皮书 §03g；
//! 聚簇/配对只比坐标间的相对距离，不需要知道画布真实尺寸，不受这个换算影响）。
//! 缺省 `cluster_gap=40`/`pair_gap=120`，2026-09-07 真机样本验证有效（§03f）。
use rmv6::page::{BBox, Highlight, Stroke};

/// 聚簇/配对阈值。
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// 两笔包围盒间距 ≤ 此值视为同一片手写（字与字、行与行之间的距离）。
    pub cluster_gap: f32,
    /// 簇到勾画矩形的间距 ≤ 此值才算"写在旁边"；超过归为本页批注。
    pub pair_gap: f32,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds { cluster_gap: 40.0, pair_gap: 120.0 }
    }
}

/// 一片手写：笔画下标 + 合并包围盒。
#[derive(Debug, Clone, PartialEq)]
pub struct Cluster {
    pub strokes: Vec<usize>,
    pub bbox: BBox,
}

/// 只把"手写"笔画拿来聚簇：荧光笔/橡皮/选区不算（勾画走 GlyphRange，自由荧光笔留给以后）。
pub fn is_handwriting(s: &Stroke) -> bool {
    use rmv6::shared::tool::Tool;
    !matches!(s.tool, Tool::Highlighter | Tool::Eraser | Tool::EraseArea | Tool::EraseAll | Tool::SelectionBrush)
}

/// 并查集聚簇：任意两笔间距 ≤ gap 连通。返回按 bbox 上沿 y 排序的簇。
pub fn cluster(strokes: &[Stroke], th: &Thresholds) -> Vec<Cluster> {
    let idx: Vec<usize> = (0..strokes.len()).filter(|&i| is_handwriting(&strokes[i])).collect();
    let mut parent: Vec<usize> = (0..idx.len()).collect();
    fn find(p: &mut Vec<usize>, i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        let mut c = i;
        while p[c] != r {
            let n = p[c];
            p[c] = r;
            c = n;
        }
        r
    }
    for a in 0..idx.len() {
        for b in (a + 1)..idx.len() {
            if strokes[idx[a]].bbox.gap(&strokes[idx[b]].bbox) <= th.cluster_gap {
                let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
                if ra != rb {
                    parent[ra] = rb;
                }
            }
        }
    }
    let mut groups: std::collections::BTreeMap<usize, Vec<usize>> = Default::default();
    for a in 0..idx.len() {
        let r = find(&mut parent, a);
        groups.entry(r).or_default().push(idx[a]);
    }
    let mut out: Vec<Cluster> = groups
        .into_values()
        .map(|members| {
            let bbox = members.iter().map(|&i| strokes[i].bbox).reduce(BBox::union).expect("非空簇");
            Cluster { strokes: members, bbox }
        })
        .collect();
    out.sort_by(|a, b| a.bbox.y0.partial_cmp(&b.bbox.y0).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// 每簇 → 最近的勾画下标（间距 ≤ pair_gap），否则 None（本页批注）。
pub fn pair(clusters: &[Cluster], highlights: &[Highlight], th: &Thresholds) -> Vec<Option<usize>> {
    clusters
        .iter()
        .map(|c| {
            highlights
                .iter()
                .enumerate()
                .map(|(i, h)| (i, c.bbox.gap(&h.bbox)))
                .filter(|&(_, g)| g <= th.pair_gap)
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
                .map(|(i, _)| i)
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod fixtures {
    use rmv6::page::{BBox, Highlight, Stroke};
    use rmv6::shared::pen_color::PenColor;
    use rmv6::shared::tool::Tool;
    use rmv6::v6::crdt::CrdtId;

    pub fn stroke(id: u32, tool: Tool, x0: f32, y0: f32, x1: f32, y1: f32) -> Stroke {
        Stroke { id: CrdtId { part1: 1, part2: id }, parent: CrdtId::default(), tool, color: PenColor::Black, thickness: 1.0, points: vec![(x0, y0), (x1, y1)], bbox: BBox { x0, y0, x1, y1 } }
    }
    pub fn hl(id: u32, text: &str, x: f32, y: f32, w: f32, h: f32) -> Highlight {
        Highlight { id: CrdtId { part1: 1, part2: id }, color: PenColor::Yellow, rgba: None, start: 0, length: text.chars().count() as u32, text: text.into(), rects: vec![(x, y, w, h)], bbox: BBox { x0: x, y0: y, x1: x + w, y1: y + h } }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use rmv6::shared::tool::Tool;

    #[test]
    fn clusters_by_gap_and_skips_non_handwriting() {
        let strokes = vec![
            stroke(1, Tool::BallPoint, 100.0, 100.0, 120.0, 130.0),
            stroke(2, Tool::BallPoint, 130.0, 100.0, 150.0, 130.0), // 与 1 相距 10 → 同簇
            stroke(3, Tool::BallPoint, 100.0, 400.0, 150.0, 430.0), // 远 → 另一簇
            stroke(4, Tool::Highlighter, 100.0, 105.0, 400.0, 125.0), // 荧光笔不聚
            stroke(5, Tool::BallPoint, 160.0, 100.0, 180.0, 130.0), // 与 2 相距 10 → 链式并入簇 1
        ];
        let cs = cluster(&strokes, &Thresholds::default());
        assert_eq!(cs.len(), 2);
        assert_eq!(cs[0].strokes, vec![0, 1, 4]);
        assert_eq!(cs[0].bbox, BBox { x0: 100.0, y0: 100.0, x1: 180.0, y1: 130.0 });
        assert_eq!(cs[1].strokes, vec![2]);
    }

    #[test]
    fn pairs_cluster_to_nearest_highlight_within_gap() {
        let strokes = vec![stroke(1, Tool::BallPoint, 900.0, 300.0, 1000.0, 340.0), stroke(2, Tool::BallPoint, 100.0, 1500.0, 200.0, 1530.0)];
        let hls = vec![hl(10, "第一段", 100.0, 200.0, 700.0, 30.0), hl(11, "第二段", 100.0, 320.0, 780.0, 30.0)];
        let cs = cluster(&strokes, &Thresholds::default());
        let p = pair(&cs, &hls, &Thresholds::default());
        assert_eq!(p, vec![Some(1), None], "簇 1 挨着第二段（间距 20）；簇 2 离得远 → 本页批注");
    }

    // 原测试 real_device_sample_clusters_and_pairs_correctly 依赖真机《人骨拼圖》fixture
    // （testdata/renggu_marks/page.rm）验证真实几何数据下的聚簇+配对，公开发行版不带这份含
    // 真实版权小说原文的夹具，整个删掉；私有开发仓库这份测试原样保留。
}
