//! TTF/OTF 解析（rmsvc-core 共享）：`name` 表家族名（fc-scan 兜底）、魔数校验、CJK 覆盖率（cmap）。font-serve 与 koreader-serve 共用。
//! 取 nameID 16（Typographic Family）优先、否则 nameID 1；平台 3(Windows, UTF-16BE) 优先、否则 1(Mac Roman)。
use std::convert::TryInto;

fn u16be(b: &[u8], o: usize) -> Option<u16> {
    b.get(o..o + 2).map(|x| u16::from_be_bytes(x.try_into().unwrap()))
}
fn u32be(b: &[u8], o: usize) -> Option<u32> {
    b.get(o..o + 4).map(|x| u32::from_be_bytes(x.try_into().unwrap()))
}

/// 是否 TrueType/OpenType/TTC 魔数。
pub fn is_font(b: &[u8]) -> bool {
    matches!(b.get(0..4), Some(b"\x00\x01\x00\x00") | Some(b"OTTO") | Some(b"true") | Some(b"ttcf"))
}

/// 家族名；TTC 取第一个字体。
pub fn family_name(b: &[u8]) -> Option<String> {
    let base = if b.get(0..4) == Some(b"ttcf") { u32be(b, 12)? as usize } else { 0 };
    let num_tables = u16be(b, base + 4)? as usize;
    let mut name_off = None;
    for i in 0..num_tables {
        let rec = base + 12 + i * 16;
        if b.get(rec..rec + 4)? == b"name" {
            name_off = Some(u32be(b, rec + 8)? as usize);
            break;
        }
    }
    let n = name_off?;
    let count = u16be(b, n + 2)? as usize;
    let str_off = n + u16be(b, n + 4)? as usize;
    let mut best: Option<(u8, String)> = None; // (优先级, 名)
    for i in 0..count {
        let r = n + 6 + i * 12;
        let plat = u16be(b, r)?;
        let name_id = u16be(b, r + 6)?;
        let len = u16be(b, r + 8)? as usize;
        let off = u16be(b, r + 10)? as usize;
        if name_id != 1 && name_id != 16 {
            continue;
        }
        let raw = b.get(str_off + off..str_off + off + len)?;
        let s = match plat {
            3 | 0 => {
                let u: Vec<u16> = raw.chunks(2).filter(|c| c.len() == 2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
                String::from_utf16_lossy(&u)
            }
            1 => raw.iter().map(|&c| c as char).collect(),
            _ => continue,
        };
        let s = s.trim().to_string();
        if s.is_empty() {
            continue;
        }
        // 优先级：nameID16 > nameID1；平台 3 > 其它
        let pri = (if name_id == 16 { 2 } else { 0 }) + (if plat == 3 { 1 } else { 0 });
        if best.as_ref().map(|(p, _)| pri > *p).unwrap_or(true) {
            best = Some((pri, s));
        }
    }
    best.map(|(_, s)| s)
}


// ── CJK 覆盖率（cmap 解析）：判"是不是中文字体"+ 低覆盖上传警告 + 回退排序 ──────────────
/// 中文基本区（CJK Unified Ideographs）码位范围，共 20992 个。
pub const HAN_BMP_START: u32 = 0x4E00;
pub const HAN_BMP_END: u32 = 0x9FFF;
pub const HAN_BMP_TOTAL: usize = (HAN_BMP_END - HAN_BMP_START + 1) as usize;

fn find_table(b: &[u8], tag: &[u8; 4]) -> Option<usize> {
    let base = if b.get(0..4) == Some(b"ttcf") { u32be(b, 12)? as usize } else { 0 };
    let num = u16be(b, base + 4)? as usize;
    for i in 0..num {
        let rec = base + 12 + i * 16;
        if b.get(rec..rec + 4)? == tag {
            return Some(u32be(b, rec + 8)? as usize);
        }
    }
    None
}

/// 数中文基本区（U+4E00..=U+9FFF）里被 cmap 覆盖的码位数。解析失败返回 None。
/// 选子表：优先 format 12（platform 3/enc 10 全 Unicode），否则 format 4（3/1 BMP）。
pub fn han_bmp_coverage(b: &[u8]) -> Option<usize> {
    let cmap = find_table(b, b"cmap")?;
    let n = u16be(b, cmap + 2)? as usize;
    let mut best: Option<(u8, usize)> = None; // (优先级, 子表偏移)
    for i in 0..n {
        let r = cmap + 4 + i * 8;
        let plat = u16be(b, r)?;
        let enc = u16be(b, r + 2)?;
        let off = cmap + u32be(b, r + 4)? as usize;
        let fmt = u16be(b, off)?;
        let pri = match (plat, enc, fmt) {
            (3, 10, 12) | (0, _, 12) => 3,
            (3, 1, 4) | (0, _, 4) => 2,
            (_, _, 12) => 2,
            (_, _, 4) => 1,
            _ => continue,
        };
        if best.map(|(p, _)| pri > p).unwrap_or(true) {
            best = Some((pri, off));
        }
    }
    let off = best?.1;
    let mut count = 0usize;
    match u16be(b, off)? {
        4 => {
            let segx2 = u16be(b, off + 6)? as usize;
            let segc = segx2 / 2;
            let end_o = off + 14;
            let start_o = end_o + segx2 + 2;
            let delta_o = start_o + segx2;
            let range_o = delta_o + segx2;
            for s in 0..segc {
                let end = u16be(b, end_o + s * 2)? as u32;
                let start = u16be(b, start_o + s * 2)? as u32;
                if end < HAN_BMP_START || start > HAN_BMP_END {
                    continue;
                }
                let delta = u16be(b, delta_o + s * 2)?;
                let range = u16be(b, range_o + s * 2)? as usize;
                let lo = start.max(HAN_BMP_START);
                let hi = end.min(HAN_BMP_END);
                for c in lo..=hi {
                    let g = if range == 0 {
                        (c as u16).wrapping_add(delta)
                    } else {
                        let gi = range_o + s * 2 + range + (c - start) as usize * 2;
                        match u16be(b, gi) {
                            Some(0) | None => 0,
                            Some(v) => v.wrapping_add(delta),
                        }
                    };
                    if g != 0 {
                        count += 1;
                    }
                }
                // 恶意/损坏字体可堆几万个互相重叠、都盖满汉字区的段（每段最多 2 万次迭代 → 数亿次），也会数出 >100%。
                // 覆盖数不可能超过区内总码位，够数了就停。
                if count >= HAN_BMP_TOTAL {
                    break;
                }
            }
        }
        12 => {
            let ng = u32be(b, off + 12)? as usize;
            for g in 0..ng {
                let go = off + 16 + g * 12;
                let sc = u32be(b, go)?;
                let ec = u32be(b, go + 4)?;
                if ec < HAN_BMP_START || sc > HAN_BMP_END {
                    continue;
                }
                let lo = sc.max(HAN_BMP_START);
                let hi = ec.min(HAN_BMP_END);
                // 损坏的组 startCharCode > endCharCode：`hi - lo` 会下溢（debug panic、release 回绕成天文数字直接报满 100%）。
                if lo > hi {
                    continue;
                }
                count += (hi - lo + 1) as usize;
                if count >= HAN_BMP_TOTAL {
                    break;
                }
            }
        }
        _ => return None,
    }
    Some(count.min(HAN_BMP_TOTAL))
}

/// 覆盖率百分比（0..=100）。
pub fn han_coverage_pct(b: &[u8]) -> Option<u8> {
    han_bmp_coverage(b).map(|c| ((c * 100) / HAN_BMP_TOTAL) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 手搓最小 TTF：只有一张 name 表，两条记录（平台1 nameID1 "Foo"、平台3 nameID16 "Bar Family"）。
    fn tiny_font() -> Vec<u8> {
        let mut names: Vec<u8> = Vec::new();
        let s1 = b"Foo".to_vec();
        let s2: Vec<u8> = "Bar Family".encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        let count = 2u16;
        let string_offset = 6 + 12 * count as usize;
        names.extend_from_slice(&0u16.to_be_bytes());
        names.extend_from_slice(&count.to_be_bytes());
        names.extend_from_slice(&(string_offset as u16).to_be_bytes());
        for (plat, enc, nid, len, off) in [(1u16, 0u16, 1u16, s1.len() as u16, 0u16), (3, 1, 16, s2.len() as u16, s1.len() as u16)] {
            for v in [plat, enc, 0x409, nid, len, off] {
                names.extend_from_slice(&v.to_be_bytes());
            }
        }
        names.extend_from_slice(&s1);
        names.extend_from_slice(&s2);
        let mut f = Vec::new();
        f.extend_from_slice(b"\x00\x01\x00\x00");
        f.extend_from_slice(&1u16.to_be_bytes());
        f.extend_from_slice(&[0u8; 6]);
        let table_off = 12 + 16;
        f.extend_from_slice(b"name");
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&(table_off as u32).to_be_bytes());
        f.extend_from_slice(&(names.len() as u32).to_be_bytes());
        f.extend_from_slice(&names);
        f
    }


    /// 手搓一个带 format-4 cmap 的最小字体：覆盖 U+4E00..=U+4E0F（16 个汉字）。
    fn font_with_cmap() -> Vec<u8> {
        font_with_segments(&[(0x4E00, 0x4E0F)])
    }

    /// 同上，但段表可自定：`segs` 是 (start, end) 列表（idDelta 全取 1），末尾自动补 0xFFFF 结尾段。
    fn font_with_segments(segs: &[(u16, u16)]) -> Vec<u8> {
        // format 4：每段用 idDelta 映射到非零，[0xFFFF] 结尾段
        let seg_count = segs.len() as u16 + 1;
        let mut sub: Vec<u8> = Vec::new();
        sub.extend_from_slice(&4u16.to_be_bytes()); // format
        let len_pos = sub.len();
        sub.extend_from_slice(&0u16.to_be_bytes()); // length 占位
        sub.extend_from_slice(&0u16.to_be_bytes()); // language
        sub.extend_from_slice(&(seg_count * 2).to_be_bytes()); // segCountX2
        sub.extend_from_slice(&0u16.to_be_bytes()); // searchRange (不校验)
        sub.extend_from_slice(&0u16.to_be_bytes()); // entrySelector
        sub.extend_from_slice(&0u16.to_be_bytes()); // rangeShift
        for e in segs.iter().map(|s| s.1).chain([0xFFFF]) { sub.extend_from_slice(&e.to_be_bytes()); } // endCode
        sub.extend_from_slice(&0u16.to_be_bytes()); // reservedPad
        for st in segs.iter().map(|s| s.0).chain([0xFFFF]) { sub.extend_from_slice(&st.to_be_bytes()); } // startCode
        for _ in 0..seg_count { sub.extend_from_slice(&1u16.to_be_bytes()); } // idDelta（非零映射）
        for _ in 0..seg_count { sub.extend_from_slice(&0u16.to_be_bytes()); } // idRangeOffset=0
        let l = sub.len() as u16;
        sub[len_pos..len_pos + 2].copy_from_slice(&l.to_be_bytes());
        sfnt_with_cmap_sub(&sub)
    }

    /// format-12 cmap（`groups` = (startChar, endChar)），外面套同样的最小 sfnt。
    fn font_with_groups(groups: &[(u32, u32)]) -> Vec<u8> {
        let mut sub: Vec<u8> = Vec::new();
        sub.extend_from_slice(&12u16.to_be_bytes());
        sub.extend_from_slice(&0u16.to_be_bytes());
        sub.extend_from_slice(&((16 + 12 * groups.len()) as u32).to_be_bytes());
        sub.extend_from_slice(&0u32.to_be_bytes());
        sub.extend_from_slice(&(groups.len() as u32).to_be_bytes());
        for (st, en) in groups {
            sub.extend_from_slice(&st.to_be_bytes());
            sub.extend_from_slice(&en.to_be_bytes());
            sub.extend_from_slice(&1u32.to_be_bytes());
        }
        sfnt_with_cmap_sub(&sub)
    }

    fn sfnt_with_cmap_sub(sub: &[u8]) -> Vec<u8> {
        // cmap 头：1 子表，platform 3 enc 1
        let mut cmap: Vec<u8> = Vec::new();
        cmap.extend_from_slice(&0u16.to_be_bytes());
        cmap.extend_from_slice(&1u16.to_be_bytes());
        cmap.extend_from_slice(&3u16.to_be_bytes());
        cmap.extend_from_slice(&1u16.to_be_bytes());
        cmap.extend_from_slice(&12u32.to_be_bytes()); // 子表偏移=头后
        cmap.extend_from_slice(sub);
        // sfnt：1 张表 cmap
        let mut f = Vec::new();
        f.extend_from_slice(b"\x00\x01\x00\x00");
        f.extend_from_slice(&1u16.to_be_bytes());
        f.extend_from_slice(&[0u8; 6]);
        let table_off = 12 + 16;
        f.extend_from_slice(b"cmap");
        f.extend_from_slice(&0u32.to_be_bytes());
        f.extend_from_slice(&(table_off as u32).to_be_bytes());
        f.extend_from_slice(&(cmap.len() as u32).to_be_bytes());
        f.extend_from_slice(&cmap);
        f
    }

    #[test]
    fn counts_han_coverage_from_cmap() {
        let f = font_with_cmap();
        assert_eq!(han_bmp_coverage(&f), Some(16), "U+4E00..=U+4E0F 共 16 字");
        assert_eq!(han_coverage_pct(&f), Some(0), "16/20992 向下取整=0%");
        assert_eq!(han_bmp_coverage(b"\x00\x01\x00\x00"), None);
    }
    /// 恶意字体：几千个互相重叠、都盖满汉字区的段——覆盖数不能超过区内总码位（否则百分比溢出 u8），
    /// 也不能真去迭代 段数×2 万次。
    #[test]
    fn overlapping_segments_are_clamped_and_fast() {
        let segs = vec![(0x4E00u16, 0x9FFFu16); 3000];
        let f = font_with_segments(&segs);
        let t0 = std::time::Instant::now();
        assert_eq!(han_bmp_coverage(&f), Some(HAN_BMP_TOTAL));
        assert_eq!(han_coverage_pct(&f), Some(100));
        assert!(t0.elapsed() < std::time::Duration::from_secs(1), "够数即停，不做 3000×2 万次迭代");
    }
    /// 回归：format-12 里 start > end 的损坏组不再下溢（debug panic / release 报满 100%），跳过即可。
    #[test]
    fn format12_counts_groups_and_skips_inverted_ones() {
        assert_eq!(han_bmp_coverage(&font_with_groups(&[(0x4E00, 0x4E09)])), Some(10));
        assert_eq!(han_bmp_coverage(&font_with_groups(&[(0x5000, 0x4E10), (0x4E00, 0x4E00)])), Some(1));
    }

    #[test]
    fn parses_family_preferring_typographic_windows_name() {
        let f = tiny_font();
        assert!(is_font(&f));
        assert_eq!(family_name(&f).as_deref(), Some("Bar Family"));
        assert!(!is_font(b"PK\x03\x04"));
        assert_eq!(family_name(b"\x00\x01\x00\x00"), None);
    }
}
