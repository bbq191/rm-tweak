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

use std::io::{Cursor, Read};

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
        let (nav, ncx) = read_toc_texts(epub);
        BookMap::new(parse_epubindex(epubindex), Toc::parse(nav.as_deref(), ncx.as_deref()))
    }

    /// 该页所在的 spine 文件（起始页 ≤ page 的最后一条）。
    pub fn section_of(&self, page: usize) -> Option<&Section> {
        self.sections.iter().take_while(|s| (s.start_page as usize) <= page).last()
    }

    pub fn chapter_of(&self, page: usize) -> Option<Chapter<'_>> {
        let sec = self.section_of(page)?;
        let i = self.toc.entries.iter().position(|e| e.file == sec.file)?;
        let top = self.toc.top_ancestor(i);
        let index = self.toc.entries.iter().take(top).filter(|e| e.level == 1).count();
        let e = &self.toc.entries[i];
        Some(Chapter { index, title: &self.toc.entries[top].title, subhead: (e.level >= 2).then_some(e.title.as_str()) })
    }

    /// 全书 1 级章列表 [(index, title)]。
    pub fn chapters(&self) -> Vec<(usize, &str)> {
        self.toc.entries.iter().filter(|e| e.level == 1).enumerate().map(|(i, e)| (i, e.title.as_str())).collect()
    }
}

/// 在 EPUB zip 里找 (nav.xhtml, toc.ncx) 文本；缺的为 None。
pub fn read_toc_texts(epub: &[u8]) -> (Option<String>, Option<String>) {
    let Ok(mut ar) = zip::ZipArchive::new(Cursor::new(epub)) else { return (None, None) };
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
    let mut read = |name: Option<String>| -> Option<String> {
        let mut s = String::new();
        ar.by_name(&name?).ok()?.read_to_string(&mut s).ok()?;
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

    // 原测试 real_fixture_renggu_flat_ncx 依赖真机《人骨拼圖》fixture（testdata/renggu/ 的
    // book.epubindex + toc.ncx）验证扁平 ncx 解析，公开发行版不带这份含真实版权小说原文的夹具，
    // 整个删掉；私有开发仓库这份测试原样保留。
}
