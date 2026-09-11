//! 最小 PDF 写入器：一串图片（每图一页）→ PDF。贴合 epub.rs「手搓、零 C 依赖」风格。
//! JPEG 直接作 /DCTDecode 嵌入（不解码、不重编码——漫画页几乎都是 JPEG）；
//! PNG 用现成 `png` crate 解码成原始像素、miniz_oxide zlib 压成 /FlateDecode。
//! 给漫画（CBZ）用——xochitl 原生 PDF 翻页比 EPUB 顺、一页一图最合漫画。

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ColorSpace {
    Gray,
    Rgb,
}
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Filter {
    Dct,
    Flate,
}
impl ColorSpace {
    fn pdf_name(self) -> &'static str {
        match self {
            ColorSpace::Gray => "/DeviceGray",
            ColorSpace::Rgb => "/DeviceRGB",
        }
    }
}
impl Filter {
    fn pdf_name(self) -> &'static str {
        match self {
            Filter::Dct => "/DCTDecode",
            Filter::Flate => "/FlateDecode",
        }
    }
}

/// 一页图片：宽高 + 色彩空间 + 位深 + PDF 过滤器 + 已就绪的流数据（JPEG 原字节 / zlib 压缩像素）。
/// `bits`=每分量位深：常规图 8；「漫画省刷新」1-bit 黑白页 = 1（DeviceGray，1 位/像素，行按字节对齐）。
pub struct PdfImage {
    pub width: u32,
    pub height: u32,
    pub color: ColorSpace,
    pub bits: u8,
    pub filter: Filter,
    pub data: Vec<u8>,
}

/// 「漫画省刷新」页：已抖动成双色（0/255）的灰度图 → 1-bit /DeviceGray PDF 图。
/// 行内像素 MSB 优先打包（bit=1 表白、0 表黑），**每行按字节对齐**（PDF 图像扫描行要求），
/// 再 miniz_oxide zlib 压成 /FlateDecode。相比 8-bit 灰度直存，体积 ~1/8 且触发面板更轻的 mono 波形。
pub fn bilevel_image(gray: &image::GrayImage) -> PdfImage {
    let (w, h) = (gray.width(), gray.height());
    let row_bytes = (w as usize).div_ceil(8); // 每行字节数（向上取整到字节）
    let mut packed = vec![0u8; row_bytes * h as usize];
    for y in 0..h {
        let row_off = y as usize * row_bytes;
        for x in 0..w {
            // 抖动产物只有 0/255；≥128 视作白（bit=1）。
            if gray.get_pixel(x, y)[0] >= 128 {
                packed[row_off + (x as usize >> 3)] |= 0x80 >> (x & 7);
            }
        }
    }
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&packed, 6);
    PdfImage { width: w, height: h, color: ColorSpace::Gray, bits: 1, filter: Filter::Flate, data: compressed }
}

/// 按魔数识别 JPEG/PNG，产出可嵌入 PDF 的 PdfImage。
pub fn image_from_bytes(data: &[u8]) -> Result<PdfImage, String> {
    if data.len() >= 2 && data[0] == 0xFF && data[1] == 0xD8 {
        jpeg_to_image(data)
    } else if data.len() >= 8 && &data[..8] == b"\x89PNG\r\n\x1a\n" {
        png_to_image(data)
    } else {
        Err("非 JPEG/PNG 图片".into())
    }
}

/// 解析 JPEG 的 SOF 段取宽高与分量数；像素原样 DCTDecode 嵌入，不解码。
fn jpeg_to_image(data: &[u8]) -> Result<PdfImage, String> {
    if data.len() < 2 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err("非 JPEG（缺 SOI）".into());
    }
    let mut i = 2usize;
    while i + 1 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        // 跳过连续填充 0xFF，定位到 marker 字节
        let mut j = i + 1;
        while j < data.len() && data[j] == 0xFF {
            j += 1;
        }
        if j >= data.len() {
            break;
        }
        let marker = data[j];
        i = j; // data[i] = marker
        // 无长度段：SOI/EOI/RSTn/TEM
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            i += 1;
            continue;
        }
        if i + 3 > data.len() {
            break;
        }
        let seg_len = ((data[i + 1] as usize) << 8) | data[i + 2] as usize;
        // SOF 标记 C0..CF，排除 C4(DHT)/C8(JPG)/CC(DAC)
        let is_sof =
            (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC;
        if is_sof {
            // 段内容：precision(1) height(2) width(2) components(1)
            if i + 8 > data.len() {
                return Err("JPEG SOF 段截断".into());
            }
            let h = ((data[i + 4] as u32) << 8) | data[i + 5] as u32;
            let w = ((data[i + 6] as u32) << 8) | data[i + 7] as u32;
            let comps = data[i + 8];
            let color = match comps {
                1 => ColorSpace::Gray,
                3 => ColorSpace::Rgb,
                _ => return Err(format!("JPEG 不支持的分量数 {comps}（CMYK 等）")),
            };
            if w == 0 || h == 0 {
                return Err("JPEG 宽高为 0".into());
            }
            return Ok(PdfImage {
                width: w,
                height: h,
                color,
                bits: 8,
                filter: Filter::Dct,
                data: data.to_vec(),
            });
        }
        i += 1 + seg_len; // marker 字节 + 段（长度含 2 个长度字节自身）
    }
    Err("JPEG 未找到 SOF 段".into())
}

/// PNG 用 png crate 解码归一到 8-bit 灰度/RGB（EXPAND 展开调色板/低位深、STRIP_16 降位深；
/// 带 alpha 合成到白底），再 miniz_oxide zlib 压成 /FlateDecode 流。
fn png_to_image(data: &[u8]) -> Result<PdfImage, String> {
    let mut decoder = png::Decoder::new(data);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("PNG 头解码: {e}"))?;
    let mut buf = vec![0u8; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).map_err(|e| format!("PNG 帧解码: {e}"))?;
    let (w, h) = (info.width, info.height);
    let bytes = &buf[..info.buffer_size()];
    let (color, pixels): (ColorSpace, Vec<u8>) = match info.color_type {
        png::ColorType::Grayscale => (ColorSpace::Gray, bytes.to_vec()),
        png::ColorType::Rgb => (ColorSpace::Rgb, bytes.to_vec()),
        png::ColorType::GrayscaleAlpha => {
            let mut out = Vec::with_capacity((w * h) as usize);
            for px in bytes.chunks_exact(2) {
                let (g, a) = (px[0] as u32, px[1] as u32);
                out.push(((g * a + 255 * (255 - a)) / 255) as u8);
            }
            (ColorSpace::Gray, out)
        }
        png::ColorType::Rgba => {
            let mut out = Vec::with_capacity((w * h * 3) as usize);
            for px in bytes.chunks_exact(4) {
                let a = px[3] as u32;
                for c in 0..3 {
                    out.push(((px[c] as u32 * a + 255 * (255 - a)) / 255) as u8);
                }
            }
            (ColorSpace::Rgb, out)
        }
        png::ColorType::Indexed => {
            // EXPAND 应已展开调色板；仍到此说明非常规，明确报错胜过产坏图
            return Err("PNG 调色板未展开（异常）".into());
        }
    };
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&pixels, 6);
    Ok(PdfImage {
        width: w,
        height: h,
        color,
        bits: 8,
        filter: Filter::Flate,
        data: compressed,
    })
}

/// 把若干页图片组装成 PDF 字节。每页 MediaBox = 图片像素尺寸（1px=1pt）。
/// 对象编号：1=Catalog，2=Pages，之后每页 3 个对象（Page/Image/Contents）。
pub fn images_to_pdf(images: &[PdfImage]) -> Result<Vec<u8>, String> {
    if images.is_empty() {
        return Err("PDF 至少要有一页".into());
    }
    let n = images.len();
    let mut objects: Vec<Vec<u8>> = Vec::with_capacity(2 + n * 3);
    objects.push(b"<< /Type /Catalog /Pages 2 0 R >>".to_vec());
    let mut kids = String::new();
    for i in 0..n {
        kids.push_str(&format!("{} 0 R ", 3 + i * 3));
    }
    objects.push(
        format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.trim_end(), n).into_bytes(),
    );
    for (i, img) in images.iter().enumerate() {
        let page_id = 3 + i * 3;
        let image_id = page_id + 1;
        let contents_id = page_id + 2;
        objects.push(
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im0 {im} 0 R >> >> /Contents {con} 0 R >>",
                w = img.width, h = img.height, im = image_id, con = contents_id
            )
            .into_bytes(),
        );
        let mut xobj = format!(
            "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace {cs} /BitsPerComponent {bpc} /Filter {f} /Length {len} >>\nstream\n",
            w = img.width, h = img.height, cs = img.color.pdf_name(), bpc = img.bits, f = img.filter.pdf_name(), len = img.data.len()
        ).into_bytes();
        xobj.extend_from_slice(&img.data);
        xobj.extend_from_slice(b"\nendstream");
        objects.push(xobj);
        let content = format!("q\n{w} 0 0 {h} 0 0 cm\n/Im0 Do\nQ\n", w = img.width, h = img.height);
        objects.push(
            format!("<< /Length {} >>\nstream\n{}endstream", content.len(), content).into_bytes(),
        );
    }
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets: Vec<usize> = Vec::with_capacity(objects.len());
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        out.extend_from_slice(obj);
        out.extend_from_slice(b"\nendobj\n");
    }
    let xref_off = out.len();
    let count = objects.len() + 1; // 含空闲对象 0
    out.extend_from_slice(format!("xref\n0 {count}\n").as_bytes());
    out.extend_from_slice(b"0000000000 65535 f \n");
    for off in &offsets {
        out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(
        format!("trailer\n<< /Size {count} /Root 1 0 R >>\nstartxref\n{xref_off}\n%%EOF\n")
            .as_bytes(),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1x1 红色 PNG（真实字节，含 IHDR/IDAT/IEND）
    const RED_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn png_decodes_to_rgb() {
        let img = image_from_bytes(RED_PNG).unwrap();
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(img.color, ColorSpace::Rgb);
        assert_eq!(img.filter, Filter::Flate);
    }

    #[test]
    fn jpeg_sof_parses_dimensions() {
        // 构造最小 JPEG 骨架：SOI + SOF0(3 分量 2x3) + EOI
        let jpeg: &[u8] = &[
            0xFF, 0xD8, // SOI
            0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x03, 0x00, 0x02, 0x03, 0x01, 0x11, 0x00, 0x02,
            0x11, 0x01, 0x03, 0x11, 0x01, // SOF0: h=3 w=2 comps=3
            0xFF, 0xD9, // EOI
        ];
        let img = jpeg_to_image(jpeg).unwrap();
        assert_eq!((img.width, img.height), (2, 3));
        assert_eq!(img.color, ColorSpace::Rgb);
        assert_eq!(img.filter, Filter::Dct);
        assert_eq!(img.data, jpeg); // 原字节直嵌
    }

    #[test]
    fn pdf_structure_wellformed() {
        let img = image_from_bytes(RED_PNG).unwrap();
        let pdf = images_to_pdf(&[img]).unwrap();
        assert!(pdf.starts_with(b"%PDF-1.7"), "缺 PDF 头");
        assert!(pdf.ends_with(b"%%EOF\n"), "缺 EOF");
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/Type /Catalog"));
        assert!(s.contains("/Type /Pages"));
        assert!(s.contains("/Count 1"));
        assert!(s.contains("/Subtype /Image"));
        assert!(s.contains("/MediaBox [0 0 1 1]"));
        assert!(s.contains("startxref"));
        // xref 条目数 = 对象数(1 catalog +1 pages +3 每页) + 空闲 0 = 6
        assert!(s.contains("xref\n0 6\n"), "xref 计数错: {s}");
    }

    #[test]
    fn empty_pages_error() {
        assert!(images_to_pdf(&[]).is_err());
    }
}
