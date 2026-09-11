use crate::{bitreader::Readable, Bitreader, ParseError};

use super::{crdt::CrdtId, lwwvalue::LwwValue, TypeParse};

pub struct SubBlock {
    pub tag: Tag,
    pub size: u32,
    pub position: u64,
}

impl SubBlock {
    pub fn validate_size(
        &self,
        reader: &mut TaggedBitreader<impl Readable>,
    ) -> Result<(), crate::ParseError> {
        let expected_offset = self.position + self.size as u64;
        let end_offset = reader.bit_reader.position();
        if end_offset > expected_offset {
            return Err(ParseError::invalid(format!(
                "Subblock overflowed its declared size. got {end_offset:x} expected {expected_offset:x}"
            )));
        }
        if end_offset < expected_offset {
            reader.bit_reader.set_position(expected_offset);
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
pub enum TagType {
    ID,
    Length4,
    Byte8,
    Byte4,
    Byte1,
}

impl TryFrom<u32> for TagType {
    type Error = ParseError;

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0x1 => Ok(TagType::Byte1),
            0x4 => Ok(TagType::Byte4),
            0x8 => Ok(TagType::Byte8),
            0xC => Ok(TagType::Length4),
            0x0F => Ok(TagType::ID),
            _ => Err(ParseError::invalid(format!(
                "Invalid tag for value '{value}'"
            ))),
        }
    }
}

#[derive(Debug)]
pub struct Tag {
    index: u32,
    tag_type: TagType,
}

impl Tag {
    /// Helper function to easily generate errors and to validate
    pub fn validate(&self, tag_type: TagType, index: u32) -> Result<(), ParseError> {
        if self.tag_type != tag_type {
            return Err(ParseError::invalid(format!(
                "Invalid tag type given '{:?}' expected '{:?}'",
                self.tag_type, tag_type
            )));
        }

        if self.index != index {
            return Err(ParseError::invalid(format!(
                "Invalid tag index given '{:?}' expected '{:?}'",
                self.index, index
            )));
        }

        Ok(())
    }
}

impl TypeParse for Tag {
    fn parse(reader: &mut TaggedBitreader<impl Readable>) -> Result<Self, ParseError> {
        let x = reader.bit_reader.read_varuint()?;
        Ok(Tag {
            index: x >> 4,
            tag_type: TagType::try_from(x & 0xF)?,
        })
    }
}

pub struct TaggedBitreader<'n, N: Readable> {
    pub bit_reader: &'n mut Bitreader<N>,
}

impl<'n, N: Readable> TaggedBitreader<'n, N> {
    pub fn new(bit_reader: &'n mut Bitreader<N>) -> TaggedBitreader<'n, N> {
        TaggedBitreader { bit_reader }
    }

    pub fn read_id(&mut self, index: u32) -> Result<CrdtId, ParseError> {
        self.read_tag(index, TagType::ID)?;
        CrdtId::parse(self)
    }

    pub fn read_bool(&mut self, index: u32) -> Result<bool, ParseError> {
        self.read_tag(index, TagType::Byte1)?;
        self.bit_reader.read_bool()
    }

    pub fn read_u8(&mut self, index: u32) -> Result<u8, ParseError> {
        self.read_tag(index, TagType::Byte1)?;
        self.bit_reader.read_u8()
    }

    pub fn read_u32(&mut self, index: u32) -> Result<u32, ParseError> {
        self.read_tag(index, TagType::Byte4)?;
        self.bit_reader.read_u32()
    }

    pub fn read_f32(&mut self, index: u32) -> Result<f32, ParseError> {
        self.read_tag(index, TagType::Byte4)?;
        self.bit_reader.read_f32()
    }

    pub fn read_f64(&mut self, index: u32) -> Result<f64, ParseError> {
        self.read_tag(index, TagType::Byte8)?;
        self.bit_reader.read_f64()
    }

    pub fn read_string(&mut self, index: u32) -> Result<String, ParseError> {
        let subblock = self.read_subblock(index)?;
        let string_length = self.bit_reader.read_varuint()?;
        let _is_ascii = self.bit_reader.read_bool()?;
        let string = self.bit_reader.read_string(string_length as usize)?;
        subblock.validate_size(self)?;
        Ok(string)
    }

    pub fn read_tag(&mut self, index: u32, tag_type: TagType) -> Result<Tag, ParseError> {
        let x = self.bit_reader.read_varuint()?;

        let tag = Tag {
            index: x >> 4,
            tag_type: TagType::try_from(x & 0xF)?,
        };
        tag.validate(tag_type, index)?;

        Ok(tag)
    }

    pub fn has_tag(&mut self, index: u32, tag_type: TagType) -> Result<bool, crate::ParseError> {
        let pos = self.bit_reader.position();
        let has_tag = self.read_tag(index, tag_type).is_ok();
        self.bit_reader.set_position(pos);
        Ok(has_tag)
    }

    pub fn read_subblock(&mut self, index: u32) -> Result<SubBlock, crate::ParseError>
    where
        Self: Sized,
    {
        let tag = self.read_tag(index, TagType::Length4)?;
        let size = self.bit_reader.read_u32()?;
        let position = self.bit_reader.position();

        Ok(SubBlock {
            tag,
            size,
            position,
        })
    }

    pub fn has_subblock(&mut self, index: u32) -> Result<bool, ParseError> {
        self.has_tag(index, TagType::Length4)
    }

    pub fn read_lww_u8(&mut self, index: u32) -> Result<LwwValue<u8>, ParseError> {
        let subblock = self.read_subblock(index)?;

        let timestamp = self.read_id(1)?;
        let value = self.read_u8(2)?;

        subblock.validate_size(self)?;

        Ok(LwwValue { timestamp, value })
    }

    pub fn read_lww_string(&mut self, index: u32) -> Result<LwwValue<String>, ParseError> {
        let subblock = self.read_subblock(index)?;

        let timestamp = self.read_id(1)?;

        let subblock2 = self.read_subblock(2)?;
        let length = self.bit_reader.read_varuint()?;
        let _is_ascii = self.bit_reader.read_bool()?;
        let value = self.bit_reader.read_string(length.try_into()?)?;
        subblock2.validate_size(self)?;
        subblock.validate_size(self)?;

        Ok(LwwValue { timestamp, value })
    }

    pub fn read_lww_bool(&mut self, index: u32) -> Result<LwwValue<bool>, ParseError> {
        let subblock = self.read_subblock(index)?;

        let timestamp = self.read_id(1)?;
        let value = self.read_bool(2)?;

        subblock.validate_size(self)?;

        Ok(LwwValue { timestamp, value })
    }

    pub fn read_lww_id(&mut self, index: u32) -> Result<LwwValue<CrdtId>, ParseError> {
        let subblock = self.read_subblock(index)?;

        let timestamp = self.read_id(1)?;
        let value = self.read_id(2)?;

        subblock.validate_size(self)?;

        Ok(LwwValue { timestamp, value })
    }

    pub fn read_lww_f32(&mut self, index: u32) -> Result<LwwValue<f32>, ParseError> {
        let subblock = self.read_subblock(index)?;

        let timestamp = self.read_id(1)?;
        let value = self.read_f32(2)?;

        subblock.validate_size(self)?;

        Ok(LwwValue { timestamp, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bitreader;

    // 2026-09-09 审计补：这条 crate 处理设备上传的外部 `.rm` 文件，`tagged_bit_reader.rs` 是
    // 除 `bitreader.rs` 外解析链路的第二层地基（每个字段读之前都先核 tag），此前零单测，
    // 测试策略完全依赖真机样本回归。下面补齐"tag 类型不认识 / index 或类型对不上 / 子块声明
    // 长度跟实际不符"这几类构造性反例。

    fn tag_byte(index: u32, tag_type: u32) -> u8 {
        ((index << 4) | tag_type) as u8
    }

    #[test]
    fn tag_type_rejects_unknown_value_accepts_known_ones() {
        for known in [0x1u32, 0x4, 0x8, 0xC, 0xF] {
            assert!(TagType::try_from(known).is_ok(), "0x{known:X} 是已知 tag type");
        }
        for unknown in [0x0u32, 0x2, 0x3, 0x5, 0xA, 0xE] {
            assert!(TagType::try_from(unknown).is_err(), "0x{unknown:X} 不是已知 tag type，该拒绝而不是猜一个");
        }
    }

    #[test]
    fn read_u8_roundtrip_with_correct_tag() {
        let data: &[u8] = &[tag_byte(3, 0x1), 0x2A];
        let mut br = Bitreader::new(data);
        let mut r = TaggedBitreader::new(&mut br);
        assert_eq!(r.read_u8(3).unwrap(), 0x2A);
    }

    #[test]
    fn read_tag_rejects_index_mismatch() {
        // 数据里写的是 index=3，调用方要的是 index=5——同一份数据、字段错位（比如上游哪个
        // 字段悄悄漏读/多读一次）该被拦下，不能糊里糊涂读出一个错位的值。
        let data: &[u8] = &[tag_byte(3, 0x1), 0x2A];
        let mut br = Bitreader::new(data);
        let mut r = TaggedBitreader::new(&mut br);
        assert!(r.read_u8(5).is_err());
    }

    #[test]
    fn read_tag_rejects_type_mismatch() {
        // index 对得上（3），但类型对不上：数据是 Byte1，调用方按 Byte4 读。
        let data: &[u8] = &[tag_byte(3, 0x1), 0x2A];
        let mut br = Bitreader::new(data);
        let mut r = TaggedBitreader::new(&mut br);
        assert!(r.read_u32(3).is_err());
    }

    #[test]
    fn has_tag_probes_without_moving_position() {
        let data: &[u8] = &[tag_byte(3, 0x1), 0x2A];
        let mut br = Bitreader::new(data);
        let mut r = TaggedBitreader::new(&mut br);
        assert!(r.has_tag(3, TagType::Byte1).unwrap(), "该有这个 tag");
        assert!(!r.has_tag(9, TagType::Byte1).unwrap(), "index 对不上");
        // 探测过后游标该在原处——用真的 read_u8 验证还能正常读到同一个值，不是被 has_tag 拖走了。
        assert_eq!(r.read_u8(3).unwrap(), 0x2A);
    }

    #[test]
    fn subblock_overflow_is_rejected() {
        // 子块声明只有 1 字节内容，但实际读走了 4 字节（size 字段之后 varuint 长度=1 + is_ascii
        // 1 字节 + 1 字节字符串内容 = 3 字节，declared size 故意写成 1）——validate_size 该报错，
        // 不是悄悄接受一个跟声明长度对不上的子块（这是"声明长度超过/不足剩余字节"这类畸形输入
        // 的核心场景）。
        let tag = tag_byte(0, 0xC); // Length4
        let size: u32 = 1; // 故意写小
        let mut data = vec![tag];
        data.extend(size.to_le_bytes());
        data.push(1); // string_length varuint = 1
        data.push(0); // is_ascii = false
        data.push(b'x'); // 1 字节内容
        let mut br = Bitreader::new(data.as_slice());
        let mut r = TaggedBitreader::new(&mut br);
        assert!(r.read_string(0).is_err(), "声明的子块长度比实际读到的内容短，该报错");
    }

    #[test]
    fn subblock_declares_extra_trailing_bytes_seeks_forward_not_error() {
        // 反过来：声明的长度比实际内容长（比如新固件字段后面多塞了字节，旧解析器该跳过而不是
        // 报错）——这是 validate_size 里 `end_offset < expected_offset` 分支，代码本身认为
        // 这种情况不算错误，测试钉住这条设计意图不被后续改动破坏。
        let tag = tag_byte(0, 0xC);
        let content_len = 1u8 + 1 + 1; // varuint(1B) + is_ascii(1B) + 'x'(1B) = 3
        let size: u32 = content_len as u32 + 5; // 声明比实际多 5 字节
        let mut data = vec![tag];
        data.extend(size.to_le_bytes());
        data.push(1);
        data.push(0);
        data.push(b'x');
        data.extend([0u8; 5]); // 声明范围内的"多余"字节，read_string 不会主动去读它们
        let mut br = Bitreader::new(data.as_slice());
        let mut r = TaggedBitreader::new(&mut br);
        assert_eq!(r.read_string(0).unwrap(), "x", "内容本身该能正常读出来，不受多余声明长度影响");
    }

    #[test]
    fn truncated_subblock_header_is_io_error_not_panic() {
        // 只给 tag 字节，size（u32，4 字节）被截断——最基础的"文件在子块头部就断了"场景。
        let data: &[u8] = &[tag_byte(0, 0xC), 0x01];
        let mut br = Bitreader::new(data);
        let mut r = TaggedBitreader::new(&mut br);
        assert!(r.read_subblock(0).is_err());
    }
}
