//! rmv6 —— reMarkable `.rm` v6 笔迹文件**只读解析**，笔记线（notes/）的地基。
//!
//! 剥离移植自 vendored `remarkable_lines` 0.1.3（MIT，来源与改动见 `PROVENANCE.md`）：只留 v6（去掉 v3–v5），
//! 保留两处兼容补丁（`PenColor`/`ParagraphStyle`/`Tool` 未知码兜底、块尾多余字节跳过），补 CHECKBOX/NUMBERED 样式码
//! （2026-09-07 真机样本坐实：NUMBERED=10，其格式子块比其余样式多 7 字节未解码——见 `v6::scene_item::text::ParagraphStyle`）。
//!
//! 两层 API：
//! - 低层 [`RmFile::read`]：全部 block + 场景树（逆向格式原样暴露，调试/新字段探索用）；
//! - 高层 [`page::Page`]：一页 = 笔画 + 勾画（GlyphRange：原文/偏移/矩形）+ 打字文本，墓碑已剔除——
//!   ink-serve 几何配对、note-serve 读回校对文本都只用这一层。
use bitreader::Bitreader;
use bitreader::Readable;
use v6::block::Block;
use v6::scene_tree::SceneTree;
use v6::tagged_bit_reader::TaggedBitreader;
use v6::TypeParse;

pub mod bitreader;
pub mod page;
pub mod parse_error;
pub mod shared;
pub mod v6;
pub mod write;

pub use crate::parse_error::ParseErrorKind;
pub use parse_error::ParseError;

const V6_HEADER: &str = "reMarkable .lines file, version=6";

/// 一个 v6 `.rm` 文件：场景树 + 全部块。
#[derive(Debug)]
pub struct RmFile {
    pub tree: SceneTree,
    pub blocks: Vec<Block>,
}

impl RmFile {
    pub fn read(input: impl Readable) -> Result<RmFile, ParseError> {
        let mut reader = Bitreader::new(input);
        Self::read_impl(&mut reader).map_err(|e| e.with_context_from_bitreader(&mut reader))
    }

    fn read_impl(reader: &mut Bitreader<impl Readable>) -> Result<RmFile, ParseError> {
        let header = reader.read_bytes(43)?.into_iter().map(|i| i as char).collect::<String>();
        let header = header.trim_end();
        if header != V6_HEADER {
            return Err(ParseError::unsupported(format!("只认 v6 .rm（{V6_HEADER}），文件头是: {header}")));
        }
        let mut blocks = vec![];
        let mut tagged = TaggedBitreader::new(reader);
        loop {
            if tagged.bit_reader.eof()? {
                break;
            }
            blocks.push(Block::parse(&mut tagged)?);
        }
        let tree = SceneTree::from_blocks(&blocks)?;
        Ok(RmFile { tree, blocks })
    }
}
