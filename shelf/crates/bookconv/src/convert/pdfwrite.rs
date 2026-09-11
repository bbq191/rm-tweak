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
#[derive(Clone)]
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
                for &v in &px[..3] {
                    out.push(((v as u32 * a + 255 * (255 - a)) / 255) as u8);
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
///
/// 逐对象直接写进（按总量预留好容量的）输出缓冲区——此前先给每个对象各建一份 `Vec<u8>`（图片字节整份复制一遍）、
/// 最后再拼成 `out`（又一遍），调用方持有的 `images` + `objects` + `out` 三份整本体积同时驻留；现在只剩 `images` + `out`。
/// 产物字节与旧实现逐字节一致（`images_to_pdf_output_bytes_are_stable` 钉住）。
pub fn images_to_pdf(images: &[PdfImage]) -> Result<Vec<u8>, String> {
    if images.is_empty() {
        return Err("PDF 至少要有一页".into());
    }
    let n = images.len();
    let cap = images.iter().map(|i| i.data.len() + 640).sum::<usize>() + n * 8 + 512;
    let mut out: Vec<u8> = Vec::with_capacity(cap);
    out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
    let mut offsets: Vec<usize> = Vec::with_capacity(2 + n * 3);
    // 写一个对象：`{id} 0 obj\n` + body + `\nendobj\n`，并记下起始偏移。
    fn obj(out: &mut Vec<u8>, offsets: &mut Vec<usize>, body: &[u8]) {
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(body);
        out.extend_from_slice(b"\nendobj\n");
    }
    obj(&mut out, &mut offsets, b"<< /Type /Catalog /Pages 2 0 R >>");
    let mut kids = String::new();
    for i in 0..n {
        kids.push_str(&format!("{} 0 R ", 3 + i * 3));
    }
    obj(&mut out, &mut offsets, format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.trim_end(), n).as_bytes());
    for (i, img) in images.iter().enumerate() {
        let page_id = 3 + i * 3;
        let image_id = page_id + 1;
        let contents_id = page_id + 2;
        obj(
            &mut out,
            &mut offsets,
            format!(
                "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Resources << /XObject << /Im0 {im} 0 R >> >> /Contents {con} 0 R >>",
                w = img.width, h = img.height, im = image_id, con = contents_id
            )
            .as_bytes(),
        );
        // 图片对象：头 + 流数据 + 尾，数据直接从 `img.data` 进 `out`（不经中间 Vec）。
        offsets.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        out.extend_from_slice(
            format!(
                "<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace {cs} /BitsPerComponent {bpc} /Filter {f} /Length {len} >>\nstream\n",
                w = img.width, h = img.height, cs = img.color.pdf_name(), bpc = img.bits, f = img.filter.pdf_name(), len = img.data.len()
            )
            .as_bytes(),
        );
        out.extend_from_slice(&img.data);
        out.extend_from_slice(b"\nendstream\nendobj\n");
        let content = format!("q\n{w} 0 0 {h} 0 0 cm\n/Im0 Do\nQ\n", w = img.width, h = img.height);
        obj(&mut out, &mut offsets, format!("<< /Length {} >>\nstream\n{}endstream", content.len(), content).as_bytes());
    }
    let xref_off = out.len();
    let count = offsets.len() + 1; // 含空闲对象 0
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

/// 统一设备页面尺寸（PDF 1px=1pt 沿用本文件既有惯例）——复用 EPUB 漫画线同一套设备屏幕常量
/// （真机实测 PDF 页面尺寸精确等于这组数字时，左右留白量得 0.00%，是这次新增摆位逻辑的依据）。
pub const PDF_PAGE_W: u32 = crate::imgopt::MAX_SHORT_EDGE;
pub const PDF_PAGE_H: u32 = crate::imgopt::MAX_EDGE;
/// 写进 Info 字典 `/Producer` 的标记字符串——`looks_like_own_bookconv_pdf` 靠这个识别"没有
/// 书签目录也是我们自己产出的 PDF"（裁边路径的产物大概率没有章节结构，不能只靠 `/Outlines`
/// 判断，见该函数文档）。
const PRODUCER_MARKER: &str = "cangjie-bookconv/1";

/// 图片在统一设备页面里怎么摆：优先按宽度撑满、左右各留约 1%（常见情况——漫画页比设备"矮"，
/// 撑满宽度后自然在上下留出对称留白，居中摆）；如果这样会导致高度溢出页面（罕见的极端竖长图），
/// 改成按高度撑满、不留任何上下空间，左右留白让步（避免内容溢出屏幕优先于"左右只留1%"这个目标，
/// 两者冲突时选前者）。全程只是计算一个缩放+平移矩阵，不碰图片本身一个像素。
///
/// **绘制宽取整、且与页宽同奇偶**（954 页宽 → 934，而不是 934.92）：左右边距 `(页宽-绘制宽)/2` 因此
/// 必为整数，纵向偏移也向下取整——调用方（`imgopt::prepare_comic_page_for_pdf`）预先把图片重采样成
/// 恰好这个像素尺寸时，阅读器按 1pt=1px 光栅化就是**逐像素 1:1、整数偏移**，不会再被二次重采样。
/// 之前绘制宽 934.92 + 非整数偏移，等于图片在阅读器里又被缩放+亚像素平移了一遍，网点漫画尤其明显
/// 变糊/起摩尔纹。
/// 返回 `(绘制宽, 绘制高, x偏移, y偏移)`，直接拼进 PDF Contents 流的 `cm` 矩阵。
pub fn place_image(img_w: u32, img_h: u32, page_w: u32, page_h: u32) -> (f32, f32, f32, f32) {
    let mut target_w_px = (page_w as f32 * 0.98).floor() as u32;
    if (page_w - target_w_px) % 2 == 1 {
        target_w_px -= 1;
    }
    let (img_w, img_h, page_w, page_h) = (img_w as f32, img_h as f32, page_w as f32, page_h as f32);
    let target_w = target_w_px as f32;
    let scaled_h = img_h * (target_w / img_w);
    if scaled_h <= page_h {
        (target_w, scaled_h, (page_w - target_w) / 2.0, ((page_h - scaled_h) / 2.0).floor())
    } else {
        let scale_h = page_h / img_h;
        let scaled_w = img_w * scale_h;
        (scaled_w, page_h, (page_w - scaled_w) / 2.0, 0.0)
    }
}

/// PDF 书签标题必须用 UTF-16BE（带 `\xFE\xFF` BOM）才能正确显示中文，纯 ASCII 字面量不行。
fn pdf_text_utf16be(s: &str) -> Vec<u8> {
    let mut out = vec![0xFEu8, 0xFF];
    for u in s.encode_utf16() {
        out.push((u >> 8) as u8);
        out.push((u & 0xFF) as u8);
    }
    out
}

/// 包成 PDF literal string `(...)`，转义 `(`/`)`/`\` 三个特殊字节（UTF-16BE 的某个码元低/高字节
/// 凑巧撞上这三个 ASCII 值也要转义，否则会被误判为字符串提前结束）。
fn pdf_literal_string(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len() + 2);
    out.push(b'(');
    for &b in bytes {
        if b == b'(' || b == b')' || b == b'\\' {
            out.push(b'\\');
        }
        out.push(b);
    }
    out.push(b')');
    out
}

/// `images_to_pdf` 的平行版本，供 EPUB 漫画→PDF 这条新路径用：①统一页面尺寸（不再跟随每张图
/// 自己的像素尺寸），图片按 `place_image` 算出的位置摆放，不裁不拉伸；②带书签目录（Outlines）
/// ——`titles` 是 `(0-based 页码, 标题)` 列表，允许为空（不带任何目录，此时 Catalog 不含
/// `/Outlines`，效果等同旧版 `images_to_pdf` 只是页面尺寸统一了）。**不修改 `images_to_pdf`
/// 本体**——CBZ→PDF 那条现有路径继续用它，互不影响。
/// 单遍流式写一份 PDF：调用方逐页 `write_page` 喂图片，**每喂一页就直接写进内部输出缓冲区，
/// 喂完这一页那张 [`PdfImage`] 就可以丢了**，不需要先攒成 `Vec<PdfImage>` 再一次性序列化。
/// 真机 245MB/600页 样本坐实过两轮问题：①最早的写法先把每个对象各自建一份 `Vec<u8>` 存进
/// `objects`、最后再拼一遍，图片字节在"调用方持有的 images / objects 副本 / 最终 out"三处
/// 同时占内存；②改成一遍写完 `out`（不建 `objects` 中间层）后降到两份，但分卷投递那条路径当时
/// 仍然是"先把一份要用到的图片全部读成 `Vec<PdfImage>`，再整个传给 `images_to_pdf_with_toc`"，
/// 峰值还是贴着"一份的体积"两倍。这个类型是第三轮修法：`page_count` 从页码范围直接算好（不用
/// 等真的读完每一页才知道有几页），`write_page` 每调一次就地把这一页的对象写进 `out`、调用方读
/// 完一页的 [`PdfImage`] 传进来、这次调用结束后那份图片数据就可以释放——峰值降到约等于"一份的
/// 体积"本身，不再是它的两倍。[`images_to_pdf_with_toc`] 现在是这个类型的薄封装，行为/输出字节
/// 完全不变（`images_to_pdf_with_toc_*` 系列测试原样覆盖）。
pub struct PdfPieceWriter {
    out: Vec<u8>,
    offsets: Vec<usize>,
    n: usize,
    written: usize,
    has_toc: bool,
    outline_root_id: usize,
}

impl PdfPieceWriter {
    /// `page_count`＝这份 PDF 总共几页（**调用方必须提前知道**——通常就是页码范围的长度，不需要
    /// 真的读出图片内容才能知道），`has_toc`＝是否会有书签（决定 Catalog 要不要写 `/Outlines`）。
    pub fn begin(page_count: usize, has_toc: bool) -> Self {
        let outline_root_id = 3 + page_count * 3;
        let mut out = Vec::new();
        out.extend_from_slice(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n");
        let mut offsets = Vec::with_capacity(2 + page_count * 3);

        let obj1_off = out.len();
        out.extend_from_slice(b"1 0 obj\n");
        if has_toc {
            out.extend_from_slice(format!("<< /Type /Catalog /Pages 2 0 R /Outlines {outline_root_id} 0 R /PageMode /UseOutlines >>").as_bytes());
        } else {
            out.extend_from_slice(b"<< /Type /Catalog /Pages 2 0 R >>");
        }
        out.extend_from_slice(b"\nendobj\n");
        offsets.push(obj1_off);

        let obj2_off = out.len();
        out.extend_from_slice(b"2 0 obj\n<< /Type /Pages /Kids [");
        for i in 0..page_count {
            out.extend_from_slice(format!("{} 0 R ", 3 + i * 3).as_bytes());
        }
        out.extend_from_slice(format!("] /Count {page_count} >>\nendobj\n").as_bytes());
        offsets.push(obj2_off);

        Self { out, offsets, n: page_count, written: 0, has_toc, outline_root_id }
    }

    /// 喂下一页的图片——按调用顺序对应页码 0,1,2,...；这次调用结束后，传进来的 `img` 本身
    /// 就可以被调用方释放了（它的字节已经拷进内部 `out` 缓冲区，不再需要）。
    pub fn write_page(&mut self, img: &PdfImage) -> Result<(), String> {
        if self.written >= self.n {
            return Err(format!("PdfPieceWriter 声明了 {} 页，多喂了一页", self.n));
        }
        let i = self.written;
        let page_id = 3 + i * 3;
        let image_id = page_id + 1;
        let contents_id = page_id + 2;
        let (dw, dh, x, y) = place_image(img.width, img.height, PDF_PAGE_W, PDF_PAGE_H);

        self.offsets.push(self.out.len());
        self.out.extend_from_slice(
            format!(
                "{page_id} 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PDF_PAGE_W} {PDF_PAGE_H}] /Resources << /XObject << /Im0 {image_id} 0 R >> >> /Contents {contents_id} 0 R >>\nendobj\n"
            ).as_bytes(),
        );

        self.offsets.push(self.out.len());
        self.out.extend_from_slice(
            format!(
                "{image_id} 0 obj\n<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace {cs} /BitsPerComponent {bpc} /Filter {f} /Length {len} >>\nstream\n",
                w = img.width, h = img.height, cs = img.color.pdf_name(), bpc = img.bits, f = img.filter.pdf_name(), len = img.data.len()
            ).as_bytes(),
        );
        self.out.extend_from_slice(&img.data); // 图片字节唯一一次拷贝：从调用方的 img 直接进 out
        self.out.extend_from_slice(b"\nendstream\nendobj\n");

        self.offsets.push(self.out.len());
        let content = format!("q\n{dw} 0 0 {dh} {x} {y} cm\n/Im0 Do\nQ\n");
        self.out.extend_from_slice(
            format!("{contents_id} 0 obj\n<< /Length {} >>\nstream\n{}endstream\nendobj\n", content.len(), content).as_bytes(),
        );

        self.written += 1;
        Ok(())
    }

    /// 全部页写完后调用一次，补书签目录（如果有）+ xref + trailer，收尾成完整 PDF 字节。
    pub fn finish(mut self, titles: &[(usize, String)]) -> Result<Vec<u8>, String> {
        if self.written != self.n {
            return Err(format!("PdfPieceWriter 声明了 {} 页，实际只写了 {}", self.n, self.written));
        }
        if self.has_toc != !titles.is_empty() {
            return Err("PdfPieceWriter::begin 的 has_toc 跟 finish 传的 titles 对不上".into());
        }
        // 书签指向不存在的页＝写出一个指向不存在对象的 `/Dest`，读回来（`PdfFileReader::outline_titles`）就是越界页码。
        if let Some((p, t)) = titles.iter().find(|(p, _)| *p >= self.n) {
            return Err(format!("书签《{t}》指向第 {} 页，但只有 {} 页", p + 1, self.n));
        }
        if self.has_toc {
            let base = self.outline_root_id;
            let first_id = base + 1;
            let last_id = base + titles.len();
            self.offsets.push(self.out.len());
            self.out.extend_from_slice(
                format!("{base} 0 obj\n<< /Type /Outlines /First {first_id} 0 R /Last {last_id} 0 R /Count {} >>\nendobj\n", titles.len()).as_bytes(),
            );
            for (i, (page_idx, title)) in titles.iter().enumerate() {
                let item_id = base + 1 + i;
                let page_id = 3 + page_idx * 3;
                self.offsets.push(self.out.len());
                self.out.extend_from_slice(format!("{item_id} 0 obj\n<< /Title ").as_bytes());
                self.out.extend_from_slice(&pdf_literal_string(&pdf_text_utf16be(title)));
                self.out.extend_from_slice(format!(" /Parent {base} 0 R").as_bytes());
                if i > 0 {
                    self.out.extend_from_slice(format!(" /Prev {} 0 R", base + i).as_bytes());
                }
                if i + 1 < titles.len() {
                    self.out.extend_from_slice(format!(" /Next {} 0 R", base + 2 + i).as_bytes());
                }
                self.out.extend_from_slice(format!(" /Dest [{page_id} 0 R /Fit] >>\nendobj\n").as_bytes());
            }
        }

        // Info 对象（`/Producer` 标记）放在所有页/书签对象之后、xref 之前——下一个可用对象号
        // 就是"到这里为止已经写了几个对象"（`self.offsets.len()` 跟已写对象数严格一一对应，
        // 因为这个写手从来没有跳号）。不影响页/书签对象的既有编号方案，纯追加，对已部署真机的
        // 漫画 PDF 产出字节结构不构成破坏性变动（Info 只是新增，不是改写）。
        let info_id = self.offsets.len() + 1;
        self.offsets.push(self.out.len());
        self.out.extend_from_slice(format!("{info_id} 0 obj\n<< /Producer ").as_bytes());
        self.out.extend_from_slice(&pdf_literal_string(PRODUCER_MARKER.as_bytes()));
        self.out.extend_from_slice(b" >>\nendobj\n");

        let xref_off = self.out.len();
        let count = self.offsets.len() + 1;
        self.out.extend_from_slice(format!("xref\n0 {count}\n").as_bytes());
        self.out.extend_from_slice(b"0000000000 65535 f \n");
        for off in &self.offsets {
            self.out.extend_from_slice(format!("{off:010} 00000 n \n").as_bytes());
        }
        self.out.extend_from_slice(
            format!("trailer\n<< /Size {count} /Root 1 0 R /Info {info_id} 0 R >>\nstartxref\n{xref_off}\n%%EOF\n").as_bytes(),
        );
        Ok(self.out)
    }
}

/// [`PdfPieceWriter`] 的薄封装——一次性喂完整个 `images` 切片，行为/输出字节跟改用
/// `PdfPieceWriter` 之前完全一致。调用方如果自己逐页读图片（比如分卷投递流式路径），直接用
/// `PdfPieceWriter` 更省内存，不用先攒出这个 `&[PdfImage]`。
pub fn images_to_pdf_with_toc(images: &[PdfImage], titles: &[(usize, String)]) -> Result<Vec<u8>, String> {
    if images.is_empty() {
        return Err("PDF 至少要有一页".into());
    }
    let mut w = PdfPieceWriter::begin(images.len(), !titles.is_empty());
    for img in images {
        w.write_page(img)?;
    }
    w.finish(titles)
}

fn memfind(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// 对象号 → 字节偏移索引，从我们自己写的 xref 表一次性解出（真机 600 页/200MB 样本坐实：早期版本
/// `find_obj_body` 每次都从文件开头线性扫一遍找 `"{id} 0 obj"`，`extract_pages` 对几百个对象各
/// 扫一次，等效扫描量接近"页数×文件体积"，600 页的书卡了 11 分钟没跑完；这里改成先解一次 xref
/// （单次线性扫，O(文件体积)）拿到每个对象号的精确偏移，后续每次对象查找变成 O(1) 定位 + 只在
/// **这一个对象自身**范围内找 `endobj`，不再牵连整份文件）。
/// 只认自己 `images_to_pdf`/`images_to_pdf_with_toc` 写出的经典 xref 表格式（每条固定 20 字节：
/// `"{10位偏移} 00000 {f|n} \n"`），喂陌生 PDF（比如带交叉引用流的现代 PDF）大概率直接 `Err`。
fn parse_obj_index(pdf: &[u8]) -> Result<Vec<usize>, String> {
    // "startxref\n{偏移}\n%%EOF\n" 是我们写的固定尾巴，最长不过几十字节，只在文件最后一小段找，
    // 不用扫全文件。
    let tail_from = pdf.len().saturating_sub(256);
    let tail = &pdf[tail_from..];
    let marker = b"startxref";
    let rel = memfind(tail, marker).ok_or("缺 startxref")?;
    let after = &tail[rel + marker.len()..];
    let ds = after.iter().position(|b| b.is_ascii_digit()).ok_or("startxref 后缺数字")?;
    let de = after[ds..].iter().position(|b| !b.is_ascii_digit()).map(|o| ds + o).unwrap_or(after.len());
    let xref_off: usize =
        std::str::from_utf8(&after[ds..de]).ok().and_then(|s| s.parse().ok()).ok_or("startxref 数值解析失败")?;

    let header = b"xref\n0 ";
    if pdf.len() < xref_off + header.len() || &pdf[xref_off..xref_off + header.len()] != header {
        return Err("xref 头格式不认识（不是我们自己写的经典表）".into());
    }
    let mut p = xref_off + header.len();
    let count_start = p;
    while p < pdf.len() && pdf[p].is_ascii_digit() {
        p += 1;
    }
    let count: usize =
        std::str::from_utf8(&pdf[count_start..p]).ok().and_then(|s| s.parse().ok()).ok_or("xref count 解析失败")?;
    if pdf.get(p) != Some(&b'\n') {
        return Err("xref count 后缺换行".into());
    }
    p += 1;
    if count.checked_mul(20).and_then(|n| n.checked_add(p)).is_none_or(|end| end > pdf.len()) {
        return Err("xref 条目数据被截断".into());
    }
    let mut offsets = vec![0usize; count];
    for (i, off) in offsets.iter_mut().enumerate() {
        let line = &pdf[p + i * 20..p + i * 20 + 10];
        *off = std::str::from_utf8(line).ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    }
    Ok(offsets)
}

/// `index` 是 [`parse_obj_index`] 的结果，`index[id]` = 对象 `id` 的 `"{id} 0 obj\n"` 起始偏移。
fn find_obj_body_indexed<'a>(pdf: &'a [u8], index: &[usize], id: usize) -> Result<&'a [u8], String> {
    let pos = *index.get(id).ok_or_else(|| format!("对象 {id} 不在 xref 索引范围内"))?;
    let marker = format!("{id} 0 obj\n");
    if !pdf[pos..].starts_with(marker.as_bytes()) {
        return Err(format!("对象 {id} 的 xref 偏移跟实际内容对不上（{pos}）"));
    }
    let start = pos + marker.len();
    let rel_end = memfind(&pdf[start..], b"\nendobj").ok_or_else(|| format!("对象 {id} 缺 endobj"))?;
    Ok(&pdf[start..start + rel_end])
}

fn parse_uint_after(body: &[u8], label: &str) -> Option<u32> {
    let idx = memfind(body, label.as_bytes())? + label.len();
    let mut end = idx;
    while end < body.len() && body[end].is_ascii_digit() {
        end += 1;
    }
    if end == idx {
        return None;
    }
    std::str::from_utf8(&body[idx..end]).ok()?.parse().ok()
}

/// 解出 `/Title (...)` 这个 literal string 的原始字节（保留转义前的 `\(`/`\)`/`\\`）。
fn parse_pdf_literal(body: &[u8], after: &str) -> Option<Vec<u8>> {
    let idx = memfind(body, after.as_bytes())? + after.len();
    if body.get(idx) != Some(&b'(') {
        return None;
    }
    let mut i = idx + 1;
    let mut out = Vec::new();
    while i < body.len() {
        match body[i] {
            b'\\' if i + 1 < body.len() => {
                out.push(body[i + 1]);
                i += 2;
            }
            b')' => return Some(out),
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    None
}

fn utf16be_to_string(mut bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
        bytes = &bytes[2..];
    }
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| ((c[0] as u16) << 8) | c[1] as u16).collect();
    String::from_utf16_lossy(&units)
}

/// 廉价识别"这份 PDF 是不是我们自己 `images_to_pdf_with_toc` 产出的"——只读文件开头一小段
/// （Catalog 永远是第一个写的对象，在文件最前面），不用把整份 PDF 读进内存解析对象结构。给
/// `book-serve::Staging::list()` 这种高频调用场景判断"漫画 PDF 是否已优化"用。
/// 泛化自原来的 `looks_like_own_comic_pdf`（2026-09-19，入库 PDF 裁边那条产线接入时）：裁边
/// 输出大概率没有章节结构（原书可能就没有书签目录），不能只靠头部 `/Outlines` 判断——补一条
/// 尾部检查 `PRODUCER_MARKER`（`PdfPieceWriter::finish` 写的 Info 对象，固定在 xref 之前、
/// 文件末尾附近）。两条检查任一命中即可，两条产线（漫画 EPUB→PDF / PDF 裁边）共用同一个
/// "这是我们自己优化过的" 信号，`list()`/前端徽章逻辑不用关心具体是哪条产线产出的。
pub fn looks_like_own_bookconv_pdf(path: &std::path::Path) -> bool {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    let mut head = Vec::with_capacity(4096);
    if (&f).take(4096).read_to_end(&mut head).is_err() {
        return false;
    }
    if memfind(&head, b"/Type /Catalog").is_some() && memfind(&head, b"/Outlines").is_some() {
        return true;
    }
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let tail_len = len.min(4096);
    if f.seek(SeekFrom::End(-(tail_len as i64))).is_err() {
        return false;
    }
    let mut tail = Vec::with_capacity(tail_len as usize);
    if f.take(tail_len).read_to_end(&mut tail).is_err() {
        return false;
    }
    memfind(&tail, PRODUCER_MARKER.as_bytes()).is_some()
}

/// 总页数（`/Type /Pages` 对象的 `/Count`）——只认自己生成的固定对象编号结构，喂陌生 PDF 大概率 `Err`。
pub fn page_count(pdf: &[u8]) -> Result<usize, String> {
    let index = parse_obj_index(pdf)?;
    let pages_body = find_obj_body_indexed(pdf, &index, 2)?;
    parse_uint_after(pages_body, "/Count ").ok_or_else(|| "Pages 对象缺 /Count".to_string()).map(|n| n as usize)
}

/// 对象在文件里的精确字节范围：`[offsets[id], offsets[id+1])`（最后一个对象到 `xref_off`）——
/// 这个区间本身就天然包含 `"{id} 0 obj\n...\nendobj\n"` 整段，不用另外找 `endobj` 在哪。
fn object_byte_span(offsets: &[usize], xref_off: usize, id: usize) -> Result<(usize, usize), String> {
    let start = *offsets.get(id).ok_or_else(|| format!("对象 {id} 不在 xref 索引范围内"))?;
    let end = offsets.get(id + 1).copied().unwrap_or(xref_off);
    if end <= start {
        return Err(format!("对象 {id} 字节范围非法"));
    }
    Ok((start, end))
}

/// 文件版 xref 解析——跟内存版 [`parse_obj_index`] 逻辑同构，只是从磁盘按需读（先读文件尾一小段
/// 找 `startxref`，再定位读 xref 表本身），不需要整份文件常驻内存。
fn parse_obj_index_from_file(file: &mut std::fs::File) -> Result<(Vec<usize>, usize), String> {
    use std::io::{Read, Seek, SeekFrom};
    let file_len = file.metadata().map_err(|e| e.to_string())?.len();
    let tail_len = 256u64.min(file_len);
    file.seek(SeekFrom::End(-(tail_len as i64))).map_err(|e| e.to_string())?;
    let mut tail = vec![0u8; tail_len as usize];
    file.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let rel = memfind(&tail, b"startxref").ok_or("缺 startxref")?;
    let after = &tail[rel + b"startxref".len()..];
    let ds = after.iter().position(|b| b.is_ascii_digit()).ok_or("startxref 后缺数字")?;
    let de = after[ds..].iter().position(|b| !b.is_ascii_digit()).map(|o| ds + o).unwrap_or(after.len());
    let xref_off: usize =
        std::str::from_utf8(&after[ds..de]).ok().and_then(|s| s.parse().ok()).ok_or("startxref 数值解析失败")?;

    file.seek(SeekFrom::Start(xref_off as u64)).map_err(|e| e.to_string())?;
    let header = b"xref\n0 ";
    let mut hdr = vec![0u8; header.len()];
    file.read_exact(&mut hdr).map_err(|e| e.to_string())?;
    if hdr != header {
        return Err("xref 头格式不认识（不是我们自己写的经典表）".into());
    }
    let mut count_digits = Vec::new();
    loop {
        let mut b = [0u8; 1];
        file.read_exact(&mut b).map_err(|e| e.to_string())?;
        if b[0].is_ascii_digit() {
            count_digits.push(b[0]);
        } else if b[0] == b'\n' {
            break;
        } else {
            return Err("xref count 格式不认识".into());
        }
    }
    let count: usize =
        std::str::from_utf8(&count_digits).ok().and_then(|s| s.parse().ok()).ok_or("xref count 解析失败")?;
    // count 来自文件本身：先核对"条目表真的装得进文件剩余部分"再分配——损坏/陌生 PDF 写个天文数字，照单
    // `vec![0; count*20]` 在设备上是分配失败直接 abort（不是 catch_unwind 兜得住的 panic）。这个函数也会被
    // 用户自己上传的第三方 PDF 走到（>90MB 占位通道读页数、超限拆分投递），不只是自己写的文件。
    let pos = file.stream_position().map_err(|e| e.to_string())?;
    if count.checked_mul(20).is_none_or(|n| n as u64 > file_len.saturating_sub(pos)) {
        return Err("xref 条目数据被截断".into());
    }
    let mut entries = vec![0u8; count * 20];
    file.read_exact(&mut entries).map_err(|e| e.to_string())?;
    let mut offsets = vec![0usize; count];
    for (i, off) in offsets.iter_mut().enumerate() {
        let line = &entries[i * 20..i * 20 + 10];
        *off = std::str::from_utf8(line).ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    }
    Ok((offsets, xref_off))
}

fn read_object_body_from_file(
    file: &mut std::fs::File,
    offsets: &[usize],
    xref_off: usize,
    id: usize,
) -> Result<Vec<u8>, String> {
    use std::io::{Read, Seek, SeekFrom};
    let (start, end) = object_byte_span(offsets, xref_off, id)?;
    file.seek(SeekFrom::Start(start as u64)).map_err(|e| e.to_string())?;
    let mut raw = vec![0u8; end - start];
    file.read_exact(&mut raw).map_err(|e| e.to_string())?;
    let marker = format!("{id} 0 obj\n");
    if !raw.starts_with(marker.as_bytes()) {
        return Err(format!("对象 {id} 读取内容跟预期标记不符"));
    }
    let body_start = marker.len();
    let rel_end = memfind(&raw[body_start..], b"\nendobj").ok_or_else(|| format!("对象 {id} 缺 endobj"))?;
    Ok(raw[body_start..body_start + rel_end].to_vec())
}

/// **只认自己 `images_to_pdf_with_toc` 生成的 PDF**——流式按需从磁盘读单个对象，不整份文件读进
/// 内存，也不一次性把全书图片攒成 `Vec<PdfImage>`（真机 245MB/600页 样本坐实过：旧版一次性抽取
/// 全书图片，`VmHWM` 峰值 525MB；这个读法配合调用方"一份处理完立刻丢"的用法，峰值只有"一份的
/// 体积"）。不是通用 PDF 解析器，喂陌生第三方 PDF 大概率直接 `Err`。
pub struct PdfFileReader {
    file: std::fs::File,
    offsets: Vec<usize>,
    xref_off: usize,
}

impl PdfFileReader {
    pub fn open(path: &std::path::Path) -> Result<Self, String> {
        let mut file = std::fs::File::open(path).map_err(|e| format!("打开 PDF 失败: {e}"))?;
        let (offsets, xref_off) = parse_obj_index_from_file(&mut file)?;
        Ok(Self { file, offsets, xref_off })
    }

    fn read_object(&mut self, id: usize) -> Result<Vec<u8>, String> {
        read_object_body_from_file(&mut self.file, &self.offsets, self.xref_off, id)
    }

    /// `/Type /Pages` 对象的 `/Count`。
    pub fn page_count(&mut self) -> Result<usize, String> {
        let pages_body = self.read_object(2)?;
        parse_uint_after(&pages_body, "/Count ").ok_or_else(|| "Pages 对象缺 /Count".to_string()).map(|n| n as usize)
    }

    /// 这一页的图片对象在文件里占的精确字节数（含对象包装本身，不只是图片数据）——纯查表运算，
    /// 不碰磁盘，给"按体积贪心分组"这类只需要相对大小、不需要读出真实内容的场景用。
    pub fn page_byte_span_len(&self, page_idx: usize) -> u64 {
        let image_id = 3 + page_idx * 3 + 1;
        object_byte_span(&self.offsets, self.xref_off, image_id).map(|(s, e)| (e - s) as u64).unwrap_or(0)
    }

    /// 全书书签，`(全局页码, 标题)`——没有 Outlines（不是我们自己产出的漫画 PDF）返回空列表，
    /// 不是错误。
    pub fn outline_titles(&mut self) -> Result<Vec<(usize, String)>, String> {
        let catalog = self.read_object(1)?;
        let mut titles = Vec::new();
        if let Some(outline_root) = parse_uint_after(&catalog, "/Outlines ") {
            let root_body = self.read_object(outline_root as usize)?;
            let first = parse_uint_after(&root_body, "/First ").ok_or("Outlines 缺 /First")? as usize;
            let last = parse_uint_after(&root_body, "/Last ").ok_or("Outlines 缺 /Last")? as usize;
            if last < first || last >= self.offsets.len() {
                return Err(format!("书签对象号范围 {first}..={last} 不合法"));
            }
            for item_id in first..=last {
                let item_body = self.read_object(item_id)?;
                let dest_page_id = parse_uint_after(&item_body, "/Dest [").ok_or("书签缺 /Dest")? as usize;
                // 自己写的页对象号恒为 3+3i；别的值说明不是我们的文件（或已损坏）——此前直接 `(id-3)/3`，
                // id<3 时减法溢出（release 下回绕成天文数字页码，调用方切片越界 panic）。
                if dest_page_id < 3 || !(dest_page_id - 3).is_multiple_of(3) {
                    return Err(format!("书签指向的对象 {dest_page_id} 不是页对象"));
                }
                let page_idx = (dest_page_id - 3) / 3;
                let title_bytes = parse_pdf_literal(&item_body, "/Title ").ok_or("书签缺 /Title")?;
                titles.push((page_idx, utf16be_to_string(&title_bytes)));
            }
        }
        Ok(titles)
    }

    /// 只读这一页的图片（这一次调用的内存开销就是这一张图自身的字节数，读完可以立刻丢）。
    pub fn read_page_image(&mut self, page_idx: usize) -> Result<PdfImage, String> {
        let image_id = 3 + page_idx * 3 + 1;
        let body = self.read_object(image_id)?;
        let w = parse_uint_after(&body, "/Width ").ok_or("图片对象缺 /Width")?;
        let h = parse_uint_after(&body, "/Height ").ok_or("图片对象缺 /Height")?;
        let color = if memfind(&body, b"/DeviceGray").is_some() { ColorSpace::Gray } else { ColorSpace::Rgb };
        let bits = parse_uint_after(&body, "/BitsPerComponent ").ok_or("图片对象缺 /BitsPerComponent")? as u8;
        let filter = if memfind(&body, b"/DCTDecode").is_some() { Filter::Dct } else { Filter::Flate };
        let len = parse_uint_after(&body, "/Length ").ok_or("图片对象缺 /Length")? as usize;
        let stream_marker = b"stream\n";
        let stream_at = memfind(&body, stream_marker).ok_or("图片对象缺 stream")? + stream_marker.len();
        if stream_at + len > body.len() {
            return Err(format!("图片对象 {image_id} 流数据被截断"));
        }
        Ok(PdfImage { width: w, height: h, color, bits, filter, data: body[stream_at..stream_at + len].to_vec() })
    }
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

    /// 改成"逐对象直接写 out"之前的旧实现（先建 `objects: Vec<Vec<u8>>` 再拼接）原样保留作参照。
    fn images_to_pdf_reference(images: &[PdfImage]) -> Result<Vec<u8>, String> {
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

    #[test]
    fn images_to_pdf_output_bytes_are_stable() {
        // 差分：新实现必须与旧实现逐字节一致（混合 PNG/JPEG、多页，覆盖 Flate/DCT 两种 filter 与多页对象编号）。
        let jpeg = vec![
            0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x11, 0x08, 0x00, 0x10, 0x00, 0x20, 0x03, 0x01, 0x11,
            0x00, 0x02, 0x11, 0x01, 0x03, 0x11, 0x01, 0xFF, 0xD9,
        ];
        let imgs = vec![image_from_bytes(RED_PNG).unwrap(), image_from_bytes(&jpeg).unwrap(), image_from_bytes(RED_PNG).unwrap()];
        assert_eq!(images_to_pdf(&imgs).unwrap(), images_to_pdf_reference(&imgs).unwrap());
        assert!(images_to_pdf(&[]).is_err());
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

    #[test]
    fn place_image_wide_page_centers_top_bottom_with_one_percent_side_margin() {
        // 常见漫画页：比设备页面"矮"（宽/高比 0.7 > 设备 0.5625），按宽度撑满，上下自动留白对称。
        let (dw, dh, x, y) = place_image(700, 1000, 954, 1696);
        assert!((x - 954.0 * 0.01).abs() < 1.0, "左边距应约为页宽 1%: x={x}");
        assert!((dw - 954.0 * 0.98).abs() < 1.0, "绘制宽应约为页宽 98%: dw={dw}");
        assert_eq!(x.fract(), 0.0, "左边距必须是整数像素（避免阅读器亚像素重采样）: x={x}");
        assert_eq!(dw.fract(), 0.0, "绘制宽必须是整数像素: dw={dw}");
        let bottom_gap = 1696.0 - dh - y;
        assert!((y - bottom_gap).abs() <= 2.0, "上下留白应对称（允许取整 ≤2px：绘制高小数部分+向下取整）: top={y} bottom={bottom_gap}");
    }

    #[test]
    fn place_image_extreme_tall_image_falls_back_to_height_fit_without_overflow() {
        // 极端竖长图：按 98% 宽度撑满会导致高度溢出页面，改按高度撑满避免溢出。
        let (dw, dh, x, y) = place_image(100, 1000, 954, 1696);
        assert_eq!(y, 0.0, "改按高度撑满时不留上下边距");
        assert!((dh - 1696.0).abs() < 0.01, "绘制高应等于页高: dh={dh}");
        assert!(dw < 954.0 * 0.98, "改按高度撑满后绘制宽应小于 98% 页宽（左右留白让步）: dw={dw}");
        assert!(x > 0.0, "水平应居中留白");
    }

    #[test]
    fn images_to_pdf_with_toc_structure_and_chinese_title() {
        let img1 = image_from_bytes(RED_PNG).unwrap();
        let img2 = image_from_bytes(RED_PNG).unwrap();
        let pdf = images_to_pdf_with_toc(&[img1, img2], &[(0, "镖人 卷一".to_string())]).unwrap();
        assert!(pdf.starts_with(b"%PDF-1.7"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        let s = String::from_utf8_lossy(&pdf);
        assert!(s.contains("/Type /Outlines"), "缺 Outlines 根对象");
        assert!(s.contains("/PageMode /UseOutlines"));
        assert!(s.contains(&format!("/MediaBox [0 0 {PDF_PAGE_W} {PDF_PAGE_H}]")), "页面尺寸应统一为设备尺寸");
        // 中文标题必须是 UTF-16BE + BOM 编码，不能原样出现 UTF-8 字节
        assert!(!s.contains("镖人"), "中文标题不该以 UTF-8 明文出现在 PDF 里");
        let title_utf16 = pdf_text_utf16be("镖人 卷一");
        let title_literal = pdf_literal_string(&title_utf16);
        assert!(pdf.windows(title_literal.len()).any(|w| w == title_literal.as_slice()), "找不到编码后的书签标题字节");
    }

    #[test]
    fn images_to_pdf_with_toc_empty_titles_has_no_outlines() {
        let img = image_from_bytes(RED_PNG).unwrap();
        let pdf = images_to_pdf_with_toc(&[img], &[]).unwrap();
        let s = String::from_utf8_lossy(&pdf);
        assert!(!s.contains("/Outlines"), "空标题列表不该产出 Outlines 对象");
    }

    /// xref 头里的条目数来自文件本身：写个天文数字不能照单分配（设备上是分配失败 abort，不是可兜底的 panic）。
    #[test]
    fn pdf_file_reader_rejects_absurd_xref_count_without_allocating() {
        let mut pdf = b"%PDF-1.7\n1 0 obj\n<< >>\nendobj\n".to_vec();
        let xref_off = pdf.len();
        pdf.extend_from_slice(b"xref\n0 99999999999999\n0000000000 65535 f \n");
        pdf.extend_from_slice(format!("trailer\n<< >>\nstartxref\n{xref_off}\n%%EOF\n").as_bytes());
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.pdf");
        std::fs::write(&path, &pdf).unwrap();
        assert!(PdfFileReader::open(&path).is_err());
        assert!(page_count(&pdf).is_err(), "内存版同样不能溢出/越界");
    }

    #[test]
    fn finish_rejects_bookmark_past_last_page() {
        let img = image_from_bytes(RED_PNG).unwrap();
        let err = images_to_pdf_with_toc(&[img.clone(), img], &[(0, "a".into()), (2, "b".into())]).unwrap_err();
        assert!(err.contains("只有 2 页"), "{err}");
    }

    #[test]
    fn pdf_file_reader_round_trips_images_and_titles() {
        let images: Vec<PdfImage> = (0..4).map(|_| image_from_bytes(RED_PNG).unwrap()).collect();
        let titles = vec![(0, "第一卷".to_string()), (2, "第二卷".to_string())];
        let pdf = images_to_pdf_with_toc(&images, &titles).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.pdf");
        std::fs::write(&path, &pdf).unwrap();

        let mut reader = PdfFileReader::open(&path).unwrap();
        assert_eq!(reader.page_count().unwrap(), 4);
        assert_eq!(reader.outline_titles().unwrap(), titles);
        for (i, orig) in images.iter().enumerate() {
            let got = reader.read_page_image(i).unwrap();
            assert_eq!(orig.width, got.width);
            assert_eq!(orig.height, got.height);
            assert_eq!(orig.color, got.color);
            assert_eq!(orig.filter, got.filter);
            assert_eq!(orig.data, got.data, "逐页读出的图片流字节应跟生成时完全一致");
        }
    }

    #[test]
    fn looks_like_own_bookconv_pdf_detects_outline_not_plain_pdf() {
        let img = image_from_bytes(RED_PNG).unwrap();
        let with_toc = images_to_pdf_with_toc(&[img], &[(0, "卷一".to_string())]).unwrap();
        let img2 = image_from_bytes(RED_PNG).unwrap();
        let plain = images_to_pdf(&[img2]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let p1 = dir.path().join("a.pdf");
        let p2 = dir.path().join("b.pdf");
        std::fs::write(&p1, &with_toc).unwrap();
        std::fs::write(&p2, &plain).unwrap();
        assert!(looks_like_own_bookconv_pdf(&p1));
        assert!(!looks_like_own_bookconv_pdf(&p2), "无 Outlines/Producer 标记的普通 PDF 不该被误判为我们自己的产物");
    }

    #[test]
    fn looks_like_own_bookconv_pdf_detects_producer_marker_without_outline() {
        // 裁边路径没有书签目录（原书可能就没有章节结构）——靠 finish() 写的 Producer 标记识别。
        let img = image_from_bytes(RED_PNG).unwrap();
        let no_toc = images_to_pdf_with_toc(&[img], &[]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("trim_only.pdf");
        std::fs::write(&p, &no_toc).unwrap();
        assert!(looks_like_own_bookconv_pdf(&p), "无书签但带 Producer 标记的自产 PDF 应该被识别");
    }

    #[test]
    fn pdf_file_reader_stays_fast_on_many_pages() {
        // 真机 600 页/245MB 样本坐实过两轮问题：①改索引化之前，对象查找是"每次都从文件开头
        // 线性扫一遍"，等效扫描量接近页数×文件体积，超限分卷投递卡了 11 分钟一份都没传上；
        // ②索引化只修了 CPU 复杂度，`extract_pages` 仍然一次性把全书图片读进 `Vec<PdfImage>`，
        // `VmHWM` 峰值到过 525MB。这里测的是索引化本身（500 页耗时应该秒出，不该随页数退化）；
        // "不一次性攒全书图片"这条内存纪律由 `PdfFileReader::read_page_image` 逐页读、调用方
        // 逐份处理完就丢来保证，不是这个测试测的范围（真机 `VmHWM` 前后对比见部署验证记录）。
        let images: Vec<PdfImage> = (0..500).map(|_| image_from_bytes(RED_PNG).unwrap()).collect();
        let titles: Vec<(usize, String)> = (0..500).step_by(50).map(|i| (i, format!("第{i}页"))).collect();
        let pdf = images_to_pdf_with_toc(&images, &titles).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.pdf");
        std::fs::write(&path, &pdf).unwrap();

        let started = std::time::Instant::now();
        let mut reader = PdfFileReader::open(&path).unwrap();
        assert_eq!(reader.page_count().unwrap(), 500);
        assert_eq!(reader.outline_titles().unwrap().len(), 10);
        for i in 0..500 {
            reader.read_page_image(i).unwrap();
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "500 页逐页读取耗时异常: {:?}", started.elapsed());
    }

    #[test]
    fn pdf_file_reader_page_byte_span_len_is_cheap_size_proxy() {
        // 分卷贪心分组只需要"相对大小"，不需要真读出图片内容——page_byte_span_len 纯查表，
        // 不碰磁盘；这里验证它跟真实读出来的图片字节数量级一致（差距只在对象包装的固定开销）。
        let images: Vec<PdfImage> = vec![image_from_bytes(RED_PNG).unwrap(), image_from_bytes(RED_PNG).unwrap()];
        let pdf = images_to_pdf_with_toc(&images, &[(0, "卷一".to_string())]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.pdf");
        std::fs::write(&path, &pdf).unwrap();
        let mut reader = PdfFileReader::open(&path).unwrap();
        for i in 0..2 {
            let span = reader.page_byte_span_len(i);
            let real = reader.read_page_image(i).unwrap().data.len() as u64;
            assert!(span >= real, "对象包装范围应该 >= 图片数据本身: span={span} real={real}");
            assert!(span - real < 200, "对象包装开销不该离谱地大: span={span} real={real}");
        }
    }

    #[test]
    fn pdf_file_reader_rejects_out_of_range_page() {
        let img = image_from_bytes(RED_PNG).unwrap();
        let pdf = images_to_pdf_with_toc(&[img], &[]).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.pdf");
        std::fs::write(&path, &pdf).unwrap();
        let mut reader = PdfFileReader::open(&path).unwrap();
        assert_eq!(reader.page_count().unwrap(), 1);
        assert!(reader.read_page_image(5).is_err());
    }
}
