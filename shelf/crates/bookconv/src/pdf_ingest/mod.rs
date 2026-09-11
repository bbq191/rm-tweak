//! 入库 PDF 的「优化」：有文字层→转 EPUB（保留图片+公式+必须有 TOC），无文字层的扫描件/
//! PDF 漫画→只做裁边处理、格式不变。跟 `comic_pdf.rs`（EPUB→PDF）方向相反，两条产线平行独立、
//! 互不影响。
//!
//! **三个新依赖分工**（都是纯 Rust、无 C 依赖，见 `Cargo.toml` 里各自的引入注释）：
//! - `lopdf`：本模块自己直接依赖的版本（见 `Cargo.toml` 锁 0.45.0）——页树/字体资源/图片
//!   XObject/`/Outlines` 书签树读取，`classify_pdf`/`optimize_pdf_trim_only`/
//!   `optimize_pdf_to_epub` 的结构解析部分都走它。
//! - `pdf-extract`（本地 fork `pdf-extract-cj`）：2026-09-24 起与本模块用**同一版 lopdf 0.45**
//!   （`pub use lopdf::*` 重导出的就是同一个类型），只用来驱动它的 `OutputDev` 钩子拿逐字符位置+字号，
//!   直接复用本模块已经解析好的 `lopdf::Document`（`extract_positioned_text_doc`）。此前 fork 锁 lopdf 0.42、
//!   两边类型不能互传，每本 PDF 要整份解析两遍、二进制里也编进两份 lopdf。**已知局限**：
//!   `OutputDev::output_character` 不带字体名（只有字号），公式区域探测这次只能靠 Unicode 码位判断，
//!   不能按数学字体族名判断（见 `is_formula_char` 文档注释，含真实 pdflatex 样本验证过 Unicode 映射基本正确）。
//! - `hayro`：整页光栅化，只给公式区域裁剪用（自己独立解析一遍 PDF 字节，`Pdf::new(bytes)`）。
//!
//! 一份 PDF 因此在 `optimize_pdf_to_epub` 里最多被解析两遍（lopdf 一遍，供结构读取与逐字提取共用；
//! hayro 只在有公式时才额外解析+渲染）。
//!
//! **已知局限（真实样本核对时发现，未修——按整行字符包围盒算公式区域，天然是粗粒度的）**：
//! 行内公式紧贴正文（如 "the identity $e^{i\\pi}+1=0$ is"）时，公式两侧紧邻的一两个正文
//! 单词可能被误判进公式块的包围盒、从正文里消失（`tests/fixtures/sample.pdf` 里
//! "identity" 被吞成 "i e" 就是这个问题）——根因是 pdf-extract 在字体切换处（正文字体切数学
//! 斜体/符号字体）就会分出新的 `line`，公式区域按"这一整行字符的包围盒"算，跟真正的公式视觉
//! 边界不完全重合。要根治需要按字符级别（不是行级别）精确圈公式区域，这次没做，记在这里避免
//! 以后误以为是新 bug。

use crate::convert::pdfwrite::{self, PdfPieceWriter};
use crate::epub::{Book, BookMeta, Chapter, NavEntry, Resource};
use std::path::Path;

// 按职责拆成子模块（原 `pdf_ingest.rs` 一个文件 1100 行）：`classify` · `text` · `headings` · `trim` · `to_epub` · `source`；
// `pub`/`pub(crate)` 项在这里 glob re-export，`crate::pdf_ingest::xxx` 旧路径不变。
mod classify;
mod headings;
mod source;
mod text;
mod to_epub;
mod trim;

pub use self::classify::*;
pub use self::headings::*;
pub use self::source::*;
pub(crate) use self::text::*;
pub use self::to_epub::*;
pub use self::trim::*;

#[cfg(test)]
mod tests;

/// 单条对象流/交叉引用流解压后的上限（lopdf 载入时会立刻解开这两类流；默认不设限，几 KB 的压缩流可以解出几 GB）。
/// 正常 PDF 的这类流远小于 1MB，64MB 只拦解压炸弹。页内容流的上限在 `pdf-extract-cj` 里。
const MAX_LOAD_STREAM_BYTES: usize = 64 * 1024 * 1024;

/// 按字节解析 PDF（带解压上限）。
pub(super) fn parse_pdf(bytes: &[u8]) -> Result<lopdf::Document, String> {
    lopdf::Document::load_mem_with_options(bytes, lopdf::LoadOptions::with_max_decompressed_size(MAX_LOAD_STREAM_BYTES)).map_err(|e| format!("PDF 结构解析失败: {e}"))
}

/// 读文件并解析：原始字节解析完即释放（`Document` 自己持有全部对象），不让整本 PDF 的字节陪着后续逐字提取、
/// 图片解码撑到函数结束（2026-09-24 审计：分类/裁边/转 EPUB 三处此前都把字节留到函数末尾）。
pub(super) fn load_pdf(src: &Path) -> Result<lopdf::Document, String> {
    let bytes = std::fs::read(src).map_err(|e| format!("读源文件失败: {e}"))?;
    parse_pdf(&bytes)
}


// ============================================================================
// 分类
// ============================================================================
