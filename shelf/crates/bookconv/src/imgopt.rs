//! 优化器图片降采样——按 reMarkable Paper Pro Move 实际屏幕规格做设备级优化。
//!
//! Move 屏 = 1696×954 px（7.3″、264 PPI、Gallery 3 彩色墨水屏）。书里常带 2000–4000px 的高清原图，
//! 超出屏幕的像素**纯属浪费**：拖慢加载、吃内存、还逼 xochitl 在渲染期临时缩放（慢且质量不可控）。
//! 优化器在组包/优化阶段把超大图 Lanczos3 预缩到长边 ≤1696，缩放质量我们控（优于运行时缩放）。
//!
//! 纪律：**只缩不放、保宽高比、保原格式、达标即跳过（幂等 + 免二次编码损失）、任何失败原样保留**
//! （绝不因优化损坏原书）。只碰 JPEG/PNG（书内图几乎都是；GIF 可能动图，跳过不冒险）。

use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::ImageFormat;
use std::io::Cursor;

/// Move 屏最长像素边 / 最短像素边。
pub const MAX_EDGE: u32 = 1696;
pub const MAX_SHORT_EDGE: u32 = 954;
/// 重编码 JPEG 质量（0–100）。85 = 视觉无损级，体积/画质平衡；e-ink 上更看不出差异。
const JPEG_QUALITY: u8 = 85;

/// 保比缩进 `max_w × max_h` 框（宽高比保持、保原格式），只在超框时动；返回新字节或 `None`
/// （已达标 / 非 JPEG·PNG / 解码失败 / 重编码没变小 → 调用方原样保留）。
fn downscale_into(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Vec<u8>> {
    let (fmt, (w, h)) = header_dims(bytes)?;
    if w <= max_w && h <= max_h {
        return None; // 已达标：不解码不重编码（避免无谓的二次有损压缩；2473 页漫画只读头是秒级、全解是分钟级）
    }
    let img = image::load_from_memory_with_format(bytes, fmt).ok()?;
    let resized = img.resize(max_w, max_h, FilterType::Lanczos3);
    let mut out = Vec::new();
    match fmt {
        ImageFormat::Jpeg => {
            let mut enc = JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY);
            enc.encode_image(&resized).ok()?;
        }
        ImageFormat::Png => {
            resized.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).ok()?;
        }
        _ => return None,
    }
    // 只有确实变小才采用（极端下重编码可能变大 → 保留原图，不倒退体积）。
    (out.len() < bytes.len()).then_some(out)
}

/// **CBZ/漫画整页**降采样：按朝向选盒（竖 954×1696 / 横 1696×954），页整张填屏、横页横读可用 1696 宽。
/// 真机探针（2026-09-02，5 张 400–2400px 宽图上机看渲染的 `<uuid>.pdf`）：只卡长边会让方图多留 1.8× 无用像素。
pub fn downscale_for_device(bytes: &[u8]) -> Option<Vec<u8>> {
    let (_, (w, h)) = header_dims(bytes)?;
    let (max_w, max_h) = if w >= h { (MAX_EDGE, MAX_SHORT_EDGE) } else { (MAX_SHORT_EDGE, MAX_EDGE) };
    downscale_into(bytes, max_w, max_h)
}

/// 只读文件头取 (格式, 宽, 高)，不解码像素。非 JPEG/PNG → None。
fn header_dims(bytes: &[u8]) -> Option<(ImageFormat, (u32, u32))> {
    let fmt = image::guess_format(bytes).ok()?;
    if !matches!(fmt, ImageFormat::Jpeg | ImageFormat::Png) {
        return None;
    }
    let dims = image::ImageReader::with_format(Cursor::new(bytes), fmt).into_dimensions().ok()?;
    Some((fmt, dims))
}

/// **EPUB 内嵌图**降采样：一律竖向框 954×1696（**宽绝不超 954**）。EPUB 图可能**行内**（xochitl 按固有
/// 尺寸渲染、不认 CSS），横图容许 1696 宽会让行内横幅溢出竖屏——2026-09-04 真机《飘》1696×630 的
/// `class="logo"` 内联横幅溢出坐实。竖向框下：块级图仍适配列宽（显示无变化）、行内图不再超宽。
pub fn downscale_for_epub(bytes: &[u8]) -> Option<Vec<u8>> {
    downscale_into(bytes, MAX_SHORT_EDGE, MAX_EDGE)
}

/// 「漫画省刷新」色彩保留阈值：页面平均色度（RGB 通道极差 /255 的均值）低于此值视作**黑白/偏色扫描**、
/// 转 1-bit；高于此值视作**真彩页**（漫画彩封/彩插）→ 保留彩色不动。真机实测火影正文=0（灰度 JPEG）、
/// 彩封≈0.5(HSL 饱和度)，0.06 能干净分开：清洗偏色扫描、保住真彩。
const COLOR_KEEP_CHROMA: f32 = 0.06;

/// 采样估计页面平均色度（避免逐像素遍历大图）：每隔若干像素取样，取 RGB 极差均值 /255。
/// 灰度图（r=g=b）色度恒 0。
fn mean_chroma(img: &image::DynamicImage) -> f32 {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    let total = (w as u64) * (h as u64);
    if total == 0 {
        return 0.0;
    }
    let step = ((total / 40_000).max(1)) as usize; // 约取 ~4 万样本封顶
    let (mut sum, mut n) = (0f32, 0u32);
    for px in rgb.pixels().step_by(step) {
        let (r, g, b) = (px[0], px[1], px[2]);
        let spread = r.max(g).max(b) - r.min(g).min(b);
        sum += spread as f32;
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        (sum / n as f32) / 255.0
    }
}

/// 「漫画省刷新」核心：解码一页图 → 若为真彩页返回 `None`（调用方保留彩色）；否则转灰度 + Floyd–Steinberg
/// 抖动成双色（0/255）返回 `GrayImage`。抖动保住网点/灰面观感，双色触发面板更轻的 mono 波形（真机坐实：
/// 1-bit 翻页显著更快更轻），且比 8-bit 灰度 FlateDecode 体积小得多。只碰 JPEG/PNG，其余/解码失败=`None`。
pub fn dither_bilevel(bytes: &[u8]) -> Option<image::GrayImage> {
    let fmt = image::guess_format(bytes).ok()?;
    if !matches!(fmt, ImageFormat::Jpeg | ImageFormat::Png) {
        return None;
    }
    let img = image::load_from_memory_with_format(bytes, fmt).ok()?;
    if mean_chroma(&img) >= COLOR_KEEP_CHROMA {
        return None; // 真彩页：保留彩色（Move 是彩屏，别无脑丢色）
    }
    let mut luma = img.to_luma8();
    image::imageops::colorops::dither(&mut luma, &image::imageops::colorops::BiLevel);
    Some(luma)
}

/// 条目是否是可降采样图片（按扩展名快筛，真正的格式判定在 `downscale_for_device` 里用魔数）。
pub fn is_downscalable(name: &str) -> bool {
    let l = name.to_lowercase();
    l.ends_with(".jpg") || l.ends_with(".jpeg") || l.ends_with(".png")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, GenericImageView, RgbImage};

    fn jpeg_of(w: u32, h: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
        }));
        let mut buf = Vec::new();
        JpegEncoder::new_with_quality(&mut buf, 90).encode_image(&img).unwrap();
        buf
    }

    #[test]
    fn device_orientation_box_for_comics() {
        // CBZ/漫画整页：按朝向选盒。横图 3392×1908 → 1696×954（横读可用满宽）
        let big = jpeg_of(3392, 1908);
        let (w, h) = image::load_from_memory(&downscale_for_device(&big).unwrap()).unwrap().dimensions();
        assert_eq!((w, h), (MAX_EDGE, 954), "横页应到 1696×954");
        // 方图 → 954×954
        let sq = jpeg_of(2000, 2000);
        let (w, h) = image::load_from_memory(&downscale_for_device(&sq).unwrap()).unwrap().dimensions();
        assert_eq!((w, h), (954, 954));
    }

    #[test]
    fn epub_portrait_box_caps_width_954() {
        // EPUB 内嵌图一律卡宽 ≤954（防行内横幅溢出竖屏）
        // 横图 1696×630 的内联横幅（《飘》真机溢出源）→ 954×~355
        let banner = jpeg_of(1696, 630);
        let (w, h) = image::load_from_memory(&downscale_for_epub(&banner).unwrap()).unwrap().dimensions();
        assert_eq!(w, 954, "横幅宽必须卡到 954");
        assert!(h < 400, "保比 h={h}");
        // 方图 → 954×954；竖图 1000×3000 → 565×1696
        let sq = jpeg_of(2000, 2000);
        assert_eq!(image::load_from_memory(&downscale_for_epub(&sq).unwrap()).unwrap().dimensions(), (954, 954));
        let tall = jpeg_of(1000, 3000);
        let (w, h) = image::load_from_memory(&downscale_for_epub(&tall).unwrap()).unwrap().dimensions();
        assert!(w <= 954 && h == 1696, "竖图 {w}x{h}");
    }

    #[test]
    fn skips_already_small_image() {
        let small = jpeg_of(800, 600);
        assert!(downscale_for_device(&small).is_none(), "已达标图不动（幂等、免二次损失）");
    }

    #[test]
    fn ignores_non_image_bytes() {
        assert!(downscale_for_device(b"not an image at all").is_none());
    }

    #[test]
    fn is_downscalable_by_ext() {
        assert!(is_downscalable("OEBPS/images/p1.JPG"));
        assert!(is_downscalable("a/b.png"));
        assert!(!is_downscalable("style.css"));
        assert!(!is_downscalable("cover.gif"));
    }
}
