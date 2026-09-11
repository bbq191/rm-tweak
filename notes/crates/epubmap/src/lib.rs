//! epubmap —— 设备 EPUB 的「页号 → 章 / 小节」映射（笔记线用来给每条勾画标章节、按章生成笔记本）。
//!
//! 两份数据源合一：
//! - **`<uuid>.epubindex`**（xochitl 导入时生成的二进制）：记录每个 spine 文件的**起始页**（[`index`]，格式事实剥离自旧
//!   `device-core::epubindex` 的逆向结论，3.28 真机再核：两张表、第一张的第一个 u32 是起始页）；
//! - **EPUB 自身目录**（[`toc`]：优先 `nav.xhtml` 的嵌套 `<ol>`，退回 `toc.ncx` 的嵌套 `navPoint`），统一成带层级的
//!   [`toc::TocEntry`] 列表。
//!
//! [`BookMap`] 把两者合起来：`chapter_of(page)` 给出该页所属的**章**（1 级祖先）与**小节**（本条目若 ≥2 级）。
//! 页号 0-based（封面=0），与 `.rm` 页文件在 `.content` `pages` 里的位置一致。
pub mod index;
pub mod toc;

pub use index::{parse_epubindex, Section};
pub use toc::{Toc, TocEntry};

use std::io::{Cursor, Read, Seek};

/// 某页所属的章节标签。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter<'a> {
    /// 1 级条目在目录里的序号（0-based；note-serve 用它编「第 N 章」文件名与顺序）。
    pub index: usize,
    /// 章标题（本条目是 1 级就是自己，否则是它的 1 级祖先）。
    pub title: &'a str,
    /// 小节标题（本条目 ≥2 级才有）。
    pub subhead: Option<&'a str>,
}

#[derive(Debug, Default)]
pub struct BookMap {
    pub sections: Vec<Section>,
    pub toc: Toc,
}

impl BookMap {
    pub fn new(sections: Vec<Section>, toc: Toc) -> BookMap {
        BookMap { sections, toc }
    }

    /// 从设备上的 `.epub` 字节 + `.epubindex` 字节直接建表（找不到目录 → toc 为空，只剩 section 粒度）。
    pub fn from_epub(epub: &[u8], epubindex: &[u8]) -> BookMap {
        BookMap::from_epub_reader(Cursor::new(epub), epubindex)
    }

    /// 同 [`Self::from_epub`]，但直接吃可 seek 的读端（如打开的 `.epub` 文件）：zip 只读中央目录和目录那一两个条目，
    /// 不必把整本书读进内存——大书（上传绕过 100MB 限制的那类）整本 `fs::read` 会顶破服务的 MemoryMax（2026-09-24）。
    pub fn from_epub_reader<R: Read + Seek>(epub: R, epubindex: &[u8]) -> BookMap {
        let (nav, ncx) = read_toc_texts_from(epub);
        BookMap::new(parse_epubindex(epubindex), Toc::parse(nav.as_deref(), ncx.as_deref()))
    }

    /// 该页所在的 spine 文件（起始页 ≤ page 的最后一条）。
    pub fn section_of(&self, page: usize) -> Option<&Section> {
        self.sections.iter().take_while(|s| (s.start_page as usize) <= page).last()
    }

    /// **章 = 没有父条目的顶层条目**（通常就是 1 级）。此前 `chapters()` 只收 1 级、`chapter_of` 却取顶层祖先：目录里
    /// 分组项是 `<span>`（没有链接）时，二级条目没有父条目，它的章号按"前面有几个 1 级"算、标题却是它自己——跟
    /// `chapters()` 对不上（下标越界或撞上别的章），这些条目在两处投影里找不到自己的章，永远生成/导出不出来
    /// （2026-09-25 第四轮审计）。两处现在用同一个判据；标准嵌套目录（每个二级都有 1 级父条目）结果不变。
    pub fn chapter_of(&self, page: usize) -> Option<Chapter<'_>> {
        let sec = self.section_of(page)?;
        let i = self.toc.entries.iter().position(|e| e.file == sec.file)?;
        let top = self.toc.top_ancestor(i);
        let index = self.toc.entries.iter().take(top).filter(|e| e.parent.is_none()).count();
        Some(Chapter { index, title: &self.toc.entries[top].title, subhead: (i != top).then_some(self.toc.entries[i].title.as_str()) })
    }

    /// 全书章列表 [(index, title)]（顶层条目，见 [`Self::chapter_of`]）。
    pub fn chapters(&self) -> Vec<(usize, &str)> {
        self.toc.entries.iter().filter(|e| e.parent.is_none()).enumerate().map(|(i, e)| (i, e.title.as_str())).collect()
    }
}

/// 在 EPUB zip 里找 (nav.xhtml, toc.ncx) 文本；缺的为 None。
pub fn read_toc_texts(epub: &[u8]) -> (Option<String>, Option<String>) {
    read_toc_texts_from(Cursor::new(epub))
}

/// 同 [`read_toc_texts`]，吃可 seek 的读端。
pub fn read_toc_texts_from<R: Read + Seek>(epub: R) -> (Option<String>, Option<String>) {
    let Ok(mut ar) = zip::ZipArchive::new(epub) else { return (None, None) };
    let (mut nav_name, mut ncx_name) = (None, None);
    for i in 0..ar.len() {
        let Ok(f) = ar.by_index(i) else { continue };
        let n = f.name().to_string();
        let l = n.to_ascii_lowercase();
        if l.ends_with(".ncx") {
            ncx_name.get_or_insert(n);
        } else if l.ends_with("nav.xhtml") || (l.contains("nav") && l.ends_with(".xhtml")) {
            nav_name.get_or_insert(n);
        }
    }
    // 目录文件设读取上限：解压后的大小由 zip 自己声明，坏书/恶意书可以声称极大（笔记服务 MemoryMax=128M）。
    const TOC_MAX: u64 = 16 << 20;
    let mut read = |name: Option<String>| -> Option<String> {
        let mut s = String::new();
        ar.by_name(&name?).ok()?.take(TOC_MAX).read_to_string(&mut s).ok()?;
        Some(s)
    };
    let nav = read(nav_name);
    let ncx = read(ncx_name);
    (nav, ncx)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAV_NESTED: &str = r#"<nav epub:type="toc"><ol>
      <li><a href="chap_0002.xhtml">Dedication</a></li>
      <li><a href="chap_0003.xhtml">Book One</a>
        <ol><li><a href="chap_0004.xhtml">Chapter One</a></li><li><a href="chap_0005.xhtml">Chapter Two</a></li></ol>
      </li>
      <li><a href="chap_0014.xhtml">Book Two</a>
        <ol><li><a href="chap_0015.xhtml">Chapter Eleven</a></li></ol>
      </li>
    </ol></nav>"#;

    #[test]
    fn nested_nav_gives_chapter_and_subhead() {
        let secs = vec![
            Section { file: "chap_0002.xhtml".into(), start_page: 1 },
            Section { file: "chap_0003.xhtml".into(), start_page: 5 },
            Section { file: "chap_0004.xhtml".into(), start_page: 6 },
            Section { file: "chap_0005.xhtml".into(), start_page: 20 },
            Section { file: "chap_0014.xhtml".into(), start_page: 40 },
            Section { file: "chap_0015.xhtml".into(), start_page: 41 },
        ];
        let m = BookMap::new(secs, Toc::parse(Some(NAV_NESTED), None));
        assert_eq!(m.chapter_of(0), None, "封面页无 section");
        assert_eq!(m.chapter_of(2), Some(Chapter { index: 0, title: "Dedication", subhead: None }));
        assert_eq!(m.chapter_of(5), Some(Chapter { index: 1, title: "Book One", subhead: None }));
        assert_eq!(m.chapter_of(10), Some(Chapter { index: 1, title: "Book One", subhead: Some("Chapter One") }));
        assert_eq!(m.chapter_of(25), Some(Chapter { index: 1, title: "Book One", subhead: Some("Chapter Two") }));
        assert_eq!(m.chapter_of(99), Some(Chapter { index: 2, title: "Book Two", subhead: Some("Chapter Eleven") }));
        assert_eq!(m.chapters(), vec![(0, "Dedication"), (1, "Book One"), (2, "Book Two")]);
    }

    /// 回归：分组项是 `<span>` 的目录，章号与 `chapters()` 必须对得上（此前越界，条目永远投影不出去）。
    #[test]
    fn chapter_index_matches_chapters_when_groups_have_no_link() {
        let nav = r#"<ol><li><a href="pre.xhtml">前言</a></li><li><span>第一部</span><ol><li><a href="c1.xhtml">第一章</a></li><li><a href="c2.xhtml">第二章</a></li></ol></li></ol>"#;
        let secs = vec![Section { file: "pre.xhtml".into(), start_page: 1 }, Section { file: "c1.xhtml".into(), start_page: 3 }, Section { file: "c2.xhtml".into(), start_page: 9 }];
        let m = BookMap::new(secs, Toc::parse(Some(nav), None));
        assert_eq!(m.chapters(), vec![(0, "前言"), (1, "第一章"), (2, "第二章")]);
        for page in [1, 4, 10] {
            let c = m.chapter_of(page).unwrap();
            assert_eq!(m.chapters()[c.index].1, c.title, "第 {page} 页：章号与章表一致");
            assert_eq!(c.subhead, None);
        }
    }

    /// 从文件读端建表与整本字节建表结果一致（ingest 改走文件读端，不再整本读进内存）。
    #[test]
    fn reader_and_bytes_give_same_map() {
        use std::io::Write;
        let mut buf = Cursor::new(Vec::new());
        {
            let mut w = zip::ZipWriter::new(&mut buf);
            let o = zip::write::SimpleFileOptions::default();
            w.start_file("mimetype", o).unwrap();
            w.write_all(b"application/epub+zip").unwrap();
            w.start_file("OEBPS/big.bin", o).unwrap();
            w.write_all(&vec![7u8; 1 << 20]).unwrap();
            w.start_file("OEBPS/nav.xhtml", o).unwrap();
            w.write_all(NAV_NESTED.as_bytes()).unwrap();
            w.finish().unwrap();
        }
        let bytes = buf.into_inner();
        let dir = std::env::temp_dir().join(format!("epubmap-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("b.epub");
        std::fs::write(&p, &bytes).unwrap();
        let a = BookMap::from_epub(&bytes, &[]);
        let b = BookMap::from_epub_reader(std::io::BufReader::new(std::fs::File::open(&p).unwrap()), &[]);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(a.chapters(), vec![(0, "Dedication"), (1, "Book One"), (2, "Book Two")]);
        assert_eq!(a.chapters(), b.chapters());
    }

    // 原测试 real_fixture_renggu_flat_ncx 依赖真机《人骨拼圖》fixture（testdata/renggu/ 的
    // book.epubindex + toc.ncx）验证扁平 ncx 解析，公开发行版不带这份含真实版权小说原文的夹具，
    // 整个删掉；私有开发仓库这份测试原样保留（上面 reader_and_bytes_give_same_map 在私有仓库也用这份
    // 夹具，公开版改用内联 nav 构造）。
}
