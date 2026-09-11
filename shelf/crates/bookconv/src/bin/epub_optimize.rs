//! host/设备通用 CLI：对一本 EPUB 跑 `optimize::optimize_epub_with`（与设备端 book-serve Optimize 步**同一函数**）。
//! 缺省 = 清洗层（伪 DRM 剥离 / CSS 锁剥离 / 边距段距归零+2em / 空页清理 / 缺目录时自动目录 / 双 id 折叠）+ 优化器
//! （脚注拆环 / duokan 标记 / 远程图内联 / 双 id 去重 / 图片降采样 / e-ink 提对比），产物自带
//! `META-INF/com.cangjie.optimized` 标记，设备 autoopt 不会再优化一遍。`wash_epub.sh` 末步用它。
//!
//! 用法: epub-optimize [选项] 输入.epub 输出.epub    （输入输出可同路径=就地覆盖，先整本写内存再落盘）
//!   --no-wash        只跑优化器不清洗（= v5 行为）
//!   --keep-spacing   清洗但保留原书段间距（诗集/剧本）
//!   --auto-toc       强制从 h1–h6 重建目录（缺省仅在无目录时生成）
//!   --footnote-anchor 脚注用章末锚点跳转（缺省 Inline 内联常显，对齐设备 native→xochitl）
//!   --check          产物过质量门，打印 JSON 报告；不过则退出码 3（产物仍写出）
//!   --require-toc    质量门把"无目录"升为失败
//! 退出码: 0 成功；1 用法错；2 优化失败（输入原样不动）；3 质量门未过。

use bookconv::optimize::{self, FootnoteMode, OptimizeOpts};
use bookconv::wash::{AutoToc, WashOpts};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flags: Vec<&str> = args.iter().filter(|a| a.starts_with("--")).map(|s| s.as_str()).collect();
    let files: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    if files.len() != 2 || flags.iter().any(|f| !["--no-wash", "--keep-spacing", "--auto-toc", "--footnote-anchor", "--check", "--require-toc"].contains(f)) {
        eprintln!("用法: epub-optimize [--no-wash] [--keep-spacing] [--auto-toc] [--footnote-anchor] [--check] [--require-toc] 输入.epub 输出.epub");
        std::process::exit(1);
    }
    let epub = match std::fs::read(files[0]) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("读 {}: {e}", files[0]);
            std::process::exit(2);
        }
    };
    let wash = if flags.contains(&"--no-wash") {
        None
    } else {
        Some(WashOpts {
            keep_para_spacing: flags.contains(&"--keep-spacing"),
            auto_toc: if flags.contains(&"--auto-toc") { AutoToc::Always } else { AutoToc::IfMissing },
            ..Default::default()
        })
    };
    let footnote = if flags.contains(&"--footnote-anchor") { FootnoteMode::Anchor } else { FootnoteMode::Inline };
    let (out, rep) = match optimize::optimize_epub_with(&epub, &OptimizeOpts { wash, footnote }) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("优化失败: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = std::fs::write(files[1], &out) {
        eprintln!("写 {}: {e}", files[1]);
        std::process::exit(2);
    }
    println!("epub-optimize v{}: {} 文件/{} 章, {} → {} 字节", optimize::OPTIMIZE_VERSION, rep.total_files, rep.html_files, rep.bytes_before, rep.bytes_after);
    if let Some(w) = &rep.wash {
        println!("清洗: css {} / html {} / 伪DRM剥离 {:?} / 空页 {:?} / 自动目录 {} 条 / 双id折叠 {}", w.css_files, w.html_files, w.pseudo_drm_stripped, w.empty_pages_removed, w.toc_generated, w.dup_id_tags_collapsed);
    }
    if flags.contains(&"--check") {
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
