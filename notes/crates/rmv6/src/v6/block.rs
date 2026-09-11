use crate::{bitreader::Readable, v6::crdt::CrdtId, Bitreader, ParseError};

mod blocks;
pub use blocks::*;

use super::{
    scene_item::{glyph_range::GlyphRange, line::Line, text::Text},
    tagged_bit_reader::TaggedBitreader,
    TypeParse,
};

#[derive(Debug)]
pub struct BlockInfo {
    pub start_offset: u64,
    pub size: u32,
    pub min_version: u8,
    pub current_version: u8,
}

impl BlockInfo {
    pub fn has_bytes_remaining(&self, reader: &Bitreader<impl Readable>) -> bool {
        self.start_offset + self.size as u64 > reader.position()
    }
}

#[derive(Debug, Clone)]
pub enum Block {
    MigrationInfo(MigrationInfoBlock),
    PageInfo(PageInfoBlock),
    TreeNode(TreeNodeBlock),
    SceneTree(SceneTreeBlock),
    SceneGlyphItem(SceneItemBlock<GlyphRange>),
    SceneGroupItem(SceneItemBlock<CrdtId>),
    SceneLineItem(SceneItemBlock<Line>),
    SceneTextItem(SceneItemBlock<Text>),
    AuthorsIds(AuthorsIdsBlock),
    RootText(RootTextBlock),
    Unknown { block_type: u8, data: Vec<u8> },
}

/// Parsing methods for parsing blocks
pub trait BlockParse {
    fn parse(
        info: &BlockInfo,
        reader: &mut TaggedBitreader<impl Readable>,
    ) -> Result<Self, ParseError>
    where
        Self: Sized;
}

impl TypeParse for Block {
    fn parse(reader: &mut TaggedBitreader<impl Readable>) -> Result<Self, ParseError> {
        let size = reader.bit_reader.read_u32()?;

        // unknown value
        let _ = reader.bit_reader.read_u8()?;
        let min_version = reader.bit_reader.read_u8()?;
        let current_version = reader.bit_reader.read_u8()?;
        let block_type = reader.bit_reader.read_u8()?;

        if current_version < min_version {
            return Err(ParseError::invalid(
                "current_version can't be smaller than min_version",
            ));
        }

        let start_offset = reader.bit_reader.position();

        // println!(
        //     "\nStarting new block at offset {:x} until {:x}",
        //     reader.bit_reader.position() - 4,
        //     start_offset + size as u64
        // );

        let info = BlockInfo {
            start_offset,
            size,
            min_version,
            current_version,
        };

        let block = match block_type {
            0x00 => Block::MigrationInfo(MigrationInfoBlock::parse(&info, reader)?),
            0x01 => Block::SceneTree(SceneTreeBlock::parse(&info, reader)?),
            0x02 => Block::TreeNode(TreeNodeBlock::parse(&info, reader)?),
            0x03 => Block::SceneGlyphItem(SceneItemBlock::parse(
                &info,
                reader,
                SceneItemType::SceneGlyphItemBlock,
                |_info, reader| GlyphRange::parse(reader),
            )?),
            0x04 => Block::SceneGroupItem(SceneItemBlock::parse(
                &info,
                reader,
                SceneItemType::SceneGroupItemBlock,
                |_info, reader| {
                    // XXX don't know what this means
                    reader.read_id(2)
                },
            )?),
            0x05 => Block::SceneLineItem(SceneItemBlock::parse(
                &info,
                reader,
                SceneItemType::SceneLineItemBlock,
                |info, reader| Line::parse(info, reader),
            )?),
            0x06 => Block::SceneTextItem(SceneItemBlock::parse(
                &info,
                reader,
                SceneItemType::SceneTextItemBlock,
                |_info, reader| Text::parse(reader),
            )?),
            0x07 => Block::RootText(RootTextBlock::parse(&info, reader)?),
            0x09 => Block::AuthorsIds(AuthorsIdsBlock::parse(&info, reader)?),
            0x0A => Block::PageInfo(PageInfoBlock::parse(&info, reader)?),
            _ => Block::Unknown {
                block_type,
                data: reader.bit_reader.read_bytes(size as usize)?,
            },
        };

        let expected_offset = start_offset + size as u64;
        let end_offset = reader.bit_reader.position();
        if end_offset < expected_offset {
            // 新固件给块加了本 crate 不认的尾部字段（rmscene 也只是 "some data not read"）。
            // 跳过剩余字节到块尾即可，不必把整个 .rm 打挂——我们只关心 Line 笔划。
            let skip = (expected_offset - end_offset) as usize;
            reader.bit_reader.read_bytes(skip)?;
        } else if end_offset > expected_offset {
            // 多读 = 真的读错了边界，才报错。
            return Err(ParseError::invalid(format!(
                "Block type '{block_type}' over-read. got {end_offset:x} expected {expected_offset:x}"
            )));
        }

        Ok(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::v6::TypeParse;

    // 2026-09-09 审计补：`Block::parse` 是 rmv6 里"declared length（外层块的 size 字段）跟实际
    // 内容是否对得上"这条纪律的把关点，此前零单测。构造字节序列覆盖 version 倒挂 / 未知
    // block_type 兜底 / 头部截断三类畸形输入，另加一条已知块类型（MigrationInfoBlock）走完整
    // dispatch 流程的正向用例。

    fn header(size: u32, min_version: u8, current_version: u8, block_type: u8) -> Vec<u8> {
        let mut v = size.to_le_bytes().to_vec();
        v.push(0); // unknown 字节，Block::parse 读了不用
        v.push(min_version);
        v.push(current_version);
        v.push(block_type);
        v
    }

    #[test]
    fn current_version_less_than_min_version_is_rejected() {
        let data = header(0, 3, 2, 0xFF); // current(2) < min(3)
        let mut br = Bitreader::new(data.as_slice());
        let mut r = TaggedBitreader::new(&mut br);
        let e = Block::parse(&mut r).unwrap_err();
        assert!(e.message.contains("current_version"), "{}", e.message);
    }

    #[test]
    fn unknown_block_type_captures_raw_bytes_without_error() {
        // 0xFF 不在任何已知 block_type 分派表里——这条 crate 只关心自己认识的块类型，其余原样
        // 收进 Unknown 变体，不该因为"不认识"就整份报错。
        let content = [0xAAu8, 0xBB, 0xCC];
        let mut data = header(content.len() as u32, 0, 0, 0xFF);
        data.extend(content);
        let mut br = Bitreader::new(data.as_slice());
        let mut r = TaggedBitreader::new(&mut br);
        match Block::parse(&mut r).unwrap() {
            Block::Unknown { block_type, data } => {
                assert_eq!(block_type, 0xFF);
                assert_eq!(data, content);
            }
            other => panic!("应该落进 Unknown 变体: {other:?}"),
        }
    }

    #[test]
    fn truncated_header_is_io_error_not_panic() {
        // 只给 3 个字节（size 字段还没读完）——最基础的"文件在块头部就断了"场景。
        let data: &[u8] = &[0x01, 0x00, 0x00];
        let mut br = Bitreader::new(data);
        let mut r = TaggedBitreader::new(&mut br);
        assert!(Block::parse(&mut r).is_err());
    }

    #[test]
    fn known_block_type_parses_through_full_dispatch() {
        // MigrationInfoBlock（block_type=0x00）：read_id(1) + read_u8(2)，内容长度算好跟声明的
        // size 精确对上，走一遍"认识的块类型"这条分派路径，跟上面两条"不认识的类型"/"版本倒挂"
        // 互补。
        let content = [
            0x1Fu8, // tag: index=1, type=ID(0xF)
            0x05,   // CrdtId.part1
            0x07,   // CrdtId.part2（varuint 单字节，无续接位）
            0x21,   // tag: index=2, type=Byte1(0x1)
            0x01,   // is_device = true
        ];
        let mut data = header(content.len() as u32, 0, 0, 0x00);
        data.extend(content);
        let mut br = Bitreader::new(data.as_slice());
        let mut r = TaggedBitreader::new(&mut br);
        match Block::parse(&mut r).unwrap() {
            Block::MigrationInfo(b) => {
                assert_eq!((b.migration_id.part1, b.migration_id.part2), (5, 7));
                assert!(b.is_device);
            }
            other => panic!("应该落进 MigrationInfo 变体: {other:?}"),
        }
    }
}
