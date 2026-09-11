//! 指纹：FNV-1a 64（无依赖、跨平台稳定）。簇指纹按笔画 id 排序 + 点数 + 量化包围盒算——同一片手写不管文件重写多少次
//! 指纹不变；补一笔/擦一笔就变。条目 id 也用它（创建时一次性算，之后不重算）。

pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

pub fn hex(h: u64) -> String {
    format!("{h:016x}")
}

/// 包围盒 (x0, y0, x1, y1)。
pub type Bbox4 = (f32, f32, f32, f32);
/// 簇指纹输入的一笔：(笔画 id 字符串, 点数, 包围盒)。
pub type StrokeSig<'a> = (&'a str, usize, Bbox4);

/// 簇指纹输入：`StrokeSig` 列表，顺序无关。
pub fn cluster_hash(strokes: &[StrokeSig<'_>]) -> String {
    let mut items: Vec<String> = strokes.iter().map(|(id, n, b)| format!("{id}|{n}|{:.0}|{:.0}|{:.0}|{:.0}", b.0, b.1, b.2, b.3)).collect();
    items.sort();
    hex(fnv1a(items.join(";").as_bytes()))
}

/// 条目 id：书 uuid + 页 id + 首笔 id（创建时的最小笔画 id）。
pub fn entry_id(book: &str, page: &str, first_stroke: &str) -> String {
    hex(fnv1a(format!("{book}/{page}/{first_stroke}").as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_order_independent() {
        assert_eq!(hex(fnv1a(b"")), "cbf29ce484222325");
        let a = cluster_hash(&[("1:5", 10, (1.0, 2.0, 30.0, 40.0)), ("1:6", 3, (5.0, 5.0, 6.0, 6.0))]);
        let b = cluster_hash(&[("1:6", 3, (5.4, 5.0, 6.0, 6.0)), ("1:5", 10, (1.0, 2.0, 30.0, 40.0))]);
        assert_eq!(a, b, "顺序无关、坐标量化到整数");
        assert_ne!(a, cluster_hash(&[("1:5", 11, (1.0, 2.0, 30.0, 40.0)), ("1:6", 3, (5.0, 5.0, 6.0, 6.0))]), "补一点就变");
        assert_eq!(entry_id("b", "p", "1:5"), entry_id("b", "p", "1:5"));
        assert_ne!(entry_id("b", "p", "1:5"), entry_id("b", "q", "1:5"));
    }
}
