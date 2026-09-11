use crate::{
    shared::pen_color::PenColor,
    v6::{tagged_bit_reader::TagType, TypeParse},
    ParseError,
};

#[derive(Debug, Clone)]
pub struct Rectangle {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl TypeParse for Rectangle {
    fn parse(
        reader: &mut crate::v6::tagged_bit_reader::TaggedBitreader<impl crate::bitreader::Readable>,
    ) -> Result<Self, crate::ParseError> {
        Ok(Rectangle {
            x: reader.bit_reader.read_f64()?,
            y: reader.bit_reader.read_f64()?,
            w: reader.bit_reader.read_f64()?,
            h: reader.bit_reader.read_f64()?,
        })
    }
}

#[derive(Debug, Clone)]
pub struct GlyphRange {
    pub start: u32,
    pub length: u32,
    pub text: String,
    pub color: PenColor,
    /// 可选 RGBA（tag 10）。Paper Pro 彩色高亮把实际颜色放这里；同 PenColor::Unknown(9)
    /// (HIGHLIGHT) 下靠它区分蓝/橙等。存的是 little-endian uint32(BGRA)，这里已解成 (R,G,B,A)。
    pub color_rgba: Option<(u8, u8, u8, u8)>,
    pub rectangles: Vec<Rectangle>,
}

impl TypeParse for GlyphRange {
    fn parse(
        reader: &mut crate::v6::tagged_bit_reader::TaggedBitreader<impl crate::bitreader::Readable>,
    ) -> Result<Self, crate::ParseError> {
        let start = reader.read_u32(2)?;
        let length = reader.read_u32(3)?;
        let color = PenColor::try_from(reader.read_u32(4)?)?;
        let text = reader.read_string(5)?;

        let subblock = reader.read_subblock(6)?;
        let rectangles = (0..reader.bit_reader.read_varuint()?)
            .into_iter()
            .map(|_| Rectangle::parse(reader))
            .collect::<Result<Vec<Rectangle>, ParseError>>()?;
        subblock.validate_size(reader)?;

        // 可选 color_rgba（tag 10, Byte4）：packed LE uint32 存 BGRA → 解成 (R,G,B,A)。
        let color_rgba = if reader.has_tag(10, TagType::Byte4)? {
            let packed = reader.read_u32(10)?;
            Some((
                ((packed >> 16) & 0xFF) as u8,
                ((packed >> 8) & 0xFF) as u8,
                (packed & 0xFF) as u8,
                ((packed >> 24) & 0xFF) as u8,
            ))
        } else {
            None
        };

        Ok(GlyphRange {
            start,
            length,
            text,
            color,
            color_rgba,
            rectangles,
        })
    }
}
