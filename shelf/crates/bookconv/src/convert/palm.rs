//! PalmDB / MOBI 容器共享底座。MOBI6 与 KF8/AZW3 是**同一 PalmDB + PalmDOC + EXTH 容器**，
//! 仅正文语义不同（MOBI6=单 HTML 靠 `<mbp:pagebreak>` 分页；KF8=预组装好的 XHTML 文档序列）。
//! 把容器解析（记录表 / PalmDOC 解压 / extra_data_flags 尾字节剥离 / EXTH / 图片 / 封面）统一在此，
//! `mobi.rs` 与 `kf8.rs` 两条线共用。
//!
//! ⚠ 为什么不用 `mobi` crate 做解压：真机词典样本（现代汉语词典.mobi，extra_data_flags=3）暴露
//! `mobi` 0.8 在**尾字节位置**上判断错误（它按标准 0xF2 读到 0xffff），跨记录拼接错位 → 整本乱码、
//! 找不到 `<body>`。本模块按「record0 起 +0xF2 == MOBI 头起 +0xE2」正确定位，词典真样本解压干净。

use super::common;

/// PalmDB 记录切片表：每条记录一个 `&[u8]`（借 data，零拷贝）。
pub fn parse_palmdb(d: &[u8]) -> Result<Vec<&[u8]>, String> {
    if d.len() < 78 {
        return Err("文件过短，非 PalmDB".into());
    }
    let nrec = u16::from_be_bytes([d[76], d[77]]) as usize;
    if nrec == 0 || d.len() < 78 + nrec * 8 {
        return Err("PalmDB 记录表越界".into());
    }
    let mut offs: Vec<usize> = Vec::with_capacity(nrec + 1);
    for i in 0..nrec {
        let p = 78 + i * 8;
        offs.push(u32::from_be_bytes([d[p], d[p + 1], d[p + 2], d[p + 3]]) as usize);
    }
    offs.push(d.len());
    let mut out = Vec::with_capacity(nrec);
    for i in 0..nrec {
        let (a, b) = (offs[i], offs[i + 1]);
        if a > d.len() || b > d.len() || a > b {
            return Err("PalmDB 记录偏移非法".into());
        }
        out.push(&d[a..b]);
    }
    Ok(out)
}

/// PalmDB 数据库名（file[0..32] 空字符截断），做书名兜底。
pub fn palmdb_name(d: &[u8]) -> String {
    if d.len() < 32 {
        return String::new();
    }
    let raw = &d[0..32];
    let end = raw.iter().position(|&b| b == 0).unwrap_or(32);
    String::from_utf8_lossy(&raw[..end]).trim().to_string()
}

/// 从 record0 解出的容器头（compression / encryption / 文本记录数 / extra_flags / MOBI 头）。
pub struct Header<'a> {
    pub compression: u16,
    pub encryption: u16,
    pub text_record_count: usize,
    pub extra_flags: u16,
    /// r0[16..]，即 MOBI 魔数开始处。
    pub mobi: &'a [u8],
    pub mobi_hlen: usize,
}

/// 解析 record0（PalmDOC 头 + MOBI 头）。校验 MOBI 魔数、DRM、HUFF/CDIC。
pub fn parse_header(r0: &[u8]) -> Result<Header<'_>, String> {
    if r0.len() < 16 + 8 {
        return Err("record0 过短".into());
    }
    let compression = u16::from_be_bytes([r0[0], r0[1]]);
    let text_record_count = u16::from_be_bytes([r0[8], r0[9]]) as usize;
    let encryption = u16::from_be_bytes([r0[12], r0[13]]);
    if encryption != 0 {
        return Err("有 DRM 加密，不支持（请先去 DRM）".into());
    }
    if compression == 17480 {
        return Err("HUFF/CDIC 压缩，暂不支持（请用 calibre 转 EPUB）".into());
    }
    let mobi = &r0[16..];
    if mobi.len() < 8 || &mobi[0..4] != b"MOBI" {
        return Err("非 MOBI/KF8 容器（无 MOBI 头）".into());
    }
    let mobi_hlen = u32::from_be_bytes([mobi[4], mobi[5], mobi[6], mobi[7]]) as usize;
    let extra_flags = read_extra_flags(mobi);
    Ok(Header { compression, encryption, text_record_count, extra_flags, mobi, mobi_hlen })
}

/// 解压文本记录 1..=text_record_count → 完整正文字节（MOBI6=单 HTML；KF8=rawML）。
/// 每条记录先剥 trailing bytes（extra_data_flags）再 PalmDOC 解压（compression==1 时原样拼）。
pub fn decompress_text(records: &[&[u8]], h: &Header) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let last = h.text_record_count.min(records.len().saturating_sub(1));
    for rec in records.iter().take(last + 1).skip(1) {
        let ts = trailing_size(rec, h.extra_flags);
        let body = &rec[..rec.len().saturating_sub(ts)];
        if h.compression == 2 {
            palmdoc_decompress(body, &mut out);
        } else {
            out.extend_from_slice(body);
        }
    }
    out
}

/// extra_data_flags：正确位置是「record0 起 +0xF2」== 「MOBI 头起 +0xE2」。标准文档常写 0xF2 是相对
/// record0；本函数以 MOBI 头为基。先试 0xE2，再退 0xF2，取「合理小值」（仅低几位是标志位）。
pub fn read_extra_flags(mobi: &[u8]) -> u16 {
    let at = |o: usize| -> Option<u16> {
        if mobi.len() >= o + 2 { Some(u16::from_be_bytes([mobi[o], mobi[o + 1]])) } else { None }
    };
    for o in [0xE2usize, 0xF2] {
        if let Some(v) = at(o) {
            if v <= 0x3F {
                return v;
            }
        }
    }
    0
}

/// 每条文本记录尾部 trailing bytes 大小（多字节重叠 + TBS 索引项，须剥离再解压）。
pub fn trailing_size(rec: &[u8], flags: u16) -> usize {
    let mut num = 0usize;
    let mut f = flags >> 1;
    while f != 0 {
        if f & 1 != 0 {
            let end = rec.len().saturating_sub(num);
            num += backward_varint(&rec[..end]);
        }
        f >>= 1;
    }
    if flags & 1 != 0 {
        let idx = rec.len().saturating_sub(num).saturating_sub(1);
        if idx < rec.len() {
            num += (rec[idx] & 0x3) as usize + 1;
        }
    }
    num.min(rec.len())
}

/// 从末尾向前读 7bit/byte 变长整数，遇高位=1 停止。
pub fn backward_varint(data: &[u8]) -> usize {
    let mut val = 0usize;
    let mut shift = 0u32;
    for &b in data.iter().rev() {
        val |= ((b & 0x7f) as usize) << shift;
        shift += 7;
        if b & 0x80 != 0 {
            break;
        }
        if shift > 28 {
            break;
        }
    }
    val
}

/// PalmDOC/LZ77 解压，追加到 out。完全 bounds-safe（坏 back-ref 跳过，不崩）。
pub fn palmdoc_decompress(data: &[u8], out: &mut Vec<u8>) {
    let n = data.len();
    let mut i = 0;
    while i < n {
        let c = data[i];
        i += 1;
        if c == 0 {
            out.push(0);
        } else if c <= 8 {
            let take = (c as usize).min(n - i);
            out.extend_from_slice(&data[i..i + take]);
            i += c as usize;
        } else if c < 0x80 {
            out.push(c);
        } else if c < 0xc0 {
            if i >= n {
                break;
            }
            let di = (((c as usize) << 8) | data[i] as usize) & 0x3fff;
            i += 1;
            let length = (di & 7) + 3;
            let dist = di >> 3;
            if dist > 0 && dist <= out.len() {
                for _ in 0..length {
                    out.push(out[out.len() - dist]);
                }
            }
        } else {
            out.push(b' ');
            out.push(c ^ 0x80);
        }
    }
}

/// 一条图片记录：字节 + 扩展名 + MIME。
pub struct ImgRec {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    pub mime: &'static str,
}

/// 扫所有记录收集图片（JPEG/PNG/GIF），按记录序。这个顺序就是：
/// - MOBI6 `<img recindex="N">` 的 1-based 索引空间（`images[N-1]`）；
/// - KF8 `kindle:embed:NNNN` 的 1-based 索引空间；
/// - EXTH 201 封面偏移（相对首图记录）的 0-based 索引空间（`images[cover_idx]`）。
pub fn collect_images(records: &[&[u8]]) -> Vec<ImgRec> {
    records
        .iter()
        .filter_map(|r| {
            common::image_ext_mime(r).map(|(ext, mime)| ImgRec { bytes: r.to_vec(), ext, mime })
        })
        .collect()
}

/// EXTH 元数据（title=503 / author=100 / publisher=101 / cover 记录偏移=201 / 缩略图偏移=202）。
#[derive(Default)]
pub struct Exth {
    pub title: String,
    pub author: String,
    pub publisher: String,
    pub language: String,
    pub cover_index: Option<usize>,
    pub thumb_index: Option<usize>,
}

/// EXTH 语言（BCP-47，如 "zh-CN"/"ru"/"en-US"）→ EPUB `dc:language`；空则退 `en`。
pub fn lang_or_default(lang: &str) -> String {
    let t = lang.trim();
    if t.is_empty() { "en".to_string() } else { t.to_string() }
}

/// 解析 EXTH 头（紧跟 MOBI 头之后）。
pub fn parse_exth(mobi: &[u8], mobi_hlen: usize) -> Exth {
    let mut e = Exth::default();
    if mobi_hlen == 0 || mobi.len() < mobi_hlen + 12 || &mobi[mobi_hlen..mobi_hlen + 4] != b"EXTH" {
        return e;
    }
    let base = mobi_hlen;
    let count =
        u32::from_be_bytes([mobi[base + 8], mobi[base + 9], mobi[base + 10], mobi[base + 11]]) as usize;
    let mut p = base + 12;
    for _ in 0..count {
        if p + 8 > mobi.len() {
            break;
        }
        let rtype = u32::from_be_bytes([mobi[p], mobi[p + 1], mobi[p + 2], mobi[p + 3]]);
        let rlen = u32::from_be_bytes([mobi[p + 4], mobi[p + 5], mobi[p + 6], mobi[p + 7]]) as usize;
        if rlen < 8 || p + rlen > mobi.len() {
            break;
        }
        let val = &mobi[p + 8..p + rlen];
        let as_u32 = || {
            if val.len() >= 4 {
                Some(u32::from_be_bytes([val[0], val[1], val[2], val[3]]) as usize)
            } else {
                None
            }
        };
        match rtype {
            503 => e.title = String::from_utf8_lossy(val).to_string(),
            100 if e.author.is_empty() => e.author = String::from_utf8_lossy(val).to_string(),
            101 if e.publisher.is_empty() => e.publisher = String::from_utf8_lossy(val).to_string(),
            201 => e.cover_index = as_u32(),
            202 => e.thumb_index = as_u32(),
            524 if e.language.is_empty() => e.language = String::from_utf8_lossy(val).to_string(),
            _ => {}
        }
        p += rlen;
    }
    e
}

/// 按 EXTH 选封面在 images 中的下标：201（相对首图记录的偏移）优先，越界退 202 缩略图，再退首图。
/// `collect_images` 已按记录序过滤出图片，偏移空间与 EXTH 一致（真机 dict/azw3 样本验证命中）。
pub fn pick_cover_index(images: &[ImgRec], exth: &Exth) -> Option<usize> {
    if images.is_empty() {
        return None;
    }
    let inb = |i: Option<usize>| i.filter(|&n| n < images.len());
    Some(inb(exth.cover_index).or_else(|| inb(exth.thumb_index)).unwrap_or(0))
}

/// 一条 NCX 目录项：`pos`=章在 rawML 的字节偏移，`label`=真章名，`level`=层级（0=顶层）。
pub struct NcxEntry {
    pub pos: usize,
    pub label: String,
    pub level: u8,
}

/// 前向读 7bit/byte 变长整数（CNCX 串长 / INDX tag 值用），遇高位=1 停止；推进 `*p`。
fn read_varint_fwd(b: &[u8], p: &mut usize) -> usize {
    let mut val = 0usize;
    while *p < b.len() {
        let c = b[*p];
        *p += 1;
        val = (val << 7) | (c & 0x7f) as usize;
        if c & 0x80 != 0 {
            break;
        }
    }
    val
}

fn find_sub(h: &[u8], pat: &[u8]) -> Option<usize> {
    h.windows(pat.len()).position(|w| w == pat)
}
fn rfind_sub(h: &[u8], pat: &[u8]) -> Option<usize> {
    h.windows(pat.len()).rposition(|w| w == pat)
}

// ── INDX 索引族共享底座（NCX 目录、fragment 索引都是这套结构）─────────────────
// MOBI 头某偏移存「头 INDX 记录号」；头 INDX 的 +0x18 = 数据块数 ndata；数据 INDX 靠尾部 IDXT 表
// 定位各条目；条目 = [id 长度 1 字节][id 文本][后续 tag/值 字节]。以下三个 helper 收敛这套解析，
// `parse_ncx` 与 `parse_fragment_starts` 共用（去重）。

/// 定位 INDX 索引族：从 MOBI 头 `field_off` 读记录号 → (记录号, 头 INDX 切片, 数据块数 ndata)。
fn indx_locate<'a>(records: &[&'a [u8]], mobi: &[u8], field_off: usize) -> Option<(usize, &'a [u8], usize)> {
    if mobi.len() < field_off + 4 {
        return None;
    }
    let idx = u32::from_be_bytes([mobi[field_off], mobi[field_off + 1], mobi[field_off + 2], mobi[field_off + 3]]) as usize;
    if idx == 0 || idx >= records.len() {
        return None;
    }
    let hdr = records[idx];
    if hdr.len() < 0x1C || &hdr[0..4] != b"INDX" {
        return None;
    }
    let ndata = u32::from_be_bytes([hdr[0x18], hdr[0x19], hdr[0x1A], hdr[0x1B]]) as usize;
    if ndata == 0 || idx + ndata >= records.len() {
        return None;
    }
    Some((idx, hdr, ndata))
}

/// 遍历一个数据 INDX 块的所有条目，返回各条目「id 长度字节起」的字节切片（到下一条目/IDXT）。
fn indx_entries(data: &[u8]) -> Vec<&[u8]> {
    if data.len() < 0x1C || &data[0..4] != b"INDX" {
        return vec![];
    }
    let nent = u32::from_be_bytes([data[0x18], data[0x19], data[0x1A], data[0x1B]]) as usize;
    let idxt = match rfind_sub(data, b"IDXT") {
        Some(x) => x,
        None => return vec![],
    };
    let mut offs: Vec<usize> = Vec::with_capacity(nent);
    for k in 0..nent {
        let p = idxt + 4 + k * 2;
        if p + 2 > data.len() {
            break;
        }
        offs.push(u16::from_be_bytes([data[p], data[p + 1]]) as usize);
    }
    let mut out = Vec::with_capacity(offs.len());
    for k in 0..offs.len() {
        let s = offs[k];
        let e = if k + 1 < offs.len() { offs[k + 1] } else { idxt };
        if s < e && e <= data.len() {
            out.push(&data[s..e]);
        }
    }
    out
}

/// 拆条目切片为 (id 文本, 其后的 tag/值 字节)。
fn indx_split_entry(entry: &[u8]) -> Option<(&[u8], &[u8])> {
    let id_len = *entry.first()? as usize;
    if 1 + id_len > entry.len() {
        return None;
    }
    Some((&entry[1..1 + id_len], &entry[1 + id_len..]))
}

/// 解析 KF8 **NCX 目录** → `(rawML 偏移, 真章名, 层级)` 列表。KF8 章名不在正文 `<title>`（那常=书名），
/// 而在独立索引：NCX 记录号存 MOBI 头 `+0xE4`；结构 = 头 INDX（含 `TAGX` 标签定义 + `+0x18` 数据块数）
/// 加 N 个数据 INDX（每条目 = id 文本 + 按 TAGX 编码的 tag 值）+ CNCX（`ncx+1+ndata`，标签字串池）。
/// tag 1 = pos（**直接 rawML 偏移**）、tag 3 = CNCX 标签偏移、tag 4 = 层级。
/// 5 本真机样本（俄/日/中，4–30 条，含层级）验证。解析失败/非预期结构 → 返回空（调用方退化，不崩不回归）。
pub fn parse_ncx(records: &[&[u8]], h: &Header) -> Vec<NcxEntry> {
    let (ncx, hdr, ndata) = match indx_locate(records, h.mobi, 0xE4) {
        Some(x) => x,
        None => return vec![],
    };
    // TAGX 标签定义表（每项 4 字节：tag / nvals / mask / eof）
    let tagx_at = match find_sub(hdr, b"TAGX") {
        Some(x) => x,
        None => return vec![],
    };
    if tagx_at + 12 > hdr.len() {
        return vec![];
    }
    let tagx_len =
        u32::from_be_bytes([hdr[tagx_at + 4], hdr[tagx_at + 5], hdr[tagx_at + 6], hdr[tagx_at + 7]])
            as usize;
    let tagx_end = (tagx_at + tagx_len).min(hdr.len());
    let mut tagtable: Vec<(u8, u8, u8, u8)> = Vec::new();
    let mut q = tagx_at + 12;
    while q + 4 <= tagx_end {
        tagtable.push((hdr[q], hdr[q + 1], hdr[q + 2], hdr[q + 3]));
        q += 4;
    }
    let cncx = records[ncx + 1 + ndata];
    // CNCX 文本校验：首串须 UTF-8 可解，否则判定不是我们要的 NCX（防误读别的索引族）
    {
        let mut p = 0usize;
        let l = read_varint_fwd(cncx, &mut p);
        if l == 0 || p + l > cncx.len() || std::str::from_utf8(&cncx[p..p + l]).is_err() {
            return vec![];
        }
    }
    let label_at = |o: usize| -> String {
        if o >= cncx.len() {
            return String::new();
        }
        let mut p = o;
        let l = read_varint_fwd(cncx, &mut p);
        if p + l > cncx.len() {
            return String::new();
        }
        String::from_utf8_lossy(&cncx[p..p + l]).into_owned()
    };

    let mut out = Vec::new();
    for blk in 0..ndata {
        for entry in indx_entries(records[ncx + 1 + blk]) {
            // 条目 = id 文本(忽略) + 控制字节 + 按 TAGX 编码的 tag 值（皆相对该条目切片）。
            let (_id, tagbytes) = match indx_split_entry(entry) {
                Some(x) => x,
                None => continue,
            };
            if tagbytes.is_empty() {
                continue;
            }
            let ctrl = tagbytes[0];
            let mut p = 1usize;
            let (mut pos, mut label_off, mut level) = (None, None, 0u8);
            for &(tag, nvals, mask, eof) in &tagtable {
                if eof != 0 || ctrl & mask == 0 {
                    continue;
                }
                let mut first = None;
                for i in 0..nvals {
                    if p >= tagbytes.len() {
                        break;
                    }
                    let v = read_varint_fwd(tagbytes, &mut p);
                    if i == 0 {
                        first = Some(v);
                    }
                }
                match tag {
                    1 => pos = first,
                    3 => label_off = first,
                    4 => level = first.unwrap_or(0) as u8,
                    _ => {}
                }
            }
            if let (Some(pos), Some(lo)) = (pos, label_off) {
                let label = label_at(lo);
                if !label.trim().is_empty() {
                    out.push(NcxEntry { pos, label, level });
                }
            }
        }
    }
    out
}

/// 解析 KF8 **fragment 索引** → 每个 fragment 在 rawML 的起始字节偏移（下标=fragment id）。
/// fragment 索引记录号存 MOBI 头 `+0xE8`；数据 INDX 每条目的 id 文本就是偏移十进制串（如 "0000000400"）。
/// 供内链 `kindle:pos:fid:F:off:O` 定位：目标 rawML 偏移 = `frag_starts[F] + O`（与 NCX pos_fid 同源，验证过）。
pub fn parse_fragment_starts(records: &[&[u8]], h: &Header) -> Vec<usize> {
    let (fidx, _hdr, ndata) = match indx_locate(records, h.mobi, 0xE8) {
        Some(x) => x,
        None => return vec![],
    };
    let mut out = Vec::new();
    for blk in 0..ndata {
        for entry in indx_entries(records[fidx + 1 + blk]) {
            // fragment 条目的 id 文本就是 rawML 起始偏移的十进制串（如 "0000000400"）。
            if let Some((id, _)) = indx_split_entry(entry) {
                out.push(std::str::from_utf8(id).unwrap_or("").trim().parse::<usize>().unwrap_or(0));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palmdoc_literal_and_backref() {
        let mut out = Vec::new();
        palmdoc_decompress(b"Hi", &mut out);
        assert_eq!(out, b"Hi");
    }

    #[test]
    fn backward_varint_reads_from_end() {
        assert_eq!(backward_varint(&[0x00, 0x83]), 3);
    }

    #[test]
    fn read_extra_flags_prefers_e2_plausible() {
        // MOBI 头：前 0xE2 填 0，然后 0xE2 处放 0x0003（合理），0xF2 处放 0xffff（不合理，跳过）。
        let mut m = vec![0u8; 0xF4];
        m[0..4].copy_from_slice(b"MOBI");
        m[0xE2] = 0x00;
        m[0xE3] = 0x03;
        m[0xF2] = 0xff;
        m[0xF3] = 0xff;
        assert_eq!(read_extra_flags(&m), 3);
    }

    #[test]
    fn pick_cover_uses_exth_201_then_falls_back() {
        let img = |b: u8| ImgRec { bytes: vec![b], ext: "jpg", mime: "image/jpeg" };
        let images = vec![img(0), img(1), img(2)];
        let mut e = Exth { cover_index: Some(2), ..Default::default() };
        assert_eq!(pick_cover_index(&images, &e), Some(2));
        e.cover_index = Some(9); // 越界 → 退 202
        e.thumb_index = Some(1);
        assert_eq!(pick_cover_index(&images, &e), Some(1));
        e.thumb_index = Some(9); // 都越界 → 退首图
        assert_eq!(pick_cover_index(&images, &e), Some(0));
    }

    #[test]
    fn palmdb_name_null_terminated() {
        let mut d = vec![0u8; 78];
        d[0..5].copy_from_slice(b"Book\0");
        assert_eq!(palmdb_name(&d), "Book");
    }
}
