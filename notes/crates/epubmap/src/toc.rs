//! EPUB 目录 → 带层级的条目表。两种源（策略）产出同一形状：`nav.xhtml`（EPUB3，嵌套 `<ol>`）优先，`toc.ncx`（EPUB2，
//! 嵌套 `navPoint`）退回；两者都按"标签事件流 + 深度栈"解析，不用完整 XML 解析器（设备端体积）。
use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TocEntry {
    /// href 的 basename（去 `#片段`）。
    pub file: String,
    pub title: String,
    /// 1 = 章（顶层），2 = 节，……
    pub level: u8,
    /// 上一级条目的下标。
    pub parent: Option<usize>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Toc {
    pub entries: Vec<TocEntry>,
}

impl Toc {
    /// 有 nav 先用 nav，nav 为空再用 ncx。
    pub fn parse(nav: Option<&str>, ncx: Option<&str>) -> Toc {
        if let Some(t) = nav.map(Toc::from_nav).filter(|t| !t.entries.is_empty()) {
            return t;
        }
        ncx.map(Toc::from_ncx).unwrap_or_default()
    }

    /// `nav.xhtml`：`<ol>` 深度即层级；条目 = `<a href>`。
    pub fn from_nav(text: &str) -> Toc {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r#"(?is)(<ol\b)|(</ol\s*>)|<a[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap());
        let mut b = Builder::default();
        for c in re.captures_iter(text) {
            if c.get(1).is_some() {
                b.depth += 1;
            } else if c.get(2).is_some() {
                b.leave();
            } else {
                b.push(c.get(3).map_or("", |m| m.as_str()), c.get(4).map_or("", |m| m.as_str()));
            }
        }
        b.finish()
    }

    /// `toc.ncx`：`navPoint` 嵌套深度即层级；标题取 `<text>`，文件取 `<content src>`。
    pub fn from_ncx(text: &str) -> Toc {
        static RE: OnceLock<Regex> = OnceLock::new();
        let re = RE.get_or_init(|| Regex::new(r#"(?is)(<navpoint\b)|(</navpoint\s*>)|<text>(.*?)</text>|<content[^>]*src="([^"]+)""#).unwrap());
        let mut b = Builder::default();
        let mut pending_title = String::new();
        for c in re.captures_iter(text) {
            if c.get(1).is_some() {
                b.depth += 1;
            } else if c.get(2).is_some() {
                b.leave();
            } else if let Some(t) = c.get(3) {
                pending_title = t.as_str().to_string();
            } else if let Some(src) = c.get(4) {
                b.push(src.as_str(), &pending_title);
            }
        }
        b.finish()
    }

    /// 条目 i 的 1 级祖先下标（自己是 1 级就是自己）。
    pub fn top_ancestor(&self, mut i: usize) -> usize {
        while let Some(p) = self.entries[i].parent {
            i = p;
        }
        i
    }
}

/// 深度栈 → 条目表：`depth` 是当前嵌套层数，`last_at[d]` 是深度 d 最近一个条目的下标（作为深度 d+1 的父）。
#[derive(Default)]
struct Builder {
    depth: usize,
    entries: Vec<TocEntry>,
    last_at: Vec<Option<usize>>,
}

impl Builder {
    fn leave(&mut self) {
        if self.depth > 0 {
            self.depth -= 1;
        }
        self.last_at.truncate(self.depth.saturating_sub(1).max(0) + 1);
    }
    fn push(&mut self, href: &str, raw_title: &str) {
        let file = href.split('#').next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
        let title = strip_tags(raw_title);
        if file.is_empty() || title.is_empty() {
            return;
        }
        let level = self.depth.max(1);
        let parent = if level >= 2 { self.last_at.get(level - 2).copied().flatten() } else { None };
        if self.entries.iter().any(|e| e.file == file) {
            return; // 同文件多个锚点（#片段）只记首个，页粒度分不开
        }
        self.entries.push(TocEntry { file, title, level: level as u8, parent });
        if self.last_at.len() < level {
            self.last_at.resize(level, None);
        }
        self.last_at[level - 1] = Some(self.entries.len() - 1);
    }
    fn finish(self) -> Toc {
        Toc { entries: self.entries }
    }
}

/// 去标签、折叠空白（标题里可能夹 `<span>`）。
fn strip_tags(s: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nav_nesting_and_parents() {
        let t = Toc::from_nav(r#"<ol><li><a href="x/a.xhtml">A</a><ol><li><a href="b.xhtml#p1"><span>B</span> 1</a></li><li><a href="b.xhtml#p2">B2</a></li></ol></li><li><a href="c.xhtml">C</a></li></ol>"#);
        let v: Vec<(&str, &str, u8, Option<usize>)> = t.entries.iter().map(|e| (e.file.as_str(), e.title.as_str(), e.level, e.parent)).collect();
        assert_eq!(v, vec![("a.xhtml", "A", 1, None), ("b.xhtml", "B 1", 2, Some(0)), ("c.xhtml", "C", 1, None)]);
        assert_eq!(t.top_ancestor(1), 0);
    }

    #[test]
    fn ncx_nesting() {
        let t = Toc::from_ncx(r#"<navMap><navPoint><navLabel><text>Part I</text></navLabel><content src="p1.xhtml"/>
            <navPoint><navLabel><text>Ch 1</text></navLabel><content src="c1.xhtml"/></navPoint></navPoint>
            <navPoint><navLabel><text>Part II</text></navLabel><content src="p2.xhtml"/></navPoint></navMap>"#);
        let v: Vec<(&str, u8, Option<usize>)> = t.entries.iter().map(|e| (e.title.as_str(), e.level, e.parent)).collect();
        assert_eq!(v, vec![("Part I", 1, None), ("Ch 1", 2, Some(0)), ("Part II", 1, None)]);
        assert_eq!(Toc::parse(Some("<ol></ol>"), Some(r#"<navPoint><text>X</text><content src="x.xhtml"/></navPoint>"#)).entries.len(), 1, "nav 空退回 ncx");
    }
}
