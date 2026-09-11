//! HTML 规整：body_inner / split_blocks / fix_internal_links —— 移植自 download.py + epub.py。
//! regex crate 不支持 lookahead，`&(?!#?\w+;)` 的转义手写实现。
//!
//! 子模块：`basic`（正文提取/块切分/内链规整/id 去重）· `footnote_cycles`（脚注互指环拆解）· `footnote`（脚注识别/收集/
//! 就地重排/内联）· `contrast`（e-ink 提对比、字体锁剥离）。

use regex::Regex;
use std::sync::OnceLock;

// 按职责拆成子模块（原 `htmlproc.rs` 一个文件 1400 行、6 个测试模块与代码交错）；`pub` 项在这里 glob re-export，
// `crate::htmlproc::xxx` 旧路径不变；兄弟模块之间互相调用也走这层（各子模块 `use super::*`）。
mod basic;
mod contrast;
mod footnote;
mod footnote_cycles;

pub use self::basic::*;
pub use self::contrast::*;
pub use self::footnote::*;
pub use self::footnote_cycles::*;
