//! 一次性诊断工具（2026-09-19，《镖人》第8卷真机上传反复失败排查用）：单独拆出某一卷、写到本地
//! 文件，脱离跟其他卷排队上传的干扰，方便直接对这一份单独测试（真机 `curl` 直传 xochitl `/upload`）。
//! 用法: comic-piece-extract <输入.epub> <标题子串，如"第8卷"> <输出.epub> [预算字节数，默认150MB]
use bookconv::comic_split;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 3 {
        eprintln!("用法: comic-piece-extract <输入.epub> <标题子串> <输出.epub> [预算字节数]");
        std::process::exit(1);
    }
    let input = std::path::Path::new(&args[0]);
    let want = &args[1];
    let output = std::path::Path::new(&args[2]);
    let budget: u64 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(150 * 1024 * 1024);

    let mut found = false;
    let result = comic_split::deliver_split_streaming(input, budget, |piece_name, bytes, _idx, _total| {
        if piece_name.contains(want.as_str()) {
            found = true;
            std::fs::write(output, bytes).map_err(|e| e.to_string())?;
            println!("写出 {piece_name}: {} 字节 -> {}", bytes.len(), output.display());
        } else {
            println!("跳过 {piece_name}（{} 字节，不是目标）", bytes.len());
        }
        Ok(())
    });
    match result {
        Ok(Some(outcome)) => {
            println!("delivered(跳过的也算成功构建): {:?}", outcome.delivered);
            println!("failed: {:?}", outcome.failed);
        }
        Ok(None) => println!("整本在预算内，或不是漫画——没有拆分方案"),
        Err(e) => {
            eprintln!("拆分失败: {e}");
            std::process::exit(2);
        }
    }
    if !found {
        eprintln!("警告：没有任何一份标题包含 {want:?}");
        std::process::exit(3);
    }
}
