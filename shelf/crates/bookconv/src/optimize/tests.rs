//! 优化器单测（原 `optimize.rs` 内联的 `mod tests`）。
    use super::*;
    use zip::write::SimpleFileOptions;
    use zip::CompressionMethod;

    #[test]
    fn inline_remote_images_fetches_removes_and_keeps_local() {
        // 本地图不动；远程抓到→内联改本地名+进资源；远程抓不到→删 img(免放大镜)
        let html = r#"<p><img src="local.png"/><img class="c" src="https://x.com/a.png"/><img src="//y.com/b.png"/></p>"#;
        let mut n = 0usize;
        let (out, res) = inline_remote_images(html, "OEBPS", &mut n, |src| {
            if src.contains("a.png") { Some((vec![1, 2, 3], "png")) } else { None } // b 抓不到
        });
        assert!(out.contains(r#"src="local.png""#), "本地图应原样: {out}");
        assert!(out.contains(r#"src="cj_remote_0.png""#), "远程抓到应改本地名: {out}");
        assert!(out.contains(r#"class="c""#), "改 src 应保留其它属性: {out}");
        assert!(!out.contains("x.com") && !out.contains("y.com"), "远程 URL 应消失: {out}");
        assert!(!out.contains("b.png"), "抓不到的远程 img 应被删: {out}");
        assert_eq!(res.len(), 1, "只有 1 张抓到");
        assert_eq!(res[0].0, "OEBPS/cj_remote_0.png", "资源落本章目录");
        assert_eq!(res[0].1, vec![1, 2, 3]);
    }

    /// 端到端设备优化：EPUB 含超大 JPEG + 灰字 CSS + 灰字/细体内联 style，过优化器后
    /// ① 图片缩到 ≤1696px、② 灰字→纯黑、细字重→400。
    #[test]
    fn device_tuning_downscales_image_and_blackens_text() {
        use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView, RgbImage};
        // 超大 JPEG（3392×1908 = 2× 屏）
        let big = DynamicImage::ImageRgb8(RgbImage::from_fn(3392, 1908, |x, _| {
            image::Rgb([(x % 256) as u8, 100, 150])
        }));
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&big).unwrap();

        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("OEBPS/style.css", stored).unwrap();
            zw.write_all(b"body{color:#333}a{color:blue}").unwrap();
            zw.start_file("OEBPS/img/big.jpg", stored).unwrap();
            zw.write_all(&jpg).unwrap();
            zw.start_file("OEBPS/c1.xhtml", stored).unwrap();
            zw.write_all(r#"<html><body><p style="color:#666;font-weight:300">灰字</p></body></html>"#.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        let (out, _rep) = optimize_epub(&buf).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        // ① 图片缩小
        let mut ib = Vec::new();
        ar.by_name("OEBPS/img/big.jpg").unwrap().read_to_end(&mut ib).unwrap();
        let (w, h) = image::load_from_memory(&ib).unwrap().dimensions();
        assert!(w <= 1696 && h <= 1696, "图应缩到 ≤1696，实为 {w}x{h}");
        assert!(ib.len() < jpg.len(), "缩后体积应变小");
        // ② css 灰字→黑、彩色不动
        let mut css = String::new();
        ar.by_name("OEBPS/style.css").unwrap().read_to_string(&mut css).unwrap();
        assert_eq!(css, "body{color:#000000}a{color:blue}", "css 灰字→黑、彩色不动: {css}");
        // ② 内联 style 灰字→黑、细体→400
        let mut x = String::new();
        ar.by_name("OEBPS/c1.xhtml").unwrap().read_to_string(&mut x).unwrap();
        assert!(x.contains(r#"style="color:#000000;font-weight:400""#), "内联提对比: {x}");
    }

    #[test]
    fn comic_epub_gets_higher_quality_reencode_than_text_book() {
        use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView, RgbImage};
        // x、y 都要变化——否则整张图任意一列（或行）颜色恒定，会被 trim_margins 的"纯色留白"判据
        // 误判成可裁的边框，让漫画路径意外比对照组多裁一刀，干扰这条测试本身要验证的"质量差异"。
        let big = DynamicImage::ImageRgb8(RgbImage::from_fn(2000, 3000, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 150])));
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&big).unwrap();

        // 漫画书：25 张纯图片页（其中一张是超框大图）
        let mut comic_buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut comic_buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            let items: String = (1..=25).map(|i| format!(r#"<item id="c{i}" href="c{i}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
            let spine: String = (1..=25).map(|i| format!(r#"<itemref idref="c{i}"/>"#)).collect();
            zw.start_file("content.opf", stored).unwrap();
            zw.write_all(format!(r#"<package version="3.0"><metadata><dc:title>漫画</dc:title></metadata><manifest>{items}</manifest><spine>{spine}</spine></package>"#).as_bytes()).unwrap();
            for i in 1..=25 {
                zw.start_file(format!("c{i}.xhtml"), stored).unwrap();
                zw.write_all(format!(r#"<html><body><img src="p{i}.jpg"/></body></html>"#).as_bytes()).unwrap();
            }
            zw.start_file("p1.jpg", stored).unwrap();
            zw.write_all(&jpg).unwrap();
            zw.finish().unwrap();
        }
        // 文字书：同一张超框大图，但正文是长文字（不判漫画）
        let long_text = "正".repeat(500);
        let mut text_buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut text_buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("content.opf", stored).unwrap();
            zw.write_all(r#"<package version="3.0"><metadata><dc:title>文字书</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#.as_bytes()).unwrap();
            zw.start_file("c1.xhtml", stored).unwrap();
            zw.write_all(format!("<html><body><p>{long_text}</p><img src=\"p1.jpg\"/></body></html>").as_bytes()).unwrap();
            zw.start_file("p1.jpg", stored).unwrap();
            zw.write_all(&jpg).unwrap();
            zw.finish().unwrap();
        }

        let (comic_out, _) = optimize_epub(&comic_buf).unwrap();
        let (text_out, _) = optimize_epub(&text_buf).unwrap();
        let mut comic_img = Vec::new();
        ZipArchive::new(Cursor::new(&comic_out)).unwrap().by_name("p1.jpg").unwrap().read_to_end(&mut comic_img).unwrap();
        let mut text_img = Vec::new();
        ZipArchive::new(Cursor::new(&text_out)).unwrap().by_name("p1.jpg").unwrap().read_to_end(&mut text_img).unwrap();

        // 2026-09-19 起两边不再要求尺寸完全一致：漫画路径多了 `pad_to_device_aspect` 这一步
        // （真机反馈"底部留白太多"，CSS 治不了；用户明确目标是"上下留白尽量等比、左右留白尽可能
        // 接近 0"，见该函数文档），文字书路径的内嵌图不是整页漫画，不需要补白。这里只保留还站得
        // 住脚的部分：两边都应该被缩进屏幕框内（不超限），漫画路径的高度应该补到刚好等于设备
        // 页面长宽比对应的高度（宽度不变，左右留白全程是 0）。
        let (cw, ch) = image::load_from_memory(&comic_img).unwrap().dimensions();
        let (tw, th) = image::load_from_memory(&text_img).unwrap().dimensions();
        assert!(cw <= crate::imgopt::MAX_SHORT_EDGE && ch <= crate::imgopt::MAX_EDGE, "漫画路径应该缩进屏幕框: {cw}x{ch}");
        assert!(tw <= crate::imgopt::MAX_SHORT_EDGE && th <= crate::imgopt::MAX_EDGE, "文字书内嵌图也应该缩进屏幕框: {tw}x{th}");
        assert_eq!((cw, ch), (crate::imgopt::MAX_SHORT_EDGE, crate::imgopt::MAX_EDGE), "缺省（开关关）漫画整页补白到屏幕比例 954×1696: {cw}x{ch}");
        // 开关开（MinMargin）：补白到 xochitl 图片框比例（配阅读器页边距 1）
        let mm = OptimizeOpts { comic_frame: crate::imgopt::EpubComicFrame::MinMargin, ..Default::default() }; // 与缺省 optimize_epub 只差页框模式
        let (mm_out, _) = optimize_epub_with(&comic_buf, &mm).unwrap();
        let mut mm_img = Vec::new();
        ZipArchive::new(Cursor::new(&mm_out)).unwrap().by_name("p1.jpg").unwrap().read_to_end(&mut mm_img).unwrap();
        assert_eq!(image::load_from_memory(&mm_img).unwrap().dimensions(), (crate::imgopt::MAX_SHORT_EDGE, crate::imgopt::EPUB_COMIC_PAGE_H), "MinMargin 模式补白到图片框比例");
        // 文字书不受开关影响（不是漫画）
        let (tm_out, _) = optimize_epub_with(&text_buf, &mm).unwrap();
        let mut tm_img = Vec::new();
        ZipArchive::new(Cursor::new(&tm_out)).unwrap().by_name("p1.jpg").unwrap().read_to_end(&mut tm_img).unwrap();
        assert_eq!(tm_img, text_img, "文字书内嵌图不受漫画页框开关影响");
        assert!(comic_img.len() > text_img.len(), "漫画书判定应触发更高质量重编码，体积应更大: comic={} text={}", comic_img.len(), text_img.len());
    }

    #[test]
    fn streaming_matches_in_memory_output_for_footnote_book() {
        // 内存版跟流式版共用 first_pass_html/transform_html_chapter，这条测试证明两条路径对同一本
        // 带跨文件脚注的书产出一致的正文——不是"抽了函数就当一样"，是真跑两条路径对拍。
        let epub = make_crossfile_endnote_epub();
        let (mem_out, mem_rep) = optimize_epub(&epub).unwrap();

        let t = tempfile::tempdir().unwrap();
        let input_path = t.path().join("in.epub");
        let output_path = t.path().join("out.epub");
        std::fs::write(&input_path, &epub).unwrap();
        let mut progresses = Vec::new();
        let stream_rep = optimize_epub_file_streaming(&input_path, &output_path, &OptimizeOpts::default(), |done, total| progresses.push((done, total))).unwrap();
        let stream_out = std::fs::read(&output_path).unwrap();

        assert_eq!(mem_rep.total_files, stream_rep.total_files);
        assert_eq!(mem_rep.html_files, stream_rep.html_files);
        assert!(!progresses.is_empty(), "阶段二应该至少回调一次进度");
        assert!(progresses.iter().all(|(_, total)| *total == progresses[0].1), "total 全程不变");
        assert_eq!(progresses.last().unwrap().0, progresses[0].1, "最后一次回调 done 应该等于 total（全部写完）");
        assert!(progresses.windows(2).all(|w| w[0].0 < w[1].0), "done 应该严格递增，不重复不倒退");

        let read = |bytes: &[u8], n: &str| {
            let mut ar = ZipArchive::new(Cursor::new(bytes)).unwrap();
            let mut s = String::new();
            ar.by_name(n).unwrap().read_to_string(&mut s).unwrap();
            s
        };
        for name in ["ch1.xhtml", "ch2.xhtml"] {
            let mem_ch = read(&mem_out, name);
            let stream_ch = read(&stream_out, name);
            assert_eq!(mem_ch, stream_ch, "{name} 内存版跟流式版应产出完全一致的正文");
        }
    }

    #[test]
    fn streaming_downscales_comic_images_same_as_in_memory() {
        // 内存版跟流式版共用 transform_image_bytes，图片处理结果应该逐字节一致——流式版的差别只在
        // "什么时候、从哪读图片字节"，不该影响处理结果本身。
        use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView, RgbImage};
        let big = DynamicImage::ImageRgb8(RgbImage::from_fn(2000, 3000, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 150])));
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&big).unwrap();

        let mut comic_buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut comic_buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            let items: String = (1..=25).map(|i| format!(r#"<item id="c{i}" href="c{i}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
            let spine: String = (1..=25).map(|i| format!(r#"<itemref idref="c{i}"/>"#)).collect();
            zw.start_file("content.opf", stored).unwrap();
            zw.write_all(format!(r#"<package version="3.0"><metadata><dc:title>漫画</dc:title></metadata><manifest>{items}</manifest><spine>{spine}</spine></package>"#).as_bytes()).unwrap();
            for i in 1..=25 {
                zw.start_file(format!("c{i}.xhtml"), stored).unwrap();
                zw.write_all(format!(r#"<html><body><img src="p{i}.jpg"/></body></html>"#).as_bytes()).unwrap();
            }
            zw.start_file("p1.jpg", stored).unwrap();
            zw.write_all(&jpg).unwrap();
            zw.finish().unwrap();
        }

        let (mem_out, _) = optimize_epub(&comic_buf).unwrap();
        let t = tempfile::tempdir().unwrap();
        let input_path = t.path().join("comic.epub");
        let output_path = t.path().join("comic_out.epub");
        std::fs::write(&input_path, &comic_buf).unwrap();
        optimize_epub_file_streaming(&input_path, &output_path, &OptimizeOpts::default(), |_, _| {}).unwrap();
        let stream_out = std::fs::read(&output_path).unwrap();

        let mut mem_img = Vec::new();
        ZipArchive::new(Cursor::new(&mem_out)).unwrap().by_name("p1.jpg").unwrap().read_to_end(&mut mem_img).unwrap();
        let mut stream_img = Vec::new();
        ZipArchive::new(Cursor::new(&stream_out)).unwrap().by_name("p1.jpg").unwrap().read_to_end(&mut stream_img).unwrap();
        assert_eq!(mem_img, stream_img, "同一张图内存版跟流式版处理结果应逐字节一致");
        assert!(image::load_from_memory(&stream_img).unwrap().dimensions().0 <= 954, "流式版也该按漫画框约束缩放");
    }

    #[test]
    fn streaming_parallel_many_images_match_sequential_in_memory_and_keep_order() {
        // 并行（worker + 提前量）不许乱序、不许改任何一张图的处理结果：20 张互不相同的图（尺寸/内容都不同，
        // 顺序错位或串图必然被发现），流式并行版逐张、逐字节对照内存顺序版；条目顺序也必须一致。
        use image::{DynamicImage, RgbImage};
        let n = 20usize; // ≥20 张才判漫画，走漫画单趟管线
        let mut comic_buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut comic_buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            let items: String = (1..=n).map(|i| format!(r#"<item id="c{i}" href="c{i}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
            let spine: String = (1..=n).map(|i| format!(r#"<itemref idref="c{i}"/>"#)).collect();
            zw.start_file("content.opf", stored).unwrap();
            zw.write_all(format!(r#"<package version="3.0"><metadata><dc:title>漫画</dc:title></metadata><manifest>{items}</manifest><spine>{spine}</spine></package>"#).as_bytes()).unwrap();
            for i in 1..=n {
                zw.start_file(format!("c{i}.xhtml"), stored).unwrap();
                zw.write_all(format!(r#"<html><body><img src="p{i}.png"/></body></html>"#).as_bytes()).unwrap();
                let (w, h) = (320 + (i as u32 * 37) % 30, 480 + (i as u32 * 91) % 40); // 短边≥318 才走补白路径；debug 构建下 SIMD 缩放很慢，图要小
                let img = DynamicImage::ImageRgb8(RgbImage::from_fn(w, h, |x, y| image::Rgb([((x + i as u32 * 13) % 256) as u8, ((y * 3) % 256) as u8, (i * 8 % 256) as u8])));
                // PNG：不预放大（无损放大体积暴涨），只补白到设备长宽比——路径真实、debug 构建下也够快。
                let mut jpg = Vec::new();
                img.write_to(&mut Cursor::new(&mut jpg), image::ImageFormat::Png).unwrap();
                zw.start_file(format!("p{i}.png"), stored).unwrap();
                zw.write_all(&jpg).unwrap();
            }
            zw.finish().unwrap();
        }
        let (mem_out, _) = optimize_epub(&comic_buf).unwrap();
        let t = tempfile::tempdir().unwrap();
        let (input_path, output_path) = (t.path().join("c.epub"), t.path().join("o.epub"));
        std::fs::write(&input_path, &comic_buf).unwrap();
        optimize_epub_file_streaming(&input_path, &output_path, &OptimizeOpts::default(), |_, _| {}).unwrap();
        let stream_out = std::fs::read(&output_path).unwrap();
        let (mut a, mut b) = (ZipArchive::new(Cursor::new(&mem_out)).unwrap(), ZipArchive::new(Cursor::new(&stream_out)).unwrap());
        for i in 1..=n {
            let name = format!("p{i}.png");
            let (mut x, mut y) = (Vec::new(), Vec::new());
            a.by_name(&name).unwrap().read_to_end(&mut x).unwrap();
            b.by_name(&name).unwrap().read_to_end(&mut y).unwrap();
            assert_eq!(x, y, "第 {i} 张图并行结果与顺序结果不一致（乱序或串图）");
        }
        let mut y1 = Vec::new();
        b.by_name("p1.png").unwrap().read_to_end(&mut y1).unwrap();
        let (w1, h1) = image::ImageReader::new(Cursor::new(&y1)).with_guessed_format().unwrap().into_dimensions().unwrap();
        assert!(h1 > 491 && w1 == 327, "应真的补白到设备长宽比（管线确实跑过）: {w1}x{h1}");
        let order = |z: &mut ZipArchive<Cursor<&Vec<u8>>>| -> Vec<String> { (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_string()).filter(|n| n != OPTIMIZE_MARKER).collect() };
        assert_eq!(order(&mut a), order(&mut b), "条目顺序必须一致");
    }

    #[test]
    fn streaming_cancel_stops_early_with_cancelled_error() {
        // 取消回调返回 true：应立刻以 CANCELLED_MSG 失败，不产出完整文件；worker 线程要能干净退出（不死锁）。
        let t = tempfile::tempdir().unwrap();
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("content.opf", stored).unwrap();
            zw.write_all(br#"<package version="3.0"><metadata><dc:title>t</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#).unwrap();
            zw.start_file("c1.xhtml", stored).unwrap();
            zw.write_all(b"<html><body><p>hi</p></body></html>").unwrap();
            zw.finish().unwrap();
        }
        let (input, output) = (t.path().join("a.epub"), t.path().join("o.epub"));
        std::fs::write(&input, &buf).unwrap();
        let calls = std::cell::Cell::new(0);
        let err = optimize_epub_file_streaming_ctl(&input, &output, &OptimizeOpts::default(), None, &|| { calls.set(calls.get() + 1); calls.get() >= 2 }, |_, _| {}).unwrap_err();
        assert_eq!(err, CANCELLED_MSG);
    }

    #[test]
    fn streaming_rejects_missing_input_and_leaves_no_partial_output() {
        let t = tempfile::tempdir().unwrap();
        let output_path = t.path().join("out.epub");
        let err = optimize_epub_file_streaming(&t.path().join("does-not-exist.epub"), &output_path, &OptimizeOpts::default(), |_, _| {}).unwrap_err();
        assert!(err.contains("打开输入失败"), "{err}");
        assert!(!output_path.exists(), "输入都打不开，不该产生任何输出文件");
    }

    #[test]
    fn streaming_builder_equals_legacy_wrappers_and_honors_cancel() {
        let t = tempfile::tempdir().unwrap();
        let input = t.path().join("in.epub");
        std::fs::write(&input, make_epub()).unwrap();
        let opts = OptimizeOpts::default();
        let (a, b) = (t.path().join("a.epub"), t.path().join("b.epub"));
        optimize_epub_file_streaming(&input, &a, &opts, |_, _| {}).unwrap();
        StreamingOptimize::new(&input, &b, &opts).run(|_, _| {}).unwrap();
        assert_eq!(std::fs::read(&a).unwrap(), std::fs::read(&b).unwrap(), "builder 与旧封装产出逐字节相同");
        // 取消：第一个条目处理完就停
        let c = t.path().join("c.epub");
        let err = StreamingOptimize::new(&input, &c, &opts).cancel(&|| true).run(|_, _| {}).unwrap_err();
        assert_eq!(err, CANCELLED_MSG);
    }

    /// 造一个最小 EPUB(mimetype + 一章带锁字体的 xhtml)，过优化器后字体锁应被剥掉、结构保留。
    fn make_epub() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("EPUB/css/x.css", stored).unwrap();
            zw.write_all(b"body{margin:0}").unwrap();
            zw.start_file("EPUB/xhtml/Section01.xhtml", stored).unwrap();
            zw.write_all(
                r#"<html><body><p style="font-size:16px;font-family:'PingFang SC';">正文</p></body></html>"#.as_bytes(),
            )
            .unwrap();
            zw.finish().unwrap();
        }
        buf
    }

    #[test]
    fn strips_font_keeps_structure() {
        let (out, rep) = optimize_epub(&make_epub()).unwrap();
        assert_eq!(rep.html_files, 1);
        // 重新解开验证
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        // mimetype 首个
        assert_eq!(ar.by_index(0).unwrap().name(), "mimetype");
        // css 原样保留
        let mut css = String::new();
        ar.by_name("EPUB/css/x.css").unwrap().read_to_string(&mut css).unwrap();
        assert_eq!(css, "body{margin:0}");
        // xhtml 字体锁被剥
        let mut x = String::new();
        ar.by_name("EPUB/xhtml/Section01.xhtml").unwrap().read_to_string(&mut x).unwrap();
        assert!(!x.contains("font-family"), "字体锁未剥: {x}");
        assert!(x.contains("<p>正文</p>"), "正文结构被破坏: {x}");
    }

    /// 真机《甲午：摇摆的战争》坐实的真实形态：章节文件**没有扩展名**（`Chapter_2`/`Chapter_7_1`
    /// 这种命名），只按扩展名判断的 `is_html` 会把它整个漏过、字体锁/脚注图标全都没清洗——用户反馈
    /// "字体锁死改不了"。这条端到端跑一遍优化器，断言无扩展名的章节也真的被处理了。
    #[test]
    fn strips_font_lock_on_extensionless_chapter_file() {
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("EPUB/xhtml/Chapter_2", stored).unwrap();
            zw.write_all(r#"<?xml version="1.0"?><html><body><p style="font-size:16px;font-family:'PingFang SC';">正文</p></body></html>"#.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        let (out, rep) = optimize_epub(&buf).unwrap();
        assert_eq!(rep.html_files, 1, "无扩展名的章节也该被数进 html_files: {:?}", rep);
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        let mut x = String::new();
        ar.by_name("EPUB/xhtml/Chapter_2").unwrap().read_to_string(&mut x).unwrap();
        assert!(!x.contains("font-family"), "无扩展名章节的字体锁未剥: {x}");
        assert!(x.contains("<p>正文</p>"), "正文结构被破坏: {x}");
    }

    /// Calibre `wash_epub.sh` 洗过的 duokan 脚注（《人骨拼图》AZW3→EPUB 真实形态）：同文件 href 带文件名、
    /// 标记是真 `<img>` + `<a id="c_2_1">`、注释块 `<li id="a_2_1">` 内回链 `href="part0004.html#c_2_1"`
    /// 构成真 2-环。优化后：href 归一裸锚、标记换上标且 id 保留、回链去链、注释留在原 li 里不被搬成
    /// 无效嵌套（v4 前两条红线全踩：id 丢=回链悬空、`<p id><p>` 嵌套）。
    #[test]
    fn calibre_washed_duokan_footnote_survives() {
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("text/part0004.html", stored).unwrap();
            zw.write_all(r##"<html><body><p class="a">退休储蓄基金<sup class="calibre4"><a class="duokan-footnote" href="part0004.html#a_2_1" id="c_2_1"><img alt="注释1" class="duokan-footnote1" src="../images/00003.png"/></a></sup>，她可以</p>
<ol class="duokan-footnote-content">
<li class="duokan-footnote-item" id="a_2_1">
<p class="pfootnotetext"><a class="calibre6" href="part0004.html#c_2_1">美国一项延税储蓄计划。</a></p>
</li>
</ol></body></html>"##.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        let (out, _) = optimize_epub(&buf).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        let mut x = String::new();
        ar.by_name("text/part0004.html").unwrap().read_to_string(&mut x).unwrap();
        assert!(!x.contains("part0004.html#"), "同文件 href 未归一裸锚: {x}");
        assert!(x.contains(r##"<a href="#a_2_1" id="c_2_1"><sup>1</sup></a>"##), "标记未换上标/丢 id: {x}");
        assert!(!x.contains("<img"), "标记死图未清: {x}");
        assert!(!x.contains(r##"href="#c_2_1""##), "回链未去链(互指对整对丢弃): {x}");
        assert!(x.contains("延税储蓄计划"), "注释文本丢失: {x}");
        assert!(x.contains(r##"<li class="duokan-footnote-item" id="a_2_1">"##), "注释块应原地留在 li 里: {x}");
        assert!(!x.contains("<div class=\"footnotes\">"), "同章注释不该被当跨文件搬走: {x}");
        assert!(!x.contains("<p id=\"a_2_1\">\n"), "不该产出无效嵌套 <p id><p>: {x}");
    }

    /// 造一本"正文章引用书末尾注文件"的 EPUB（bug 复现形态）：两章各引用 notes.xhtml 里的一条
    /// 普通尾注（<a href="notes.xhtml#nX">，非 noteref；注释为 <p id="nX">）。优化后每章 marker 应变
    /// 同章锚点、注释搬进对应章，且两章共用形态不撞 id。
    fn make_crossfile_endnote_epub() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("ch1.xhtml", stored).unwrap();
            zw.write_all(r#"<html><body><p>第一章正文<a href="notes.xhtml#n1">1</a>结束</p></body></html>"#.as_bytes()).unwrap();
            zw.start_file("ch2.xhtml", stored).unwrap();
            zw.write_all(r#"<html><body><p>第二章正文<a href="notes.xhtml#n2">1</a>结束</p></body></html>"#.as_bytes()).unwrap();
            zw.start_file("notes.xhtml", stored).unwrap();
            zw.write_all(r#"<html><body><p class="footnote" id="n1">第一章的注释</p><p class="footnote" id="n2">第二章的注释</p></body></html>"#.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        buf
    }

    /// 带真实 content.opf 的最小书：spine 里第一页是一份「目录页」（链到一堆不同章节文件，形状
    /// 跟 Calibre 常见排版一致），后面跟几章正文。
    fn make_epub_with_html_toc_page(n_chapters: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            let links: String = (1..=n_chapters).map(|i| format!(r#"<li><a href="c{i}.xhtml">第{i}章</a></li>"#)).collect();
            zw.start_file("OEBPS/toc.html", stored).unwrap();
            zw.write_all(format!(r#"<html><body><h1>目录</h1><ul>{links}</ul></body></html>"#).as_bytes()).unwrap();
            for i in 1..=n_chapters {
                zw.start_file(format!("OEBPS/c{i}.xhtml"), stored).unwrap();
                zw.write_all(format!("<html><body><p>第{i}章正文</p></body></html>").as_bytes()).unwrap();
            }
            let manifest_chapters: String = (1..=n_chapters).map(|i| format!(r#"<item id="c{i}" href="c{i}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
            let spine_chapters: String = (1..=n_chapters).map(|i| format!(r#"<itemref idref="c{i}"/>"#)).collect();
            zw.start_file("OEBPS/content.opf", stored).unwrap();
            zw.write_all(
                format!(
                    r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="toc-html" href="toc.html" media-type="application/xhtml+xml"/>{manifest_chapters}</manifest><spine><itemref idref="toc-html"/>{spine_chapters}</spine></package>"#
                )
                .as_bytes(),
            )
            .unwrap();
            zw.finish().unwrap();
        }
        buf
    }

    /// 母版库按书指定翻页方向（2026-09-25）：`page_direction=Some` 只改 OPF 的 spine 属性，其余条目与不指定时逐字节相同；
    /// 不指定＝保留原书（原书没写就还是没写）。内存版与流式版一致。
    #[test]
    fn page_direction_touches_only_opf_spine() {
        use crate::direction::{spine_direction, PageDirection};
        let epub = make_epub_with_html_toc_page(3);
        let entries = |bytes: &[u8]| {
            let mut ar = ZipArchive::new(Cursor::new(bytes)).unwrap();
            (0..ar.len())
                .map(|i| {
                    let mut f = ar.by_index(i).unwrap();
                    let mut v = Vec::new();
                    f.read_to_end(&mut v).unwrap();
                    (f.name().to_string(), v)
                })
                .collect::<Vec<_>>()
        };
        let base = OptimizeOpts { wash: Some(crate::wash::WashOpts::default()), ..Default::default() };
        let rtl = OptimizeOpts { page_direction: Some(PageDirection::Rtl), ..base.clone() };
        let (plain, _) = optimize_epub_with(&epub, &base).unwrap();
        let (flipped, _) = optimize_epub_with(&epub, &rtl).unwrap();
        let (a, b) = (entries(&plain), entries(&flipped));
        assert_eq!(a.len(), b.len());
        for ((na, da), (nb, db)) in a.iter().zip(&b) {
            assert_eq!(na, nb, "条目顺序不变");
            let (sa, sb) = (String::from_utf8_lossy(da), String::from_utf8_lossy(db));
            if na.ends_with(".opf") {
                assert_eq!(spine_direction(&sa), None, "不指定＝保留原书（原书没写）: {sa}");
                assert_eq!(spine_direction(&sb), Some(PageDirection::Rtl), "{sb}");
                assert_eq!(sb.replacen(r#" page-progression-direction="rtl""#, "", 1), sa, "OPF 只多这一个属性");
            } else {
                assert_eq!(da, db, "{na} 不该受方向设置影响");
            }
        }
        // 流式版产出同样的 OPF
        let t = tempfile::tempdir().unwrap();
        let (input, output) = (t.path().join("in.epub"), t.path().join("out.epub"));
        std::fs::write(&input, &epub).unwrap();
        StreamingOptimize::new(&input, &output, &rtl).run(|_, _| {}).unwrap();
        assert_eq!(crate::direction::spine_direction_file(&output), None, "测试书没有 container.xml，按文件读不到 OPF");
        let opf_of = |v: &[(String, Vec<u8>)]| v.iter().find(|(n, _)| n.ends_with(".opf")).map(|(_, d)| d.clone()).unwrap();
        assert_eq!(opf_of(&entries(&std::fs::read(&output).unwrap())), opf_of(&b), "流式与内存版 OPF 一致");
        // 从左往右：原书的 rtl 被改掉
        let ltr = OptimizeOpts { page_direction: Some(PageDirection::Ltr), ..base.clone() };
        let (back, _) = optimize_epub_with(&flipped, &ltr).unwrap();
        let opf = String::from_utf8(opf_of(&entries(&back))).unwrap();
        assert_eq!(spine_direction(&opf), Some(PageDirection::Ltr), "{opf}");
    }

    #[test]
    fn html_toc_page_kept_in_spine_not_stripped_as_redundant() {
        // 真机回归（2026-09-19，《疯探》）：书自带的 HTML 目录页（链到几十个章节文件）曾被旧逻辑当
        // "跟原生 TOC 冗余"从 spine 删掉，翻页再也看不到目录——违背 EPUB 线原则①"保留目录页"。
        let (out, _) = optimize_epub(&make_epub_with_html_toc_page(20)).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        let mut opf = String::new();
        ar.by_name("OEBPS/content.opf").unwrap().read_to_string(&mut opf).unwrap();
        assert!(opf.contains(r#"idref="toc-html""#), "目录页的 itemref 不该从 spine 被删: {opf}");
    }

    #[test]
    fn crossfile_endnotes_relinked_per_chapter() {
        let (out, _) = optimize_epub(&make_crossfile_endnote_epub()).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        let read = |ar: &mut ZipArchive<Cursor<&Vec<u8>>>, n: &str| {
            let mut s = String::new();
            ar.by_name(n).unwrap().read_to_string(&mut s).unwrap();
            s
        };
        let ch1 = read(&mut ar, "ch1.xhtml");
        assert!(ch1.contains(r##"<a href="#n1">1</a>"##), "ch1 marker 未改同章锚点: {ch1}");
        assert!(ch1.contains("第一章的注释"), "ch1 未搬入其注释: {ch1}");
        assert!(!ch1.contains("notes.xhtml"), "ch1 仍残留跨文件 href: {ch1}");
        let ch2 = read(&mut ar, "ch2.xhtml");
        assert!(ch2.contains("第二章的注释"), "ch2(中间章)未搬入其注释: {ch2}");
        assert!(ch2.contains(r##"<a href="#n2">1</a>"##), "ch2 marker 未改同章锚点: {ch2}");
        // 注释已从 notes.xhtml 移走（不重复渲染）
        let notes = read(&mut ar, "notes.xhtml");
        assert!(!notes.contains("第一章的注释") && !notes.contains("第二章的注释"), "注释未从源文件移除: {notes}");
    }

    #[test]
    fn double_optimize_inline_footnote_no_dup() {
        // 版本升级会重优化已优化过的旧书——重优化不得把已内联的注释再翻倍。
        let opts = OptimizeOpts { wash: Some(crate::wash::WashOpts::default()), footnote: FootnoteMode::Inline, ..Default::default() };
        let (out, _) = optimize_epub_with(&make_crossfile_endnote_epub(), &opts).unwrap();
        let (out2, _) = optimize_epub_with(&out, &opts).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out2)).unwrap();
        let mut ch1 = String::new();
        ar.by_name("ch1.xhtml").unwrap().read_to_string(&mut ch1).unwrap();
        let n = ch1.matches("第一章的注释").count();
        assert_eq!(n, 1, "重优化后注释重复 {n} 次: {ch1}");
    }

    #[test]
    fn reoptimize_relinked_footnote_no_dup() {
        // 模拟旧版本(v6/v7)产物：注释已移同章末尾 <div class="footnotes"> + marker 已是同章锚点。
        // 版本升级重优化这类书时，不得把注释再翻倍。
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("ch1.xhtml", stored).unwrap();
            zw.write_all(r##"<html><body><p>正文<a href="#n1">1</a>结束</p><div class="footnotes"><p id="n1">第一章的注释</p></div></body></html>"##.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        let opts = OptimizeOpts { wash: Some(crate::wash::WashOpts::default()), footnote: FootnoteMode::Inline, ..Default::default() };
        let (out, _) = optimize_epub_with(&buf, &opts).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        let mut ch1 = String::new();
        ar.by_name("ch1.xhtml").unwrap().read_to_string(&mut ch1).unwrap();
        let n = ch1.matches("第一章的注释").count();
        assert_eq!(n, 1, "重优化旧版脚注结构翻倍 {n} 次: {ch1}");
    }

    #[test]
    fn marks_and_detects_optimized() {
        let raw = make_epub();
        assert!(!is_optimized(&raw), "原始 EPUB 不该带标记");
        // 默认 optimize_epub 无清洗层 → 只算"核心遍"标记，不能冒充完整优化
        let (out, _) = optimize_epub(&raw).unwrap();
        assert_eq!(optimized_version(&out).as_deref(), Some(format!("{OPTIMIZE_VERSION}-core").as_str()), "无 wash 应标 -core");
        assert!(is_optimized(&out) && optimized_version(&out).as_deref() != Some(OPTIMIZE_VERSION), "有标记但不算当前完整优化");
        // 带清洗层 → 完整标记
        let (full, _) = optimize_epub_with(&raw, &OptimizeOpts { wash: Some(crate::wash::WashOpts::default()), footnote: FootnoteMode::Anchor, ..Default::default() }).unwrap();
        assert_eq!(optimized_version(&full).as_deref(), Some(OPTIMIZE_VERSION), "含 wash 应标完整版本");
        // 重优化幂等：标记只有一条(不残留旧标记)、版本仍正确
        let (out2, _) = optimize_epub(&out).unwrap();
        assert_eq!(optimized_version(&out2).as_deref(), Some(format!("{OPTIMIZE_VERSION}-core").as_str()));
        let mut ar = ZipArchive::new(Cursor::new(&out2)).unwrap();
        let marker_count = (0..ar.len())
            .filter(|&i| ar.by_index(i).unwrap().name() == OPTIMIZE_MARKER)
            .count();
        assert_eq!(marker_count, 1, "重优化不应残留重复标记条目");
    }

/// 漫画 + 页边距最小化页框：纯文字页（版权/章节标题）补留边类与 css 规则，图片页/图文混排页不动；缺省页框不动；
/// 登记判据 `is_min_margin_comic_file` 只放行"文字页已留边"的产物。
#[test]
fn comic_min_margin_pads_only_pure_text_pages() {
    use image::{codecs::jpeg::JpegEncoder, DynamicImage, GrayImage};
    let mut jpg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpg, 85)
        .encode_image(&DynamicImage::ImageLuma8(GrayImage::from_fn(60, 90, |x, y| image::Luma([((x * 3 + y * 5) % 200) as u8 + 20])))).unwrap();
    let mut buf = Vec::new();
    {
        let mut zw = ZipWriter::new(Cursor::new(&mut buf));
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        zw.start_file("mimetype", stored).unwrap();
        zw.write_all(b"application/epub+zip").unwrap();
        let mut ids: Vec<String> = vec!["t1".into()];
        ids.extend((1..=22).map(|i| format!("c{i}")));
        ids.push("m1".into());
        let items: String = ids.iter().map(|id| format!(r#"<item id="{id}" href="{id}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
        let spine: String = ids.iter().map(|id| format!(r#"<itemref idref="{id}"/>"#)).collect();
        zw.start_file("content.opf", stored).unwrap();
        zw.write_all(format!(r#"<package version="3.0"><metadata><dc:title>漫画</dc:title></metadata><manifest>{items}</manifest><spine>{spine}</spine></package>"#).as_bytes()).unwrap();
        zw.start_file("t1.xhtml", stored).unwrap();
        zw.write_all(r#"<html><head><title>版权</title></head><body class="calibre2"><p>版权信息 COPYRIGHT 书名：某漫画</p></body></html>"#.as_bytes()).unwrap();
        for i in 1..=22 {
            zw.start_file(format!("c{i}.xhtml"), stored).unwrap();
            zw.write_all(format!(r#"<html><head><title>p</title></head><body class="calibre2"><img src="p{i}.jpg"/></body></html>"#).as_bytes()).unwrap();
            zw.start_file(format!("p{i}.jpg"), stored).unwrap();
            zw.write_all(&jpg).unwrap();
        }
        zw.start_file("m1.xhtml", stored).unwrap();
        zw.write_all(r#"<html><head><title>m</title></head><body class="calibre2"><h2 class="calibre14">特别附录</h2><img src="p1.jpg"/><p class="calibre7">阿塔……</p></body></html>"#.as_bytes()).unwrap();
        zw.finish().unwrap();
    }
    let read = |bytes: &[u8], n: &str| {
        let mut s = String::new();
        ZipArchive::new(Cursor::new(bytes)).unwrap().by_name(n).unwrap().read_to_string(&mut s).unwrap();
        s
    };
    let mm = OptimizeOpts { wash: Some(crate::wash::WashOpts::default()), comic_frame: crate::imgopt::EpubComicFrame::MinMargin, ..Default::default() };
    let (out, _) = optimize_epub_with(&buf, &mm).unwrap();
    assert!(read(&out, "t1.xhtml").contains(r#"class="calibre2 cj-tp""#), "纯文字页保留原类并追加留边类: {}", read(&out, "t1.xhtml"));
    assert!(!read(&out, "c1.xhtml").contains("cj-tp"), "图片页不加留边类");
    assert!(!read(&out, "m1.xhtml").contains("cj-tp"), "图文混排页不加留边类");
    // 含图页去掉 body class（Calibre 的 body 类会让图片在边距 1 下被吃掉约 20pt，真机诊断 zz-ip3）
    assert!(!read(&out, "c1.xhtml").contains("class=\"calibre2\""), "图片页 body 类应去掉: {}", read(&out, "c1.xhtml"));
    assert!(!read(&out, "m1.xhtml").contains("class=\"calibre2\""), "混排页 body 类也去掉");
    // 混排页的文字元素追加 cj-tx（保留原类），css 有带元素名的规则
    let m1 = read(&out, "m1.xhtml");
    // 清洗层会把"标题后首段"的 <p> 改写成 <div class="cj-flush">，两种形态都要带上 cj-tx
    assert!(m1.contains(r#"<h2 class="calibre14 cj-tx""#), "混排页 h2 应带 cj-tx: {m1}");
    assert!(m1.contains(r#"<p class="calibre7 cj-tx">"#) || m1.contains(r#"<div class="cj-flush cj-tx">"#), "混排页说明段应带 cj-tx: {m1}");
    assert!(!read(&out, "c1.xhtml").contains("cj-tx"), "纯图片页无 cj-tx");
    assert!(read(&out, "cangjie-wash.css").contains("p.cj-tx{margin-left:17.8pt;margin-right:17.8pt;}"), "css 有带元素名的规则");
    let css = read(&out, "cangjie-wash.css");
    assert_eq!(css.matches(crate::comic_pad::TEXT_PAGE_CSS_RULE).count(), 1, "wash css 应含留边规则一次: {css}");

    // 幂等：对产物再优化一遍，类与规则不重复
    let (out2, _) = optimize_epub_with(&out, &mm).unwrap();
    assert_eq!(read(&out2, "t1.xhtml").matches("cj-tp").count(), 1, "重复优化不重复加类");
    assert_eq!(read(&out2, "cangjie-wash.css").matches(crate::comic_pad::TEXT_PAGE_CSS_RULE).count(), 1, "重复优化不重复写规则");

    // 缺省页框（开关关）：不动文字页
    let screen = OptimizeOpts { wash: Some(crate::wash::WashOpts::default()), ..Default::default() };
    let (out_s, _) = optimize_epub_with(&buf, &screen).unwrap();
    assert!(!read(&out_s, "t1.xhtml").contains("cj-tp"), "缺省页框不加留边类");
    assert!(!read(&out_s, "m1.xhtml").contains("cj-tx"), "缺省页框不给混排页加 cj-tx");
    assert!(read(&out_s, "c1.xhtml").contains("class=\"calibre2\""), "缺省页框不动图片页的 body 类");
    assert!(!read(&out_s, "cangjie-wash.css").contains(".cj-tp"), "缺省页框不写规则");

    // 登记判据：MinMargin 产物放行；缺省页框产物（有文字页却没留边）拒绝
    let t = tempfile::tempdir().unwrap();
    let (p_mm, p_s) = (t.path().join("mm.epub"), t.path().join("screen.epub"));
    std::fs::write(&p_mm, &out).unwrap();
    std::fs::write(&p_s, &out_s).unwrap();
    assert!(crate::comic_detect::is_min_margin_comic_file(&p_mm), "文字页已留边的漫画应放行");
    assert!(!crate::comic_detect::is_min_margin_comic_file(&p_s), "文字页没留边的旧产物不放行（页边距 1 下会贴边）");
}
