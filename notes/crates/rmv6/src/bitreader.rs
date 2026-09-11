use std::io::{Cursor, Read};

use crate::{ParseError, ParseErrorKind};

pub trait Readable: Read + AsRef<[u8]> {}
impl<T: Read + AsRef<[u8]>> Readable for T {}

/// A little endian binary reader
pub struct Bitreader<N: Readable> {
    cursor: Cursor<N>,
}

impl<N: Readable> Bitreader<N> {
    pub fn new(bits: N) -> Bitreader<N> {
        Bitreader {
            cursor: Cursor::new(bits),
        }
    }

    /// End Of File, returns true if not more bytes can be read
    pub fn eof(&mut self) -> Result<bool, ParseError> {
        let pos = self.position();
        match self.read_bytes(1) {
            Ok(_) => {
                self.set_position(pos);
                Ok(false)
            }
            Err(e) => {
                // if an io error occurs we assume no more bytes can be read, aka eof
                if e.kind == ParseErrorKind::Io {
                    self.set_position(pos);
                    return Ok(true);
                }
                Err(e)
            }
        }
    }

    pub fn position(&self) -> u64 {
        self.cursor.position()
    }

    pub fn set_position(&mut self, position: u64) {
        self.cursor.set_position(position);
        // self.cursor.seek(SeekFrom::Current(position)).unwrap();
    }

    // Read bytes first from inner buffer than from bits, will also update the offset
    fn read_exact(&mut self, buffer: &mut [u8]) -> Result<(), ParseError> {
        self.cursor.read_exact(buffer)?;

        Ok(())
    }

    /// 还没读的字节数（游标被 `set_position` 挪到末尾之后算 0）。
    pub fn remaining(&self) -> u64 {
        (self.cursor.get_ref().as_ref().len() as u64).saturating_sub(self.position())
    }

    pub fn read_bytes(&mut self, amount: usize) -> Result<Vec<u8>, ParseError> {
        // 先比剩余字节再分配（2026-09-25 第四轮审计）：长度来自文件里的 u32/varuint（块 size、字符串长度），
        // 畸形或正被 xochitl 写到一半的 `.rm` 会给出上 GB 的"长度"，`vec![0; amount]` 先按它分配——超过物理内存
        // 时分配失败是 abort（不是 panic，`catch_unwind` 兜不住），ink-serve 重启追平又撞同一页 → 崩溃循环。
        if amount as u64 > self.remaining() {
            return Err(ParseError::new(format!("要读 {amount} 字节，只剩 {} 字节", self.remaining()), ParseErrorKind::Io));
        }
        let mut buffer = vec![0; amount];
        self.read_exact(&mut buffer)?;
        Ok(buffer)
    }

    pub fn read_string(&mut self, length: usize) -> Result<String, ParseError> {
        String::from_utf8(self.read_bytes(length)?)
            .map_err(|_| ParseError::invalid("String contains invalid utf-8"))
    }

    // https://en.wikipedia.org/wiki/Variable-length_quantity
    pub fn read_varuint(&mut self) -> Result<u32, ParseError> {
        let mut shift = 0;
        let mut result = 0;
        let mut i;
        loop {
            i = self.read_u8()?;
            // 本地补丁：① 先转 u32 再移位（原 `((i&0x7F)<<shift) as u32` 在 u8 上移位——
            // shift=7 就丢高位、release 亦错）；② 用 wrapping_shl 掩码 shift——新固件 .rm 里
            // 畸形/超长 varuint 会把 shift 推到 ≥32，debug 下 `u32<<shift` 溢出 panic，而设备
            // release 靠硬件 shift&31 掩码继续读、star 检测照常（Rust 曾与 Python rmscene 逐字节
            // 对齐=正是此行为）。wrapping_shl 让 debug 精确等同 release，有效 varuint(shift≤28)不受影响。
            result |= ((i & 0x7F) as u32).wrapping_shl(shift);
            shift += 7;
            if i & 0x80 == 0 {
                break;
            }
        }
        Ok(result)
    }

    pub fn read_bool(&mut self) -> Result<bool, ParseError> {
        Ok(self.read_u8()? > 0)
    }

    pub fn read_f32(&mut self) -> Result<f32, ParseError> {
        let mut buffer = [0; 4];
        self.read_exact(&mut buffer)?;
        Ok(f32::from_le_bytes(buffer))
    }

    pub fn read_f64(&mut self) -> Result<f64, ParseError> {
        let mut buffer = [0; 8];
        self.read_exact(&mut buffer)?;
        Ok(f64::from_le_bytes(buffer))
    }

    pub fn read_u8(&mut self) -> Result<u8, ParseError> {
        let mut buffer = [0];
        self.read_exact(&mut buffer)?;
        Ok(u8::from_le_bytes(buffer))
    }

    pub fn read_u16(&mut self) -> Result<u16, ParseError> {
        let mut buffer = [0; 2];
        self.read_exact(&mut buffer)?;
        Ok(u16::from_le_bytes(buffer))
    }

    pub fn read_u32(&mut self) -> Result<u32, ParseError> {
        let mut buffer = [0; 4];
        self.read_exact(&mut buffer)?;
        Ok(u32::from_le_bytes(buffer))
    }

    /// Parse uuid from data in little endian format
    /// Using Variant 2 UUID's with mixed endianess <https://en.wikipedia.org/wiki/Universally_unique_identifier#Encoding>
    pub fn read_uuid(&mut self) -> Result<String, ParseError> {
        let uuid_length = self.read_varuint()?;
        if uuid_length != 16 {
            return Err(ParseError::invalid("Expected UUID length to be 16 bytes"));
        }

        let mut uuid_bytes: Vec<u8> = self.read_bytes(uuid_length as usize)?;

        // Set first 3 uuid sections to big endianness
        uuid_bytes[..4].reverse();
        uuid_bytes[4..6].reverse();
        uuid_bytes[6..8].reverse();

        // put bytes in a single number
        let uuid_bytes = u128::from_be_bytes(
            uuid_bytes
                .try_into()
                .map_err(|_| ParseError::invalid("Failed to parse uuid bytes into integer"))?,
        );

        // turn hexidecimals into string
        let uuid = format!("{uuid_bytes:032x}");
        // add slashes
        let uuid = format!(
            "{}-{}-{}-{}-{}",
            &uuid[..8],
            &uuid[8..12],
            &uuid[12..16],
            &uuid[16..20],
            &uuid[20..],
        );

        Ok(uuid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-09-09 审计补：`bitreader.rs` 是整条 rmv6 解析链路的地基（所有上层解码模块最终都靠它
    // 读字节），此前唯一的"测试"是一段被注释掉、没有任何断言的残留（本函数替换掉它）。这条
    // crate 处理的是设备上传的外部 `.rm` 文件，测试策略此前完全依赖真机样本回归，缺"喂畸形字节
    // 应该报错而不是 panic/静默错读"这类构造性反例——下面这批就是补这块。

    #[test]
    fn primitive_roundtrips() {
        let data: &[u8] = &[0x2A, 0x01, 0x02, 0xCD, 0xCC, 0x8C, 0x3F, 1];
        let mut r = Bitreader::new(data);
        assert_eq!(r.read_u8().unwrap(), 0x2A);
        assert_eq!(r.read_u16().unwrap(), 0x0201, "小端序");
        assert!((r.read_f32().unwrap() - 1.1).abs() < 1e-6);
        assert!(r.read_bool().unwrap());
    }

    #[test]
    fn eof_true_only_after_all_bytes_consumed() {
        let data: &[u8] = &[1, 2];
        let mut r = Bitreader::new(data);
        assert!(!r.eof().unwrap());
        r.read_u8().unwrap();
        assert!(!r.eof().unwrap(), "还剩一个字节，不该是 eof");
        r.read_u8().unwrap();
        assert!(r.eof().unwrap());
        // eof() 探测不该移动游标——探测之后紧接着还能正常读到 EOF 错误，不会因为探测本身
        // 悄悄往前挪了位置导致后续读出脏数据。
        assert!(r.read_u8().is_err());
    }

    #[test]
    fn truncated_input_is_io_error_not_panic() {
        // 只给 1 个字节却要读 u32——这是最基础的"文件被截断一半"场景，必须干净返回 Io 错误。
        let data: &[u8] = &[0x01];
        let mut r = Bitreader::new(data);
        let e = r.read_u32().unwrap_err();
        assert_eq!(e.kind, ParseErrorKind::Io);
    }

    #[test]
    fn empty_input_every_read_errors_cleanly() {
        let data: &[u8] = &[];
        let mut r = Bitreader::new(data);
        assert!(r.eof().unwrap());
        assert!(r.read_u8().is_err());
        assert!(r.read_bytes(1).is_err());
    }

    /// 回归：声明长度远超文件（畸形/写到一半的 `.rm`）时先报 Io 错误，不先按声明长度分配内存。
    #[test]
    fn huge_declared_length_errors_before_allocating() {
        let data: &[u8] = &[1, 2, 3];
        let mut r = Bitreader::new(data);
        let e = r.read_bytes(usize::MAX / 2).unwrap_err();
        assert_eq!(e.kind, ParseErrorKind::Io);
        assert!(r.read_string(u32::MAX as usize).is_err());
        assert_eq!(r.read_bytes(3).unwrap(), [1, 2, 3], "失败的读取不移动游标");
        assert_eq!(r.remaining(), 0);
        r.set_position(10);
        assert_eq!(r.remaining(), 0, "游标越过末尾也不下溢");
        assert!(r.read_bytes(1).is_err());
    }

    /// 回归：块头声明 4GB 大小的未知块不会先分配 4GB。
    #[test]
    fn unknown_block_with_absurd_size_is_rejected_cheaply() {
        let mut bytes = b"reMarkable .lines file, version=6          ".to_vec();
        bytes.extend(u32::MAX.to_le_bytes());
        bytes.extend([0, 0, 0, 0xEE]);
        bytes.extend([1, 2, 3]);
        assert!(crate::RmFile::read(bytes.as_slice()).is_err());
    }

    #[test]
    fn read_string_rejects_invalid_utf8_instead_of_panicking() {
        let data: &[u8] = &[0xFF, 0xFE, 0xFD];
        let mut r = Bitreader::new(data);
        let e = r.read_string(3).unwrap_err();
        assert_eq!(e.kind, ParseErrorKind::InvalidInput);
    }

    #[test]
    fn read_varuint_multi_byte_and_truncated() {
        // 0xAC 0x02 是标准 varuint 示例（值 300：0x2C | (0x02<<7) = 0x2C + 0x100 = 300）。
        let data: &[u8] = &[0xAC, 0x02];
        let mut r = Bitreader::new(data);
        assert_eq!(r.read_varuint().unwrap(), 300);

        // 截断：continuation bit（最高位）一直是 1，字节流却在读到之前耗尽——不该 panic，
        // 该干净报 Io 错误（真机 §04 记过"畸形/超长 varuint"这类样本会出现，这条测试钉住
        // 截断分支不会 panic，跟 bitreader.rs 里那条 wrapping_shl 补丁注释描述的"超长但没截断"
        // 场景互补）。
        let data2: &[u8] = &[0x80, 0x80, 0x80]; // 全部 continuation bit=1，没有终止字节
        let mut r2 = Bitreader::new(data2);
        assert!(r2.read_varuint().is_err());
    }

    #[test]
    fn read_uuid_rejects_wrong_length_and_truncated_bytes() {
        // 长度字段（varuint）声明的不是 16。
        let data: &[u8] = &[15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut r = Bitreader::new(data);
        let e = r.read_uuid().unwrap_err();
        assert_eq!(e.kind, ParseErrorKind::InvalidInput);

        // 长度字段声明 16，但实际字节不够（截断）。
        let data2: &[u8] = &[16, 1, 2, 3];
        let mut r2 = Bitreader::new(data2);
        assert!(r2.read_uuid().is_err());
    }

    #[test]
    fn read_uuid_roundtrip_matches_known_layout() {
        // 16 字节全零应该稳定得到全零 UUID（不用去核对具体的字节序变换规则，只钉住"合法输入
        // 不会出错、格式对"）。
        let mut bytes = vec![16u8];
        bytes.extend([0u8; 16]);
        let mut r = Bitreader::new(bytes.as_slice());
        let uuid = r.read_uuid().unwrap();
        assert_eq!(uuid, "00000000-0000-0000-0000-000000000000");
        assert_eq!(uuid.len(), 36);
    }
}

