//! 最小只读 SQLite 文件读取器——纯 Rust，零 C 依赖。存在原因：`vocab.rs` 要读 KOReader 的
//! `vocabulary_builder.sqlite3`，`rusqlite`（bundled sqlite3 C 源码）在本项目的交叉编译环境下
//! 链接失败（sqlite3.c 调 `open64`/`stat64` 这类 glibc LFS64 符号，链去 musl 报符号缺失——项目
//! 现有交叉工具链是"glibc 头文件编译 + musl 链接"，只对不碰 libc 文件 I/O 的纯计算 C 代码（如
//! `ring`）成立，SQLite 真要读文件就不行），2026-09-16 用户拍板手写只读解析器绕开，用真实
//! sqlite3（`rusqlite` 降级成本 crate 的 host-only dev-dependency，生产二进制不链它）造多规模
//! fixture 差分测试代替真机验证（本轮没有真机可用）。
//!
//! 只实现读表所需的最小子集：文件头 + table b-tree（interior/leaf，index b-tree 不需要）+ 溢出页 +
//! record 变长编码，格式细节见 <https://www.sqlite.org/fileformat2.html>（稳定十余年没变过的公开格式）。
//! 不支持：WAL（假设已 checkpoint 进主文件）、UTF-16 编码（KOReader/lua-ljsqlite3 默认建库是 UTF-8，
//! 遇到非 UTF-8 直接报错，不猜）、`WITHOUT ROWID` 表。
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Value::Text(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_int(&self) -> Option<i64> {
        match self {
            Value::Int(n) => Some(*n),
            _ => None,
        }
    }
}

pub struct Db {
    bytes: Vec<u8>,
    page_size: usize,
    usable_size: usize,
    utf8: bool,
}

/// 变长整数（1-9 字节）：前 8 字节每字节高位是延续位、低 7 位有效；第 9 字节（如果走到）全 8 位有效。
fn read_varint(buf: &[u8], pos: usize) -> Result<(i64, usize), String> {
    let mut v: i64 = 0;
    let mut p = pos;
    for i in 0..9 {
        let b = *buf.get(p).ok_or("varint 越界")?;
        p += 1;
        if i == 8 {
            v = (v << 8) | b as i64;
            break;
        }
        v = (v << 7) | (b & 0x7f) as i64;
        if b & 0x80 == 0 {
            break;
        }
    }
    Ok((v, p))
}

fn be_u16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}
fn be_u32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

fn serial_type_size(t: i64) -> usize {
    match t {
        0 | 8 | 9 | 10 | 11 => 0,
        1 => 1,
        2 => 2,
        3 => 3,
        4 => 4,
        5 => 6,
        6 | 7 => 8,
        n if n >= 12 && n % 2 == 0 => ((n - 12) / 2) as usize,
        n if n >= 13 => ((n - 13) / 2) as usize,
        _ => 0,
    }
}

fn sign_extend(bytes: &[u8]) -> i64 {
    let neg = bytes[0] & 0x80 != 0;
    let mut buf = [if neg { 0xffu8 } else { 0 }; 8];
    buf[8 - bytes.len()..].copy_from_slice(bytes);
    i64::from_be_bytes(buf)
}

fn decode_value(t: i64, b: &[u8], utf8: bool) -> Result<Value, String> {
    Ok(match t {
        0 => Value::Null,
        8 => Value::Int(0),
        9 => Value::Int(1),
        10 | 11 => Value::Null, // 保留类型，正常数据不会出现
        1..=5 => Value::Int(sign_extend(b)),
        6 => Value::Int(i64::from_be_bytes(b.try_into().map_err(|_| "int64 长度不对")?)),
        7 => Value::Real(f64::from_be_bytes(b.try_into().map_err(|_| "float64 长度不对")?)),
        n if n >= 12 && n % 2 == 0 => Value::Blob(b.to_vec()),
        n if n >= 13 => {
            if !utf8 {
                return Err("只支持 UTF-8 编码的库".into());
            }
            Value::Text(String::from_utf8(b.to_vec()).map_err(|e| format!("文本不是合法 UTF-8: {e}"))?)
        }
        _ => Value::Null,
    })
}

/// 一条 record（payload）→ 按列顺序的值列表。
fn record_values(bytes: &[u8], utf8: bool) -> Result<Vec<Value>, String> {
    let (header_len, mut p) = read_varint(bytes, 0)?;
    let header_end = header_len as usize;
    let mut types = Vec::new();
    while p < header_end {
        let (t, np) = read_varint(bytes, p)?;
        types.push(t);
        p = np;
    }
    let mut body = header_end;
    let mut out = Vec::with_capacity(types.len());
    for t in types {
        let sz = serial_type_size(t);
        let slice = bytes.get(body..body + sz).ok_or("record body 越界")?;
        out.push(decode_value(t, slice, utf8)?);
        body += sz;
    }
    Ok(out)
}

impl Db {
    pub fn open(path: &Path) -> Result<Db, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("读 {} 失败: {e}", path.display()))?;
        if bytes.len() < 100 || &bytes[0..16] != b"SQLite format 3\0" {
            return Err("不是 SQLite 3 数据库文件".into());
        }
        let raw_page_size = be_u16(&bytes[16..18]) as usize;
        let page_size = if raw_page_size == 1 { 65536 } else { raw_page_size };
        let reserved = bytes[20] as usize;
        let encoding = be_u32(&bytes[56..60]);
        Ok(Db { bytes, page_size, usable_size: page_size - reserved, utf8: matches!(encoding, 0 | 1) })
    }

    fn page(&self, page_no: u32) -> Result<&[u8], String> {
        let start = (page_no as usize - 1) * self.page_size;
        self.bytes.get(start..start + self.page_size).ok_or_else(|| format!("page {page_no} 超出文件范围"))
    }

    /// 组一条 cell 的完整 payload：本地部分 + （如果溢出）溢出页链。公式见文件格式文档"Table B-Tree
    /// Leaf Cell"一节：X=U-35 是不溢出的最大本地长度；超过时本地留 K 或 M 字节，其余进溢出页。
    fn read_payload(&self, page: &[u8], mut pos: usize, payload_len: usize) -> Result<Vec<u8>, String> {
        let u = self.usable_size;
        let x = u - 35;
        if payload_len <= x {
            return page.get(pos..pos + payload_len).map(<[u8]>::to_vec).ok_or_else(|| "cell payload 越界".to_string());
        }
        let m = ((u - 12) * 32 / 255) - 23;
        let k = m + (payload_len - m) % (u - 4);
        let local = if k <= x { k } else { m };
        let mut buf = page.get(pos..pos + local).map(<[u8]>::to_vec).ok_or("cell 本地部分越界")?;
        pos += local;
        let mut next = be_u32(page.get(pos..pos + 4).ok_or("溢出页指针越界")?);
        let mut remaining = payload_len - local;
        while next != 0 && remaining > 0 {
            let op = self.page(next)?;
            let take = remaining.min(self.usable_size - 4);
            buf.extend_from_slice(op.get(4..4 + take).ok_or("溢出页内容越界")?);
            remaining -= take;
            next = be_u32(&op[0..4]);
        }
        Ok(buf)
    }

    /// 递归收集一棵 table b-tree（interior→leaf）全部叶子 cell 的 (rowid, payload)。
    fn walk_table(&self, page_no: u32, out: &mut Vec<(i64, Vec<u8>)>) -> Result<(), String> {
        let page = self.page(page_no)?;
        let hdr = if page_no == 1 { 100 } else { 0 };
        let page_type = page[hdr];
        let ncells = be_u16(&page[hdr + 3..hdr + 5]) as usize;
        let (is_leaf, is_interior) = (page_type == 0x0d, page_type == 0x05);
        if !is_leaf && !is_interior {
            return Err(format!("page {page_no}: 不是 table b-tree page（type=0x{page_type:02x}，可能是 index b-tree，本读取器不支持）"));
        }
        let ptr_start = hdr + if is_interior { 12 } else { 8 };
        for i in 0..ncells {
            let off = ptr_start + i * 2;
            let cell = be_u16(page.get(off..off + 2).ok_or("cell 指针越界")?) as usize;
            if is_interior {
                let child = be_u32(page.get(cell..cell + 4).ok_or("interior cell 越界")?);
                self.walk_table(child, out)?;
            } else {
                let (payload_len, p1) = read_varint(page, cell)?;
                let (rowid, p2) = read_varint(page, p1)?;
                out.push((rowid, self.read_payload(page, p2, payload_len as usize)?));
            }
        }
        if is_interior {
            let right = be_u32(&page[hdr + 8..hdr + 12]);
            self.walk_table(right, out)?;
        }
        Ok(())
    }

    fn find_table_root(&self, table_name: &str) -> Result<u32, String> {
        let mut cells = Vec::new();
        self.walk_table(1, &mut cells)?;
        for (_, payload) in &cells {
            // sqlite_master 固定 5 列 (type, name, tbl_name, rootpage, sql)，SQLite 内部 schema 永不变。
            let v = record_values(payload, self.utf8)?;
            if v.first().and_then(Value::as_text) == Some("table") && v.get(1).and_then(Value::as_text) == Some(table_name) {
                return v.get(3).and_then(Value::as_int).map(|n| n as u32).ok_or_else(|| format!("{table_name} 的 rootpage 不是整数"));
            }
        }
        Err(format!("sqlite_master 里没有表 {table_name}"))
    }

    /// 一张表的全部行，按列声明顺序给值。`rowid_alias_col`：这张表如果用 `INTEGER PRIMARY KEY` 声明
    /// 过某一列（该列就是 rowid 的别名），record 里那一列存的是 NULL 占位，真值要从 cell 的 rowid 取——
    /// 调用方知道自己的 schema，这里不做通用 SQL 解析去猜。
    pub fn table_rows(&self, table_name: &str, rowid_alias_col: Option<usize>) -> Result<Vec<Vec<Value>>, String> {
        let root = self.find_table_root(table_name)?;
        let mut cells = Vec::new();
        self.walk_table(root, &mut cells)?;
        let mut rows = Vec::with_capacity(cells.len());
        for (rowid, payload) in cells {
            let mut vals = record_values(&payload, self.utf8)?;
            if let Some(i) = rowid_alias_col {
                if let Some(slot) = vals.get_mut(i) {
                    *slot = Value::Int(rowid);
                }
            }
            rows.push(vals);
        }
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn write_db(path: &Path, sql: &[&str]) {
        let conn = Connection::open(path).unwrap();
        for s in sql {
            conn.execute_batch(s).unwrap();
        }
    }

    #[test]
    fn reads_small_single_page_table_including_nulls_and_rowid_alias() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("a.sqlite3");
        write_db(
            &db,
            &["CREATE TABLE title (id INTEGER PRIMARY KEY, name TEXT, filter INTEGER DEFAULT 1);
               INSERT INTO title (id, name) VALUES (1, '人骨拼图');
               INSERT INTO title (id, name, filter) VALUES (7, 'Foo', 0);"],
        );
        let d = Db::open(&db).unwrap();
        let rows = d.table_rows("title", Some(0)).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0][0].as_int(), Some(1), "INTEGER PRIMARY KEY 列的真值要从 rowid 取，record 里存的是 NULL 占位");
        assert_eq!(rows[0][1].as_text(), Some("人骨拼图"));
        assert_eq!(rows[0][2], Value::Int(1), "默认值");
        assert_eq!(rows[1][0].as_int(), Some(7));
        assert_eq!(rows[1][2], Value::Int(0));
    }

    #[test]
    fn reads_nullable_text_columns_as_null_not_empty_string() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("b.sqlite3");
        write_db(&db, &["CREATE TABLE t (word TEXT PRIMARY KEY, ctx TEXT); INSERT INTO t (word) VALUES ('lucid');"]);
        let d = Db::open(&db).unwrap();
        let rows = d.table_rows("t", None).unwrap();
        assert_eq!(rows[0][0].as_text(), Some("lucid"), "TEXT PRIMARY KEY 不是 rowid 别名，正常存在 record 里");
        assert_eq!(rows[0][1], Value::Null);
    }

    #[test]
    fn reads_large_table_forcing_interior_pages_and_preserves_order_independent_completeness() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("c.sqlite3");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE words (word TEXT PRIMARY KEY, n INTEGER)").unwrap();
        for i in 0..3000 {
            conn.execute("INSERT INTO words (word, n) VALUES (?1, ?2)", rusqlite::params![format!("word-{i:05}"), i]).unwrap();
        }
        drop(conn);
        let d = Db::open(&db).unwrap();
        let rows = d.table_rows("words", None).unwrap();
        assert_eq!(rows.len(), 3000, "表大到必然产生 interior page，全部行都要收全");
        let mut ns: Vec<i64> = rows.iter().map(|r| r[1].as_int().unwrap()).collect();
        ns.sort_unstable();
        assert_eq!(ns, (0..3000).collect::<Vec<_>>(), "3000 个整数一个不漏一个不重");
    }

    #[test]
    fn reads_overflow_pages_for_long_text_byte_for_byte() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("d.sqlite3");
        let long: String = "上下文超长测试 ".repeat(2000); // 远超一页（默认 4096），必然溢出多页
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch("CREATE TABLE t (word TEXT PRIMARY KEY, ctx TEXT)").unwrap();
        conn.execute("INSERT INTO t (word, ctx) VALUES ('w', ?1)", rusqlite::params![long]).unwrap();
        drop(conn);
        let d = Db::open(&db).unwrap();
        let rows = d.table_rows("t", None).unwrap();
        assert_eq!(rows[0][1].as_text(), Some(long.as_str()), "跨多个溢出页的文本要原样拼回来");
    }

    #[test]
    fn matches_the_real_vocabulary_builder_schema_end_to_end() {
        let t = tempfile::tempdir().unwrap();
        let db = t.path().join("vocab.sqlite3");
        write_db(
            &db,
            &["CREATE TABLE IF NOT EXISTS vocabulary (word TEXT NOT NULL UNIQUE, title_id INTEGER, create_time INTEGER NOT NULL, review_time INTEGER, due_time INTEGER NOT NULL, review_count INTEGER NOT NULL DEFAULT 0, prev_context TEXT, next_context TEXT, streak_count INTEGER NOT NULL DEFAULT 0, highlight TEXT, PRIMARY KEY(word));
               CREATE TABLE IF NOT EXISTS title (id INTEGER NOT NULL UNIQUE, name TEXT UNIQUE, filter INTEGER NOT NULL DEFAULT 1, PRIMARY KEY(id));
               INSERT INTO title (id, name) VALUES (1, '人骨拼图');
               INSERT INTO vocabulary (word, title_id, create_time, due_time, prev_context, next_context, highlight) VALUES ('ephemeral', 1, 100, 200, 'that was an ', ' moment.', 'ephemeral');"],
        );
        let d = Db::open(&db).unwrap();
        let vocab = d.table_rows("vocabulary", None).unwrap();
        assert_eq!(vocab[0][0].as_text(), Some("ephemeral"));
        assert_eq!(vocab[0][1].as_int(), Some(1), "title_id");
        assert_eq!(vocab[0][6].as_text(), Some("that was an "), "prev_context");
        let titles = d.table_rows("title", Some(0)).unwrap();
        assert_eq!(titles[0][0].as_int(), Some(1));
        assert_eq!(titles[0][1].as_text(), Some("人骨拼图"));
    }
}
