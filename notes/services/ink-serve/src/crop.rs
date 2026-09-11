//! 裁图：把一片手写变成一张 PNG 喂给视觉模型。**自渲染**（2026-09-07 二期真机验证后换的）：直接拿
//! `.rm` 笔画矢量数据（`points`）自己画折线，不依赖 xochitl 现成的页缩略图——天然不受"缩略图只画了
//! 生成那一刻视口内看得到的部分"这条限制（矢量坐标没有滚动上限，手写位置再靠下也画得出来），背景纯白
//! 也不会像贴着印刷勾画行裁缩略图那样把印刷体也带进去。真机踩过这两个坑，细节见笔记线白皮书 §03o。
//!
//! 旧方案是吃 xochitl 缩略图裁（`page_w`/`page_h` 是 EPUB 排版引擎的虚拟画布尺寸，真机实测 **960×1280**，
//! 不是物理屏 1404×1872——白皮书 §03g 记过"用错了裁图整体裁偏"的真机事故），这份坐标标定的知识仍然
//! 保留在 `config.rs::IngestConfig` 的字段与文档里，代码本身随缩略图路径一起退役了（改自渲染后没有
//! 消费者，`cargo build` 会报 dead_code——两条路径不值得同时维护，出问题回这段历史记录找）。
use image::{DynamicImage, ImageFormat};
use rmv6::page::Stroke;
use std::io::Cursor;

/// 裁剪区域小于这个像素数就当"画不出来"（笔画包围盒退化成一个点之类）。
const MIN_CROP_PX: u32 = 8;
/// 页坐标单位 → 自渲染像素的缩放：比旧的缩略图路径（约 0.4，384/960）给得更细，手写笔画看着更清楚。
const RENDER_SCALE: f32 = 2.0;
/// 画笔粗细（像素），凭手写笔画常见粗细估的，不是从设备笔迹参数精确反推的。
const LINE_WIDTH_PX: f32 = 1.6;

/// 自渲染裁图：从 `.rm` 笔画矢量数据里挑出 `stroke_ids` 指定的那些笔画，在它们的 `bbox`（留 `margin`
/// 页坐标单位的白边）范围内画折线，输出白底黑线 PNG。
pub fn render_ink(strokes: &[Stroke], stroke_ids: &[String], bbox: (f32, f32, f32, f32), margin: f32) -> Result<Vec<u8>, String> {
    let ids: std::collections::BTreeSet<&str> = stroke_ids.iter().map(String::as_str).collect();
    let picked: Vec<&Stroke> = strokes.iter().filter(|s| ids.contains(s.id.to_string().as_str())).collect();
    if picked.is_empty() {
        return Err("这片手写在当前页里一笔都没找到（笔画 id 对不上）".into());
    }
    let (x0, y0, x1, y1) = (bbox.0 - margin, bbox.1 - margin, bbox.2 + margin, bbox.3 + margin);
    let w = (((x1 - x0) * RENDER_SCALE).ceil() as i64).max(1) as u32;
    let h = (((y1 - y0) * RENDER_SCALE).ceil() as i64).max(1) as u32;
    if w < MIN_CROP_PX || h < MIN_CROP_PX {
        return Err(format!("笔画包围盒只有 {w}x{h} px，太小画不出来"));
    }
    let mut img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 255, 255, 255]));
    let to_px = |p: (f32, f32)| -> (f64, f64) { (((p.0 - x0) * RENDER_SCALE) as f64, ((p.1 - y0) * RENDER_SCALE) as f64) };
    for s in picked {
        if s.points.len() < 2 {
            // 单点笔画（一个点/一个墨点）：就画一个点大小的圆点，别整段跳过。
            if let Some(&p) = s.points.first() {
                let (x, y) = to_px(p);
                stamp_dot(&mut img, x, y, (LINE_WIDTH_PX / 2.0) as f64);
            }
            continue;
        }
        for win in s.points.windows(2) {
            let (a, b) = (to_px(win[0]), to_px(win[1]));
            draw_thick_segment(&mut img, a, b, (LINE_WIDTH_PX / 2.0) as f64);
        }
    }
    let mut out = Vec::new();
    DynamicImage::ImageRgba8(img).write_to(&mut Cursor::new(&mut out), ImageFormat::Png).map_err(|e| format!("裁图编码失败: {e}"))?;
    Ok(out)
}

/// 两点间按步长插值、逐点画圆点，拼出一条有粗细的线段（不追求抗锯齿，喂视觉模型够用）。
fn draw_thick_segment(img: &mut image::RgbaImage, a: (f64, f64), b: (f64, f64), r: f64) {
    let dist = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
    let steps = (dist.ceil() as i64).max(1);
    for i in 0..=steps {
        let t = i as f64 / steps as f64;
        stamp_dot(img, a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, r);
    }
}

/// 在 (cx, cy) 处画一个半径 r 的实心圆点，越界部分自动裁掉。
fn stamp_dot(img: &mut image::RgbaImage, cx: f64, cy: f64, r: f64) {
    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return;
    }
    let r = r.max(0.5);
    let x0 = (cx - r).floor().max(0.0) as u32;
    let x1 = ((cx + r).ceil() as i64).min(w as i64 - 1).max(0) as u32;
    let y0 = (cy - r).floor().max(0.0) as u32;
    let y1 = ((cy + r).ceil() as i64).min(h as i64 - 1).max(0) as u32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (dx, dy) = (x as f64 - cx, y as f64 - cy);
            if dx * dx + dy * dy <= r * r {
                img.put_pixel(x, y, image::Rgba([0, 0, 0, 255]));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::GenericImageView;
    use rmv6::v6::crdt::CrdtId;

    fn stroke(id: u32, pts: &[(f32, f32)]) -> Stroke {
        Stroke { id: CrdtId { part1: 1, part2: id }, parent: CrdtId::default(), tool: rmv6::shared::tool::Tool::BallPoint, color: rmv6::shared::pen_color::PenColor::Black, thickness: 1.0, points: pts.to_vec(), bbox: rmv6::page::BBox::of_points(pts.iter().copied()).unwrap() }
    }

    #[test]
    fn renders_a_clean_white_background_line_regardless_of_position() {
        // 页坐标随便给多靠下（旧的缩略图路径在这个 y 会因为超出视口裁不到），自渲染不受影响。
        let s = stroke(1, &[(0.0, 5000.0), (40.0, 5010.0), (80.0, 5000.0)]);
        let ids = vec!["1:1".to_string()];
        let png = render_ink(&[s], &ids, (0.0, 5000.0, 80.0, 5010.0), 4.0).unwrap();
        let img = image::load_from_memory_with_format(&png, ImageFormat::Png).unwrap();
        // 背景纯白：四角必须是白的（旧路径贴着印刷体裁会带进灰底/文字像素，这里保证不会）。
        for (x, y) in [(0, 0), (img.width() - 1, 0), (0, img.height() - 1)] {
            assert_eq!(img.get_pixel(x, y), image::Rgba([255, 255, 255, 255]), "背景该是纯白");
        }
        // 画面里该有黑色笔画像素（不是一整张空白纸）。
        let has_black = (0..img.width()).flat_map(|x| (0..img.height()).map(move |y| (x, y))).any(|(x, y)| img.get_pixel(x, y) == image::Rgba([0, 0, 0, 255]));
        assert!(has_black, "笔画区域该有黑像素，不该是空白");
    }

    #[test]
    fn only_picks_requested_stroke_ids_not_neighboring_ones() {
        let a = stroke(1, &[(0.0, 0.0), (10.0, 10.0)]);
        let b = stroke(2, &[(1000.0, 1000.0), (1010.0, 1010.0)]); // 不该被选中的邻簇
        let png = render_ink(&[a, b], &["1:1".to_string()], (0.0, 0.0, 10.0, 10.0), 4.0).unwrap();
        assert!(image::load_from_memory_with_format(&png, ImageFormat::Png).is_ok());
    }

    #[test]
    fn empty_id_match_or_degenerate_bbox_errors_cleanly() {
        let s = stroke(1, &[(0.0, 0.0), (10.0, 10.0)]);
        assert!(render_ink(&[s.clone()], &["9:9".to_string()], (0.0, 0.0, 10.0, 10.0), 4.0).unwrap_err().contains("一笔都没找到"));
        // 单点笔画（bbox 退化）：留白够大时应该正常出图（画一个点），不是错误。
        let dot = stroke(1, &[(0.0, 0.0)]);
        let png = render_ink(&[dot], &["1:1".to_string()], (0.0, 0.0, 0.0, 0.0), 6.0).unwrap();
        assert!(image::load_from_memory_with_format(&png, ImageFormat::Png).is_ok());
    }
}
