//! 幂等标记：优化产物内埋的 `META-INF/cangjie-optimized` 版本号读写与判定。
use super::*;

/// 从任意可 seek 的 zip 读端取标记（内存字节与磁盘文件共用；ZipArchive 只读中央目录 + 标记那一条，不解压正文）。
pub(super) fn marker_in<R: Read + std::io::Seek>(reader: R) -> Option<String> {
    let mut ar = ZipArchive::new(reader).ok()?;
    let mut f = ar.by_name(OPTIMIZE_MARKER).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s.trim().to_string())
}

/// 读 EPUB 判是否已被本优化器处理过。返回内埋的版本串(Some=已优化)。
/// 非 zip / 损坏 / 无标记都当"未优化"(None)。
pub fn optimized_version(epub: &[u8]) -> Option<String> {
    marker_in(Cursor::new(epub))
}

/// 是否已优化过(任意版本)。
pub fn is_optimized(epub: &[u8]) -> bool {
    optimized_version(epub).is_some()
}

/// 标记值分等级（2026-09-05，修"已优化徽章说谎"）：**含清洗层的完整优化 = 版本号本身**；只跑核心遍
/// （`wash=None`，如网文 / 格式转换产物的 `assemble_optimized`）= `<版本>-core`。母版库据此显示
/// 「已优化 / 已优化·未清洗」并只对 full 隐藏「优化」按钮；weread 线只看"有无标记"（`is_optimized`），不受影响。
pub fn marker_value(full: bool) -> String {
    if full { OPTIMIZE_VERSION.to_string() } else { format!("{OPTIMIZE_VERSION}-core") }
}

/// 同 optimized_version，但直接开文件——不把整本 epub 读进内存，供书库列表逐本轻量标注。
pub fn optimized_version_file(path: &str) -> Option<String> {
    // 必须套缓冲：ZipArchive 解析中央目录是逐字段几字节的小读，裸 File 每条目十几次系统调用，
    // 漫画 EPUB 动辄几百上千条目，设备上一次列表就吃掉可观的 CPU（2026-09-20 battop 实测）。
    marker_in(std::io::BufReader::new(std::fs::File::open(path).ok()?))
}
