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
/// 单张图片允许解码的像素数上限（w×h，跟格式/用途无关）——防极端高分辨率原图解码成未压缩位图
/// 把内存顶爆。2026-09-19 真机坐实：用户真实投递一套漫画（《乱马1/2》8 卷）触发超限按卷拆分
/// 落库，book-serve `VmHWM` 冲到 271MB——定位到 `downscale_into_q`/`trim_margins`/`dither_bilevel`
/// 三处解码前只用 `header_dims` 读了宽高判断"要不要处理"，没有对"这张图本身大到不该整个解出来"
/// 设硬上限。
///
/// **阈值取值不是"3 字节/像素 RGB8"这种理论估算**——第一版按这个估算给了 2500 万像素（估算峰值
/// ~75MB），结果真机又撞上一次《火影忍者》多卷投递，`VmHWM` 又冲到 262MB，跟没修之前几乎一个
/// 量级。本地测量真实调用链（`trim_margins(bytes)` → `downscale_for_epub_comic(&trimmed)`，忠实
/// 复刻 `optimize.rs` 的真实用法）在不同像素数下的实测 `VmHWM`（`/proc/<pid>/status`，release
/// 编译）：400万像素→62MB、870万像素（A4 300dpi）→97-109MB、1600万像素→164MB、2500万像素→
/// 230-236MB——**理论估算的单缓冲区大小完全没抓住真实开销**（`image` 库内部解码+`to_rgb8()`+
/// resize 中间缓冲多份同时存活，实测开销约 9-16MB/百万像素，远高于 3 字节/像素≈3MB/百万像素的
/// naive 估算）。改用实测数据定阈值：900 万像素（约 3000×3000，覆盖 A4 300dpi 及绝大多数真实
/// 漫画/书籍扫描页）在真机上峰值约 100-110MB——比 262MB 危机低一个数量级，设备实测可用内存
/// 通常有几百 MB 余量，这个量级的单张图瞬时峰值不构成风险。超限的图直接放弃处理、原样保留原图
/// 字节——调用方对这三个函数返回 `None` 本来就是"原样保留"语义，天然兜底，不是新错误路径。
const MAX_DECODE_PIXELS: u64 = 9_000_000;

fn within_decode_budget(w: u32, h: u32) -> bool {
    (w as u64) * (h as u64) <= MAX_DECODE_PIXELS
}
/// 重编码 JPEG 质量（0–100）。85 = 视觉无损级，体积/画质平衡；e-ink 上更看不出差异。
const JPEG_QUALITY: u8 = 85;
/// 漫画页专用重编码质量——EPUB 线原则④"漫画不允许压画质"：超限时仍必须缩到屏幕框内（否则设备渲染
/// 异常），但不该像普通插图那样再吃一道 85 质量的有损重编码，95 更接近视觉无损。
const JPEG_QUALITY_COMIC: u8 = 95;

/// 保比缩进 `max_w × max_h` 框（宽高比保持、保原格式），只在超框时动；返回新字节或 `None`
/// （已达标 / 非 JPEG·PNG / 解码失败 / 重编码没变小 → 调用方原样保留）。
fn downscale_into_q(bytes: &[u8], max_w: u32, max_h: u32, quality: u8) -> Option<Vec<u8>> {
    let (fmt, (w, h)) = header_dims(bytes)?;
    if w <= max_w && h <= max_h {
        return None; // 已达标：不解码不重编码（避免无谓的二次有损压缩；2473 页漫画只读头是秒级、全解是分钟级）
    }
    if !within_decode_budget(w, h) {
        return None; // 极端高分辨率原图：不整个解出来，原样保留（见 MAX_DECODE_PIXELS 文档）
    }
    let img = image::load_from_memory_with_format(bytes, fmt).ok()?;
    let resized = img.resize(max_w, max_h, FilterType::Lanczos3);
    let mut out = Vec::new();
    match fmt {
        ImageFormat::Jpeg => {
            let mut enc = JpegEncoder::new_with_quality(&mut out, quality);
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

fn downscale_into(bytes: &[u8], max_w: u32, max_h: u32) -> Option<Vec<u8>> {
    downscale_into_q(bytes, max_w, max_h, JPEG_QUALITY)
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

/// 同 [`downscale_for_epub`]，给已判定为漫画的 EPUB 用：超限时仍要缩进屏幕框（否则设备渲染异常），
/// 但用 [`JPEG_QUALITY_COMIC`] 而非普通插图的 85，尽量不损画质（EPUB 线原则④）。
pub fn downscale_for_epub_comic(bytes: &[u8]) -> Option<Vec<u8>> {
    downscale_into_q(bytes, MAX_SHORT_EDGE, MAX_EDGE, JPEG_QUALITY_COMIC)
}

/// 裁边判定容差：一行/列里像素两两 RGB 通道极差都 ≤ 这个值才算"纯色留白"。留够松（8）容 JPEG 压缩
/// 噪声，但不到能吃掉真实画面渐变的地步。
const TRIM_TOLERANCE: u8 = 8;
/// 单边最多裁掉原图这个比例——防止极端图（比如整页近乎纯色）被误判成"全是留白"裁没内容。
const TRIM_MAX_FRACTION: f32 = 0.15;

fn row_is_uniform(img: &image::RgbImage, y: u32) -> bool {
    let w = img.width();
    if w <= 1 {
        return true;
    }
    let first = *img.get_pixel(0, y);
    (1..w).all(|x| {
        let p = img.get_pixel(x, y);
        (0..3).all(|c| (p[c] as i16 - first[c] as i16).unsigned_abs() as u8 <= TRIM_TOLERANCE)
    })
}

fn col_is_uniform(img: &image::RgbImage, x: u32) -> bool {
    let h = img.height();
    if h <= 1 {
        return true;
    }
    let first = *img.get_pixel(x, 0);
    (1..h).all(|y| {
        let p = img.get_pixel(x, y);
        (0..3).all(|c| (p[c] as i16 - first[c] as i16).unsigned_abs() as u8 <= TRIM_TOLERANCE)
    })
}

/// 漫画页四边纯色/近纯色留白裁边（EPUB 线原则④"允许裁边、不允许压画质"）。只在"确实是留白"时裁——
/// 边缘整行/整列像素高度一致（[`TRIM_TOLERANCE`]）才算留白，一遇到不满足就停，不会裁进真实画面。
/// 单边最多裁 [`TRIM_MAX_FRACTION`]，兜底极端误判。没有可裁的留白 / 非 JPEG·PNG / 解码失败 → `None`
/// （调用方原样保留）。重编码用漫画质量（[`JPEG_QUALITY_COMIC`]），裁边不等于允许压画质。
pub fn trim_margins(bytes: &[u8]) -> Option<Vec<u8>> {
    let (fmt, (w, h)) = header_dims(bytes)?;
    if !within_decode_budget(w, h) {
        return None; // 极端高分辨率原图：不整个解出来，原样保留（见 MAX_DECODE_PIXELS 文档）
    }
    let img = image::load_from_memory_with_format(bytes, fmt).ok()?.to_rgb8();
    let (w, h) = img.dimensions();
    if w < 4 || h < 4 {
        return None;
    }
    let max_v = ((h as f32) * TRIM_MAX_FRACTION) as u32;
    let max_h = ((w as f32) * TRIM_MAX_FRACTION) as u32;
    let mut top = 0u32;
    while top < max_v && top + 1 < h && row_is_uniform(&img, top) {
        top += 1;
    }
    let mut bottom = 0u32;
    while bottom < max_v && bottom + 1 < h && row_is_uniform(&img, h - 1 - bottom) {
        bottom += 1;
    }
    let mut left = 0u32;
    while left < max_h && left + 1 < w && col_is_uniform(&img, left) {
        left += 1;
    }
    let mut right = 0u32;
    while right < max_h && right + 1 < w && col_is_uniform(&img, w - 1 - right) {
        right += 1;
    }
    if top == 0 && bottom == 0 && left == 0 && right == 0 {
        return None; // 没有可裁的留白
    }
    let (new_w, new_h) = (w - left - right, h - top - bottom);
    if new_w == 0 || new_h == 0 {
        return None;
    }
    let cropped = image::imageops::crop_imm(&img, left, top, new_w, new_h).to_image();
    let dyn_img = image::DynamicImage::ImageRgb8(cropped);
    let mut out = Vec::new();
    match fmt {
        ImageFormat::Jpeg => JpegEncoder::new_with_quality(&mut out, JPEG_QUALITY_COMIC).encode_image(&dyn_img).ok()?,
        ImageFormat::Png => dyn_img.write_to(&mut Cursor::new(&mut out), ImageFormat::Png).ok()?,
        _ => return None,
    }
    Some(out)
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
    let (fmt, (w, h)) = header_dims(bytes)?;
    if !within_decode_budget(w, h) {
        return None; // 极端高分辨率原图：不整个解出来，原样保留（见 MAX_DECODE_PIXELS 文档）
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

    /// 造一张带纯白边框的图：中心是彩色渐变，四边留白。
    fn framed_jpeg(w: u32, h: u32, margin: u32) -> Vec<u8> {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| {
            if x < margin || y < margin || x >= w - margin || y >= h - margin {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
            }
        }));
        let mut buf = Vec::new();
        // 高质量无损级编码，避免 JPEG 压缩噪声把"纯色"判花（真实场景裁边容差本身留够松，这里只是
        // 让测试信号干净，不代表生产输入总是这么干净）。
        JpegEncoder::new_with_quality(&mut buf, 100).encode_image(&img).unwrap();
        buf
    }

    #[test]
    fn trim_margins_crops_uniform_white_border_only() {
        let framed = framed_jpeg(200, 300, 10);
        let out = trim_margins(&framed).expect("四边留白应触发裁边");
        let (w, h) = image::load_from_memory(&out).unwrap().dimensions();
        assert_eq!((w, h), (180, 280), "应精确裁掉 10px 留白: got {w}x{h}");
    }

    #[test]
    fn trim_margins_none_when_no_uniform_border() {
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(200, 300, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])));
        let mut buf = Vec::new();
        JpegEncoder::new_with_quality(&mut buf, 95).encode_image(&img).unwrap();
        assert!(trim_margins(&buf).is_none(), "画面一直到边缘、没有留白，不该裁");
    }

    #[test]
    fn trim_margins_capped_by_max_fraction_for_near_solid_image() {
        // 几乎整张纯色(只有中心一小块不同)——裁边不能把整张图裁没，单边应被 TRIM_MAX_FRACTION 卡住。
        let img = DynamicImage::ImageRgb8(RgbImage::from_fn(200, 200, |x, y| {
            if (90..110).contains(&x) && (90..110).contains(&y) { image::Rgb([0, 0, 0]) } else { image::Rgb([255, 255, 255]) }
        }));
        let mut buf = Vec::new();
        JpegEncoder::new_with_quality(&mut buf, 100).encode_image(&img).unwrap();
        let out = trim_margins(&buf).expect("大片留白应触发裁边");
        let (w, h) = image::load_from_memory(&out).unwrap().dimensions();
        assert!(w >= 200 - 2 * 30 && h >= 200 - 2 * 30, "单边最多裁 15%，不能把画面裁没: got {w}x{h}");
    }

    #[test]
    fn comic_variant_same_box_higher_quality_than_regular_epub_image() {
        // 同样超框需要缩放，漫画路径(quality 95)重编码应该比普通插图路径(quality 85)体积更大
        // （信息保留更多，符合 EPUB 线原则④"漫画不允许压画质"）；缩放后的尺寸应该一致，只是质量不同。
        let big = jpeg_of(2000, 3000);
        let regular = downscale_for_epub(&big).expect("超框应触发重编码");
        let comic = downscale_for_epub_comic(&big).expect("超框应触发重编码");
        assert_eq!(
            image::load_from_memory(&regular).unwrap().dimensions(),
            image::load_from_memory(&comic).unwrap().dimensions(),
            "尺寸约束一致，只是质量不同"
        );
        assert!(comic.len() >= regular.len(), "漫画路径应该保留更多信息，体积不小于普通插图路径: comic={} regular={}", comic.len(), regular.len());
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
    fn within_decode_budget_boundary() {
        assert!(within_decode_budget(3000, 3000), "900 万像素，等于上限，应允许");
        assert!(!within_decode_budget(3001, 3000), "超一点点也该拒绝");
    }

    /// 2026-09-19 真机事故回归测试：用户真实投递一套漫画，某张扫描页解码成未压缩位图把
    /// book-serve `VmHWM` 顶到 271MB（第一版阈值按理论估算定的 2500 万像素，真机又撞了一次
    /// 262MB，说明理论估算不可靠，改用实测数据重新定阈值，见 `MAX_DECODE_PIXELS` 文档）。三个
    /// 解码入口都该对超限图直接放弃处理、原样保留，不再整张解出来。5001×5000（远超新阈值
    /// 900 万像素）足够验证真实调用链路，不需要造更大的图。
    #[test]
    fn oversized_image_skipped_by_all_decode_entries() {
        let huge = jpeg_of(5001, 5000);
        assert!(downscale_for_device(&huge).is_none(), "超限图应跳过降采样");
        assert!(downscale_for_epub(&huge).is_none(), "超限图应跳过降采样");
        assert!(trim_margins(&huge).is_none(), "超限图应跳过裁边");
        assert!(dither_bilevel(&huge).is_none(), "超限图应跳过省刷新转换");
    }

    #[test]
    fn is_downscalable_by_ext() {
        assert!(is_downscalable("OEBPS/images/p1.JPG"));
        assert!(is_downscalable("a/b.png"));
        assert!(!is_downscalable("style.css"));
        assert!(!is_downscalable("cover.gif"));
    }
}
