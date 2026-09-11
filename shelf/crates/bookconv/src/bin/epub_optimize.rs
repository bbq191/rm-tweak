//! host/设备通用 CLI：对一本 EPUB 跑 `optimize::optimize_epub_with`（与设备端 book-serve Optimize 步**同一函数**）。
//! 缺省 = 清洗层（伪 DRM 剥离 / CSS 锁剥离 / 边距段距归零+2em / 空页清理 / 缺目录时自动目录 / 双 id 折叠）+ 优化器
//! （脚注拆环 / duokan 标记 / 远程图内联 / 双 id 去重 / 图片降采样 / e-ink 提对比），产物自带
//! `META-INF/com.cangjie.optimized` 标记，设备 autoopt 不会再优化一遍。`wash_epub.sh` 末步用它。
//!
//! 用法: epub-optimize [选项] 输入.epub 输出.epub    （流式路径进路径出，2026-09-19 起不再整本读进
//!   内存——真机 552MB 漫画全集坐实内存版会把设备逼近系统级 OOM，见 book-serve staging.rs 同一天
//!   的改动记录；输入输出同路径=就地覆盖时内部先写临时文件再改名，安全）
//!   --no-wash        只跑优化器不清洗（= v5 行为）
//!   --keep-spacing   清洗但保留原书段间距（诗集/剧本）
//!   --auto-toc       强制从 h1–h6 重建目录（缺省仅在无目录时生成）
//!   --footnote-anchor 脚注用章末锚点跳转（缺省即 Anchor，此参数保留兼容；2026-09-17 曾短暂改缺省
//!                      为段末块，真机验证用户实际期望是"翻到哪页注释跟哪页"而不是"跟着引用它的段落"，
//!                      EPUB 是流式重排做不到真正的页底部定位，撤回改回 Anchor，段末块整个下线）
//!   --check          产物过质量门，打印 JSON 报告；不过则退出码 3（产物仍写出）
//!   --require-toc    质量门把"无目录"升为失败
//!   --comic-min-margin 漫画补白到"页边距最小"的页框（配阅读器页边距 1；缺省补白到屏幕比例）
//! 退出码: 0 成功；1 用法错；2 优化失败（输入原样不动）；3 质量门未过。

use bookconv::optimize::{self, FootnoteMode, OptimizeOpts};
use bookconv::wash::{AutoToc, WashOpts};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags: Vec<&str> = args.iter().filter(|a| a.starts_with("--")).map(|s| s.as_str()).collect();
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if files.len() != 2 || flags.iter().any(|f| !["--no-wash", "--keep-spacing", "--auto-toc", "--footnote-anchor", "--check", "--require-toc", "--comic-min-margin"].contains(f)) {
        eprintln!("用法: epub-optimize [--no-wash] [--keep-spacing] [--auto-toc] [--footnote-anchor] [--check] [--require-toc] [--comic-min-margin] 输入.epub 输出.epub");
        std::process::exit(1);
    }
    let wash = if flags.contains(&"--no-wash") {
        None
    } else {
        Some(WashOpts {
            keep_para_spacing: flags.contains(&"--keep-spacing"),
            auto_toc: if flags.contains(&"--auto-toc") { AutoToc::Always } else { AutoToc::IfMissing },
            ..Default::default()
        })
    };
    // --footnote-anchor 现在是 no-op（缺省已经是 Anchor），继续留在允许的 flag 列表里只是不破坏已有脚本调用。
    let footnote = FootnoteMode::Anchor;
    // 输入输出同路径（就地覆盖）时不能边读边写同一个文件——先写临时文件，成功后再改名覆盖。
    let same_path = files[0] == files[1];
    let out_target: std::path::PathBuf = if same_path {
        let mut t = std::path::PathBuf::from(files[1]).into_os_string();
        t.push(".optimizing.tmp");
        std::path::PathBuf::from(t)
    } else {
        std::path::PathBuf::from(files[1])
    };
    let rep = match optimize::optimize_epub_file_streaming(std::path::Path::new(files[0]), &out_target, &OptimizeOpts { wash, footnote, comic_frame: if flags.contains(&"--comic-min-margin") { bookconv::imgopt::EpubComicFrame::MinMargin } else { Default::default() }, page_direction: None }, |_, _| {}) {
        Ok(r) => r,
        Err(e) => {
            let _ = std::fs::remove_file(&out_target);
            eprintln!("优化失败: {e}");
            std::process::exit(2);
        }
    };
    if same_path {
        if let Err(e) = std::fs::rename(&out_target, files[1]) {
            eprintln!("改名覆盖 {}: {e}", files[1]);
            std::process::exit(2);
        }
    }
    println!("epub-optimize v{}: {} 文件/{} 章, {} → {} 字节", optimize::OPTIMIZE_VERSION, rep.total_files, rep.html_files, rep.bytes_before, rep.bytes_after);
    if let Some(w) = &rep.wash {
        println!(
            "清洗: css {} / html {} / 伪DRM剥离 {:?} / 空页 {:?} / 自动目录 {} 条 / 双id折叠 {} / 分部重建 {} 条 / ncx uid 修复 {} / ncx doctype 剥离 {} / ncx manifest id 修复 {}",
            w.css_files, w.html_files, w.pseudo_drm_stripped, w.empty_pages_removed, w.toc_generated, w.dup_id_tags_collapsed, w.toc_parts_restructured, w.ncx_uid_fixed, w.ncx_doctype_stripped, w.ncx_manifest_id_fixed
        );
    }
    if flags.contains(&"--check") {
        // 质量门要整本读回内存核对结构——只有 --check 时才付这个代价，不影响默认路径的流式内存省。
        let out = match std::fs::read(files[1]) {
            Ok(b) => b,
            Err(e) => {
                eprintln!("质量门读产物失败: {e}");
                std::process::exit(3);
            }
        };
        match bookconv::check::check_epub(&out, flags.contains(&"--require-toc")) {
            Ok(r) => {
                println!("{}", serde_json::to_string_pretty(&r).unwrap_or_default());
                if !r.ok {
                    std::process::exit(3);
                }
            }
            Err(e) => {
                eprintln!("质量门读产物失败: {e}");
                std::process::exit(3);
            }
        }
    }
}
