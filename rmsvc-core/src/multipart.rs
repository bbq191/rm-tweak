//! 流式 multipart/form-data 解析。每个 part 以 `Read` 形式交给调用方（边读边落盘），
//! 内存只保留一个读缓冲；多文件一次 POST 也不会把整个请求体读进内存（设备 MemoryMax 友好）。
//!
//! 用法：
//! ```ignore
//! let mut mp = MultipartReader::new(body, boundary);
//! while let Some(mut part) = mp.next_part()? {
//!     if let Some(fname) = part.filename.clone() { std::io::copy(&mut part, &mut file)?; }
//! }
//! ```
use std::io::{self, Read};

const CHUNK: usize = 64 * 1024;
const MAX_HEADER: usize = 16 * 1024;

/// 把一个 part（或任意 `Read`）流式写到 `dest` 文件，返回写入字节数（0＝空文件，是否算失败由调用方定）。
/// 单点化"建文件 + io::copy"这块——AssetUploadFlow 与 book-serve 的 spool 落盘共用（各自再决定空文件/归档语义）。
/// 错误不加前缀，调用方按场景包装（如 "接收失败: …"）。
pub fn receive_part_to<R: Read>(dest: &std::path::Path, part: &mut R) -> Result<u64, String> {
    let mut f = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    io::copy(part, &mut f).map_err(|e| e.to_string())
}

/// 从 Content-Type 里取 boundary。
pub fn boundary_of(content_type: &str) -> Option<String> {
    let ct = content_type.trim();
    if !ct.to_ascii_lowercase().starts_with("multipart/form-data") {
        return None;
    }
    ct.split(';').map(|s| s.trim()).find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k.trim().eq_ignore_ascii_case("boundary") {
            let v = v.trim().trim_matches('"');
            if v.is_empty() {
                None
            } else {
                Some(v.to_string())
            }
        } else {
            None
        }
    })
}

pub struct MultipartReader<R: Read> {
    r: R,
    delim: Vec<u8>, // "\r\n--<boundary>"
    buf: Vec<u8>,
    start: usize,
    eof: bool,
    done: bool,
    in_body: bool,
    started: bool,
}

/// 一个 part 的头信息；`Read` 读到该 part 结束返回 0。
pub struct Part<'a, R: Read> {
    pub name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    mp: &'a mut MultipartReader<R>,
}

impl<R: Read> MultipartReader<R> {
    pub fn new(r: R, boundary: &str) -> Self {
        let mut delim = b"\r\n--".to_vec();
        delim.extend_from_slice(boundary.as_bytes());
        MultipartReader { r, delim, buf: Vec::with_capacity(CHUNK * 2), start: 0, eof: false, done: false, in_body: false, started: false }
    }

    fn avail(&self) -> &[u8] {
        &self.buf[self.start..]
    }

    /// 读一块进缓冲；返回是否读到新数据。
    fn fill(&mut self) -> io::Result<bool> {
        if self.eof {
            return Ok(false);
        }
        if self.start > 0 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        // 读进栈上临时块再追加：避免每次 resize 清零 64KB（逐字节到达的流会被 memset 拖死）。
        let mut tmp = [0u8; CHUNK];
        let n = self.r.read(&mut tmp)?;
        self.buf.extend_from_slice(&tmp[..n]);
        if n == 0 {
            self.eof = true;
        }
        Ok(n > 0)
    }

    fn consume(&mut self, n: usize) {
        self.start += n;
    }

    fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
        if needle.is_empty() || hay.len() < needle.len() {
            return None;
        }
        hay.windows(needle.len()).position(|w| w == needle)
    }

    /// 跳过当前 part 未读完的 body（调用方没读完就要下一个 part）。
    fn drain_body(&mut self) -> io::Result<()> {
        let mut sink = io::sink();
        while self.in_body {
            let mut tmp = [0u8; 4096];
            let n = self.read_body(&mut tmp)?;
            if n == 0 {
                break;
            }
            io::Write::write_all(&mut sink, &tmp[..n])?;
        }
        Ok(())
    }

    /// 定位到首个 boundary（"--boundary" 可在流开头，无前导 CRLF）。
    fn skip_preamble(&mut self) -> io::Result<()> {
        let first = self.delim[2..].to_vec(); // "--boundary"
        loop {
            if let Some(i) = Self::find(self.avail(), &first) {
                self.consume(i + first.len());
                return self.after_boundary();
            }
            // 保留尾部可能的半个 boundary
            let keep = first.len().saturating_sub(1);
            let drop_n = self.avail().len().saturating_sub(keep);
            self.consume(drop_n);
            if !self.fill()? {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "multipart: 找不到起始 boundary"));
            }
        }
    }

    /// boundary 之后：`--` 收尾 / `\r\n` 进入下一 part 头。
    fn after_boundary(&mut self) -> io::Result<()> {
        while self.avail().len() < 2 {
            if !self.fill()? {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "multipart: boundary 后截断"));
            }
        }
        let a = self.avail();
        if &a[..2] == b"--" {
            self.done = true;
            self.consume(2);
        } else if &a[..2] == b"\r\n" {
            self.consume(2);
        } else {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "multipart: boundary 后非法字节"));
        }
        Ok(())
    }

    /// 下一个 part；流结束返回 None。
    pub fn next_part(&mut self) -> io::Result<Option<Part<'_, R>>> {
        if self.in_body {
            self.drain_body()?;
        }
        if !self.started {
            self.started = true;
            self.skip_preamble()?;
        }
        if self.done {
            return Ok(None);
        }
        // 读头到 \r\n\r\n
        let header_end = loop {
            if let Some(i) = Self::find(self.avail(), b"\r\n\r\n") {
                break i;
            }
            if self.avail().len() > MAX_HEADER {
                return Err(io::Error::new(io::ErrorKind::InvalidData, "multipart: part 头过长"));
            }
            if !self.fill()? {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "multipart: part 头截断"));
            }
        };
        let head = String::from_utf8_lossy(&self.avail()[..header_end]).to_string();
        self.consume(header_end + 4);
        let (name, filename, content_type) = parse_headers(&head);
        self.in_body = true;
        Ok(Some(Part { name, filename, content_type, mp: self }))
    }

    /// 读当前 part 的 body。到 part 结束返回 0 并推进到 boundary 之后。
    fn read_body(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if !self.in_body || out.is_empty() {
            return Ok(0);
        }
        loop {
            let dlen = self.delim.len();
            if let Some(i) = Self::find(self.avail(), &self.delim) {
                if i > 0 {
                    let n = i.min(out.len());
                    out[..n].copy_from_slice(&self.avail()[..n]);
                    self.consume(n);
                    return Ok(n);
                }
                // body 读尽：跨过 delim，看结尾/下一 part
                self.consume(dlen);
                self.in_body = false;
                self.after_boundary()?;
                return Ok(0);
            }
            // 未见 delim：除最后 dlen-1 字节外都是安全 body
            let safe = self.avail().len().saturating_sub(dlen - 1);
            if safe > 0 {
                let n = safe.min(out.len());
                out[..n].copy_from_slice(&self.avail()[..n]);
                self.consume(n);
                return Ok(n);
            }
            if !self.fill()? {
                return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "multipart: part body 未终止"));
            }
        }
    }
}

impl<R: Read> Read for Part<'_, R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.mp.read_body(out)
    }
}

fn header_param(line: &str, key: &str) -> Option<String> {
    line.split(';').map(|s| s.trim()).find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        if k.trim().eq_ignore_ascii_case(key) {
            Some(v.trim().trim_matches('"').to_string())
        } else {
            None
        }
    })
}

fn parse_headers(head: &str) -> (String, Option<String>, Option<String>) {
    let mut name = String::new();
    let mut filename = None;
    let mut ctype = None;
    for line in head.split("\r\n") {
        let Some((k, v)) = line.split_once(':') else { continue };
        let k = k.trim().to_ascii_lowercase();
        if k == "content-disposition" {
            name = header_param(v, "name").unwrap_or_default();
            // RFC 5987 filename* 优先；否则 filename
            filename = header_param(v, "filename*")
                .and_then(|s| s.rsplit_once("''").map(|(_, enc)| percent_decode(enc)))
                .or_else(|| header_param(v, "filename"));
        } else if k == "content-type" {
            ctype = Some(v.trim().to_string());
        }
    }
    (name, filename, ctype)
}

/// 百分号解码（filename*= 与查询串共用）。
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(if b[i] == b'+' { b' ' } else { b[i] });
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 百分号编码（RFC 3986 unreserved 之外全编）：查询串 / `?next=` 跳转共用，与 [`percent_decode`] 成对。
pub fn percent_encode(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => o.push(b as char),
            _ => o.push_str(&format!("%{:02X}", b)),
        }
    }
    o
}

/// 只取文件名的 basename（防路径穿越），空则用 default。
pub fn safe_basename(filename: &str, default: &str) -> String {
    let base = filename.replace('\\', "/");
    let base = base.rsplit('/').next().unwrap_or("").trim();
    if base.is_empty() || base == "." || base == ".." {
        default.to_string()
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(boundary: &str, parts: &[(&str, Option<&str>, &[u8])]) -> Vec<u8> {
        let mut v = Vec::new();
        for (name, fname, data) in parts {
            v.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"").as_bytes());
            if let Some(f) = fname {
                v.extend_from_slice(format!("; filename=\"{f}\"\r\nContent-Type: application/octet-stream").as_bytes());
            }
            v.extend_from_slice(b"\r\n\r\n");
            v.extend_from_slice(data);
            v.extend_from_slice(b"\r\n");
        }
        v.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
        v
    }

    /// 每次只吐 n 字节的 Reader，逼出跨块边界。
    struct Trickle<'a> {
        d: &'a [u8],
        pos: usize,
        n: usize,
    }
    impl Read for Trickle<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let k = self.n.min(out.len()).min(self.d.len() - self.pos);
            out[..k].copy_from_slice(&self.d[self.pos..self.pos + k]);
            self.pos += k;
            Ok(k)
        }
    }

    fn collect(mut mp: MultipartReader<impl Read>) -> Vec<(String, Option<String>, Vec<u8>)> {
        let mut out = Vec::new();
        while let Some(mut p) = mp.next_part().unwrap() {
            let mut d = Vec::new();
            p.read_to_end(&mut d).unwrap();
            out.push((p.name.clone(), p.filename.clone(), d));
        }
        out
    }

    #[test]
    fn boundary_extraction() {
        assert_eq!(boundary_of("multipart/form-data; boundary=abc"), Some("abc".into()));
        assert_eq!(boundary_of("multipart/form-data; boundary=\"q q\""), Some("q q".into()));
        assert_eq!(boundary_of("application/json"), None);
    }

    #[test]
    fn multiple_files_with_binary_crlf_bytes() {
        let bin: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).chain(b"\r\n--not-a-boundary\r\n".iter().copied()).collect();
        let b = body("XyZ", &[("target", None, b"native"), ("file", Some("a.epub"), &bin), ("file", Some("b.pdf"), b"%PDF")]);
        let got = collect(MultipartReader::new(&b[..], "XyZ"));
        assert_eq!(got.len(), 3);
        assert_eq!(got[0], ("target".into(), None, b"native".to_vec()));
        assert_eq!(got[1].1.as_deref(), Some("a.epub"));
        assert_eq!(got[1].2, bin);
        assert_eq!(got[2].2, b"%PDF");
    }

    #[test]
    fn boundary_split_across_read_chunks() {
        let big: Vec<u8> = vec![b'x'; 200_000];
        let b = body("bnd", &[("file", Some("x.bin"), &big), ("file", Some("y.bin"), b"yy")]);
        for n in [1usize, 3, 7, 64, 1000, 65537] {
            let got = collect(MultipartReader::new(Trickle { d: &b, pos: 0, n }, "bnd"));
            assert_eq!(got.len(), 2, "chunk {n}");
            assert_eq!(got[0].2.len(), 200_000, "chunk {n}");
            assert_eq!(got[1].2, b"yy");
        }
    }

    #[test]
    fn skipping_unread_part_still_reaches_next() {
        let b = body("q", &[("file", Some("a"), b"aaaa"), ("file", Some("b"), b"bb")]);
        let mut mp = MultipartReader::new(&b[..], "q");
        let p = mp.next_part().unwrap().unwrap();
        assert_eq!(p.filename.as_deref(), Some("a"));
        drop(p);
        let mut p2 = mp.next_part().unwrap().unwrap();
        let mut d = Vec::new();
        p2.read_to_end(&mut d).unwrap();
        assert_eq!(d, b"bb");
        assert!(mp.next_part().unwrap().is_none());
    }

    #[test]
    fn truncated_body_errors() {
        let mut b = body("q", &[("file", Some("a"), b"aaaa")]);
        b.truncate(b.len() - 8);
        let mut mp = MultipartReader::new(&b[..], "q");
        let mut p = mp.next_part().unwrap().unwrap();
        let mut d = Vec::new();
        assert!(p.read_to_end(&mut d).is_err());
    }

    #[test]
    fn filename_star_and_basename_safety() {
        let head = "Content-Disposition: form-data; name=\"file\"; filename=\"x.txt\"; filename*=UTF-8''%E4%B8%AD.epub";
        let (_, f, _) = parse_headers(head);
        assert_eq!(f.as_deref(), Some("中.epub"));
        assert_eq!(safe_basename("../../etc/passwd", "d"), "passwd");
        assert_eq!(safe_basename("C:\\x\\y.epub", "d"), "y.epub");
        assert_eq!(safe_basename("..", "d"), "d");
        assert_eq!(safe_basename("", "d"), "d");
    }

    #[test]
    fn percent_roundtrip() {
        assert_eq!(percent_encode("a b/中"), "a%20b%2F%E4%B8%AD");
        assert_eq!(percent_decode(&percent_encode("x=1&y=中 文")), "x=1&y=中 文");
    }
}
