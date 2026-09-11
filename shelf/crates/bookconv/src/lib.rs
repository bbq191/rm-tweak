//! bookconv —— 通用电子书内容层（块③ 阅读，2026-09-03 从 weread-device 抽出的独立 crate）。
//!
//! 职责：多格式 → EPUB/PDF 转换（`convert`）、通用 EPUB 优化器（`optimize`）、清洗层（`wash`，对标 host Calibre
//! 规则）、EPUB 质量门（`check`）、图片按 Move 屏降采样/抖动（`imgopt`）、XHTML 处理规则（`htmlproc`）、
//! EPUB 组装（`epub`）、远程图抓取（`netimg`）。
//! **不含任何摄入/落盘/设备路径语义**（那些在各使用方：weread-device 的 ingest、shelf 的 book-serve）。
//! 使用方：`weread-device`（微读下书 + 旧上传页）与 `shelf/services/book-serve`（书架母版库「优化」），
//! 两者互不依赖，只共用本 crate；weread-device 以 `pub use bookconv::…` re-export 保旧路径不变。
pub mod article;
pub mod check;
pub mod comic_detect;
pub mod comic_pad;
pub mod comic_pdf;
pub mod comic_split;
pub mod convert;
pub mod direction;
pub mod epub;
pub mod epubzip;
pub mod htmlproc;
pub mod imgopt;
pub mod netimg;
pub mod imgpool;
pub mod naming;
pub mod placeholder;
pub mod optimize;
pub mod pdf_ingest;
pub mod stats;
pub mod util;
pub mod wash;
