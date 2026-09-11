use std::collections::HashMap;

use crate::{
    bitreader::Readable,
    v6::{
        crdt::{CrdtId, CrdtSequence, CrdtSequenceItem},
        lwwvalue::LwwValue,
        tagged_bit_reader::{TagType, TaggedBitreader},
        TypeParse,
    },
    ParseError,
};



#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(non_camel_case_types)] // 沿用上游全大写命名，与 rmscene 对照读码方便
/// Text paragraph style.
pub enum ParagraphStyle {
    BASIC,
    PLAIN,
    HEADING,
    BOLD,
    BULLET,
    BULLET2,
    /// 复选框（rmscene CHECKBOX=6 / CHECKBOX_CHECKED=7；3.28 格式菜单「复选框」）。
    /// 真机坐实：格式菜单打的"未勾选"复选框，无论后面文字是否叫"finished"，都是 6——
    /// 勾上号（7）要点一下渲染出来的方框，不是打字样式，本样本没验到，写入器慎用。
    CHECKBOX,
    CHECKBOX_CHECKED,
    /// 有序列表（3.28 格式菜单「已编号列表」）。真机坐实码 10（2026-09-07，真机样本 `testdata/seven_styles`）；
    /// 旧 rmscene 0.8.0 不认，读到会警告丢弃当 PLAIN。格式子块跟其余样式一样只有 2 字节（`17`+样式码），
    /// **没有隐藏载荷**——早前"多 7 字节未解码"的说法是分析失误，那 7 字节其实属于 BOLD 上的
    /// Subheading 1 开关（见 `write::SUBHEADING1_MARKER`），已用二轮真机样本更正、`rmv6::write`
    /// 正常写入 NUMBERED 且编号显示正确。
    NUMBERED,
    /// 仍未知的样式码。不让未知值把整个 .rm 解析打挂（rmscene 同样容忍并继续）。
    Unknown(u8),
}

impl TryFrom<u8> for ParagraphStyle {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        Ok(match value {
            0x00 => ParagraphStyle::BASIC,
            0x01 => ParagraphStyle::PLAIN,
            0x02 => ParagraphStyle::HEADING,
            0x03 => ParagraphStyle::BOLD,
            0x04 => ParagraphStyle::BULLET,
            0x05 => ParagraphStyle::BULLET2,
            0x06 => ParagraphStyle::CHECKBOX,
            0x07 => ParagraphStyle::CHECKBOX_CHECKED,
            0x0a => ParagraphStyle::NUMBERED,
            v => ParagraphStyle::Unknown(v),
        })
    }
}

#[derive(Debug, Clone)]
pub enum TextItem {
    FormatCode(u32),
    Text(String),
}

#[derive(Debug, Clone)]
/// Block of text
pub struct Text {
    pub items: CrdtSequence<TextItem>,
    pub styles: HashMap<CrdtId, LwwValue<ParagraphStyle>>,
    pub x: f64,
    pub y: f64,
    pub width: f32,
}
impl TypeParse for Text {
    fn parse(reader: &mut TaggedBitreader<impl Readable>) -> Result<Self, crate::ParseError> {
        // subblocks
        let subblock1 = reader.read_subblock(2)?;
        let subblock2 = reader.read_subblock(1)?;
        let subblock3 = reader.read_subblock(1)?;

        // Text items
        let amount_items = reader.bit_reader.read_varuint()?;
        let items = (0..amount_items)
            .into_iter()
            .map(|_| {
                let subblock = reader.read_subblock(0)?;
                let item_id = reader.read_id(2)?;
                let left_id = reader.read_id(3)?;
                let right_id = reader.read_id(4)?;
                let deleted_length = reader.read_u32(5)?;

                let value = if reader.has_subblock(6)? {
                    let subblock = reader.read_subblock(6)?;

                    let string_length = reader.bit_reader.read_varuint()?;
                    // XXX might have a different meaning
                    let _is_ascii = reader.bit_reader.read_bool()?;
                    let string = reader.bit_reader.read_string(string_length as usize)?;

                    // if tag exists use format
                    let value = if reader.has_tag(2, TagType::Byte4)? {
                        let fmt_code = reader.read_u32(2)?;
                        TextItem::FormatCode(fmt_code)
                    } else {
                        TextItem::Text(string)
                    };
                    subblock.validate_size(reader)?;
                    value
                } else {
                    TextItem::Text(String::new())
                };
                subblock.validate_size(reader)?;

                return Ok(CrdtSequenceItem {
                    item_id,
                    left_id,
                    right_id,
                    deleted_length,
                    value,
                });
            })
            .collect::<Result<CrdtSequence<TextItem>, ParseError>>()?;

        subblock2.validate_size(reader)?;
        subblock3.validate_size(reader)?;

        let subblock4 = reader.read_subblock(2)?;
        let subblock5 = reader.read_subblock(1)?;

        // Formatting
        let amount_styles = reader.bit_reader.read_varuint()?;
        let styles = (0..amount_styles)
            .into_iter()
            .map(|_| {
                let id = CrdtId::parse(reader)?;
                let timestamp = reader.read_id(1)?;

                let subblock6 = reader.read_subblock(2)?;
                // XXX not sure what this is format?
                let _c = reader.bit_reader.read_u8()?;
                let style = ParagraphStyle::try_from(reader.bit_reader.read_u8()?)?;
                subblock6.validate_size(reader)?;
                Ok((
                    id,
                    LwwValue {
                        timestamp,
                        value: style,
                    },
                ))
            })
            .collect::<Result<HashMap<CrdtId, LwwValue<ParagraphStyle>>, ParseError>>()?;

        subblock4.validate_size(reader)?;
        subblock5.validate_size(reader)?;

        subblock1.validate_size(reader)?;

        // Last section
        // "pos_x" and "pos_y" from ddvk? Gives negative number -- possibly could
        // be bounding box?
        let subblock7 = reader.read_subblock(3)?;
        let x = reader.bit_reader.read_f64()?;
        let y = reader.bit_reader.read_f64()?;
        subblock7.validate_size(reader)?;

        let width = reader.read_f32(4)?;

        Ok(Text {
            items,
            styles,
            x,
            y,
            width,
        })
    }
}
