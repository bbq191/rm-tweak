//! host CLI：漫画 CBZ → 固定版式 PDF（一图一页），`shelf push` 判断灰阶 CBZ 体积够小、大概率能塞进原生
//! 上传上限时顺带调它，跟 CBZ 一起落母版库，给「投入原生书库」多一个选项——参见 push.py::comic_prepare。
//! **`--off`（缺省）**：原样直嵌，不重新抖动——喂给它的是 `comic_gray.py` 已经处理过（16 灰/保色）的 CBZ，
//! 再抖一遍只会烧画质。`--mono` 留着给以后可能要的场景（未接进 `shelf push`），行为等价 `cbz_to_pdf` 的
//! `EinkTone::Mono` 档：黑白/偏色页转 1-bit 抖动，真彩页保留彩色。
//!
//! 用法: cbz2pdf [--mono] 输入.cbz 输出.pdf
//! 退出码: 0 成功；1 用法错；2 转换失败（输入原样不动）。

use bookconv::convert::cbz::cbz_to_pdf;
use bookconv::convert::EinkTone;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mono = args.iter().any(|a| a == "--mono");
    let files: Vec<&String> = args.iter().filter(|a| a.as_str() != "--mono").collect();
    if files.len() != 2 {
        eprintln!("用法: cbz2pdf [--mono] 输入.cbz 输出.pdf");
        std::process::exit(1);
    }
    let data = match std::fs::read(files[0]) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("读 {}: {e}", files[0]);
            std::process::exit(2);
        }
    };
    let tone = if mono { EinkTone::Mono } else { EinkTone::Off };
    let pdf = match cbz_to_pdf(&data, tone) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("转换失败: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = std::fs::write(files[1], &pdf) {
        eprintln!("写 {}: {e}", files[1]);
        std::process::exit(2);
    }
    println!("cbz2pdf: {} → {} 字节", files[0], pdf.len());
}
