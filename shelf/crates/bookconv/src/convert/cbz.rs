//! CBZ（漫画 zip 归档）→ PDF：解包 → 图片按自然序排 → 手搓 PDF（见 pdfwrite）。
//! 只负责「解包 + 排序」，图片编码与 PDF 组装全交给 pdfwrite，互不耦合。

use super::pdfwrite::{bilevel_image, image_from_bytes, images_to_pdf};
use super::EinkTone;
use std::io::Read;
use zip::ZipArchive;

/// CBZ 字节 → PDF 字节。内含 jpg/jpeg/png 图片，按文件名自然序（page_2 < page_10）成页。
/// `tone`=墨水屏色调档：`Off` 原样直嵌；`Mono` 黑白页转 1-bit 抖动（省刷新），真彩页仍保留彩色。
pub fn cbz_to_pdf(data: &[u8], tone: EinkTone) -> Result<Vec<u8>, String> {
    let mut zip = ZipArchive::new(std::io::Cursor::new(data)).map_err(|e| format!("CBZ 打开: {e}"))?;
    let mut names: Vec<String> = Vec::new();
    for i in 0..zip.len() {
        let f = zip.by_index(i).map_err(|e| e.to_string())?;
        if f.is_dir() {
            continue;
        }
        if is_image_name(f.name()) {
            names.push(f.name().to_string());
        }
    }
    if names.is_empty() {
        return Err("CBZ 内无图片（jpg/jpeg/png）".into());
    }
    names.sort_by(|a, b| natural_cmp(a, b));
    let mut images = Vec::with_capacity(names.len());
    for name in &names {
        let mut f = zip.by_name(name).map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        f.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        // 设备优化（Tier 1）：漫画页常远超 Move 屏（1696×954），组 PDF 前按屏降采样——长边 >1696
        // 才 Lanczos3 缩、达标即跳过（仍原样直嵌、零重编码损失）。超屏像素对这块屏纯浪费（更慢/更占）。
        if let Some(smaller) = crate::imgopt::downscale_for_device(&bytes) {
            bytes = smaller;
        }
        // 省刷新（Mono）：黑白/偏色页转 1-bit 抖动（触发更轻波形+缩体积）；真彩页 dither_bilevel 返回
        // None → 落回原样直嵌路径保留彩色。Off 档直接走原样路径。
        let img = match tone {
            EinkTone::Mono => match crate::imgopt::dither_bilevel(&bytes) {
                Some(gray) => bilevel_image(&gray),
                None => image_from_bytes(&bytes).map_err(|e| format!("{name}: {e}"))?,
            },
            EinkTone::Off => image_from_bytes(&bytes).map_err(|e| format!("{name}: {e}"))?,
        };
        images.push(img);
    }
    images_to_pdf(&images)
}

fn is_image_name(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".jpg") || l.ends_with(".jpeg") || l.ends_with(".png")
}

/// 自然排序：连续数字段按数值比较（去前导零后先比位数再逐位），其余按字节。
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        if a[i].is_ascii_digit() && b[j].is_ascii_digit() {
            let (mut ni, mut nj) = (i, j);
            while ni < a.len() && a[ni].is_ascii_digit() {
                ni += 1;
            }
            while nj < b.len() && b[nj].is_ascii_digit() {
                nj += 1;
            }
            let (ta, tb) = (trim_leading_zeros(&a[i..ni]), trim_leading_zeros(&b[j..nj]));
            match ta.len().cmp(&tb.len()).then_with(|| ta.cmp(tb)) {
                Ordering::Equal => {}
                o => return o,
            }
            i = ni;
            j = nj;
        } else {
            match a[i].cmp(&b[j]) {
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                o => return o,
            }
        }
    }
    a.len().cmp(&b.len())
}

fn trim_leading_zeros(s: &[u8]) -> &[u8] {
    let mut k = 0;
    while k + 1 < s.len() && s[k] == b'0' {
        k += 1;
    }
    &s[k..]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cmp::Ordering;

    #[test]
    fn natural_sort_orders_numeric_chunks() {
        let mut v = vec![
            "page_10.jpg".to_string(),
            "page_2.jpg".into(),
            "page_1.jpg".into(),
            "page_100.jpg".into(),
        ];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["page_1.jpg", "page_2.jpg", "page_10.jpg", "page_100.jpg"]);
    }

    #[test]
    fn natural_sort_handles_leading_zeros() {
        // 数值相等（去前导零后同）时，原串更长者排后（p007 长于 p7）
        assert_eq!(natural_cmp("p007", "p7"), Ordering::Greater);
        let mut v = vec!["p010".to_string(), "p09".into(), "p1".into()];
        v.sort_by(|a, b| natural_cmp(a, b));
        assert_eq!(v, ["p1", "p09", "p010"]);
    }

    #[test]
    fn oversized_page_downscaled_in_pdf() {
        use image::{codecs::jpeg::JpegEncoder, DynamicImage, RgbImage};
        // 3392×1908（2× 屏）的漫画页
        let big = DynamicImage::ImageRgb8(RgbImage::from_fn(3392, 1908, |x, _| image::Rgb([(x % 256) as u8, 90, 160])));
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&big).unwrap();
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            z.start_file("page_1.jpg", zip::write::SimpleFileOptions::default()).unwrap();
            use std::io::Write;
            z.write_all(&jpg).unwrap();
            z.finish().unwrap();
        }
        let pdf = cbz_to_pdf(&buf, EinkTone::Off).unwrap();
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/Width 1696"), "超大页应降采样到长边 1696，PDF 里应是 /Width 1696");
        assert!(!s.contains("/Width 3392"), "不该保留原 3392 宽");
    }

    #[test]
    fn mono_tone_makes_grayscale_page_1bit() {
        use image::{codecs::png::PngEncoder, ExtendedColorType, ImageEncoder};
        // 一张灰度页（渐变，无彩色）→ Mono 档应转 1-bit
        let w = 200u32;
        let h = 120u32;
        let gray: Vec<u8> = (0..w * h).map(|i| ((i % w) * 255 / w) as u8).collect();
        let mut png = Vec::new();
        PngEncoder::new(&mut png).write_image(&gray, w, h, ExtendedColorType::L8).unwrap();
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            z.start_file("p001.png", zip::write::SimpleFileOptions::default()).unwrap();
            use std::io::Write;
            z.write_all(&png).unwrap();
            z.finish().unwrap();
        }
        let pdf = cbz_to_pdf(&buf, EinkTone::Mono).unwrap();
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/BitsPerComponent 1"), "黑白页 Mono 档应为 1-bit");
        assert!(s.contains("/ColorSpace /DeviceGray"), "应为灰度色彩空间");
    }

    #[test]
    fn mono_tone_keeps_truecolor_page() {
        use image::{codecs::jpeg::JpegEncoder, DynamicImage, RgbImage};
        // 高饱和彩页（红蓝相间）→ Mono 档应保留彩色（不转 1-bit）
        let color = DynamicImage::ImageRgb8(RgbImage::from_fn(160, 100, |x, _| {
            if x % 2 == 0 { image::Rgb([220, 20, 20]) } else { image::Rgb([20, 20, 220]) }
        }));
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&color).unwrap();
        let mut buf = Vec::new();
        {
            let mut z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            z.start_file("cover.jpg", zip::write::SimpleFileOptions::default()).unwrap();
            use std::io::Write;
            z.write_all(&jpg).unwrap();
            z.finish().unwrap();
        }
        let pdf = cbz_to_pdf(&buf, EinkTone::Mono).unwrap();
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/BitsPerComponent 8"), "真彩页应保留 8-bit 彩色");
        assert!(s.contains("/DeviceRGB"), "真彩页应保 RGB");
    }

    #[test]
    fn cbz_with_no_images_errors() {
        // 空 zip
        let mut buf = Vec::new();
        {
            let z = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            z.finish().unwrap();
        }
        assert!(cbz_to_pdf(&buf, EinkTone::Off).is_err());
    }
}
