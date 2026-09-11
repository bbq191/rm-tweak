//! 给一本已有的 EPUB **无损补封面**（2026-09-20 设备日志核查：已投的书里有的没有有效封面声明，xochitl 取不到封面缩略图）。
//!
//! 用法: cover-fix 输入.epub 输出.epub [封面缩略图.png]
//!   - 只改 OPF（`wash::ensure_cover_declared`：保证 `<meta name="cover">` 指向真图片），**其余条目原样拷贝（zip raw copy，
//!     压缩数据一个字节不动，图片零重编码）**；输出与输入同名条目顺序一致。
//!   - 第三个参数给出时，另外把封面图按 xochitl 缩略图规格（552×981 RGB PNG、白底居中等比）写出，
//!     供直接放进设备书库文档的 `<uuid>.thumbnails/cover.png`。
//!
//! 退出码: 0 成功（含"本来就有有效封面，无需改"）；1 用法错；2 失败。
use bookconv::epubzip::Entry;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 2 || a.len() > 3 {
        eprintln!("用法: cover-fix 输入.epub 输出.epub [封面缩略图.png]");
        std::process::exit(1);
    }
    if let Err(e) = run(&a[0], &a[1], a.get(2).map(String::as_str)) {
        eprintln!("失败: {e}");
        std::process::exit(2);
    }
}

fn run(input: &str, output: &str, png: Option<&str>) -> Result<(), String> {
    let mut zin = zip::ZipArchive::new(std::io::BufReader::new(std::fs::File::open(input).map_err(|e| e.to_string())?)).map_err(|e| e.to_string())?;
    // 只读非图片条目（html/opf/…），图片留空占位——封面声明检查不需要图片字节。
    let mut entries: Vec<Entry> = bookconv::epubzip::read_skeleton(&mut zin)?.entries;
    if !entries.iter().any(|e| e.name.ends_with(".opf")) {
        return Err("找不到 OPF".into());
    }
    let changed = bookconv::wash::ensure_cover_declared(&mut entries);
    let opf = entries.iter().find(|e| e.name.ends_with(".opf")).ok_or("找不到 OPF")?;
    let mut zout = zip::ZipWriter::new(std::io::BufWriter::new(std::fs::File::create(output).map_err(|e| e.to_string())?));
    for i in 0..zin.len() {
        let f = zin.by_index(i).map_err(|e| e.to_string())?;
        if changed && f.name() == opf.name {
            let o = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
            zout.start_file(f.name(), o).map_err(|e| e.to_string())?;
            std::io::Write::write_all(&mut zout, &opf.data).map_err(|e| e.to_string())?;
        } else {
            zout.raw_copy_file(f).map_err(|e| e.to_string())?; // 压缩数据原样拷贝
        }
    }
    zout.finish().map_err(|e| e.to_string())?;
    println!("封面声明: {}", if changed { "已修复（OPF 已改）" } else { "本来就有效，无需改" });
    if let Some(png_path) = png {
        let (_, bytes) = bookconv::placeholder::cover_image_of(std::path::Path::new(output)).ok_or("找不到封面图")?;
        let img = image::load_from_memory(&bytes).map_err(|e| format!("解码封面失败: {e}"))?.to_rgb8();
        let (tw, th) = (552u32, 981u32);
        let s = (tw as f32 / img.width() as f32).min(th as f32 / img.height() as f32);
        let (nw, nh) = (((img.width() as f32 * s).round() as u32).max(1), ((img.height() as f32 * s).round() as u32).max(1));
        let resized = image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3);
        let mut canvas = image::RgbImage::from_pixel(tw, th, image::Rgb([255, 255, 255]));
        image::imageops::overlay(&mut canvas, &resized, ((tw - nw) / 2) as i64, ((th - nh) / 2) as i64);
        canvas.save_with_format(png_path, image::ImageFormat::Png).map_err(|e| e.to_string())?;
        println!("封面缩略图: {png_path}（{tw}×{th}）");
    }
    Ok(())
}
