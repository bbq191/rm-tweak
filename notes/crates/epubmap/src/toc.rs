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
        let re = RE.get_or_init(|| Regex::new(r#"(?is)(<ol\b)|(</ol\s*>)|(<li\b)|<a[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#).unwrap());
        let mut b = Builder::default();
        for c in re.captures_iter(text) {
            if c.get(1).is_some() {
                b.depth += 1;
            } else if c.get(2).is_some() {
                b.leave();
            } else if c.get(3).is_some() {
                b.new_item();
            } else {
                b.push(c.get(4).map_or("", |m| m.as_str()), c.get(5).map_or("", |m| m.as_str()));
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
                b.new_item();
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

    /// 条目 i 的顶层祖先下标（没有父条目的就是自己）。
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
        self.last_at.truncate(self.depth.saturating_sub(1) + 1);
    }
    /// 新的 `<li>`/`<navPoint>` 开始：这一层还没有条目。不清的话，标签是 `<span>`（没有链接）的分组项，其子条目会
    /// 挂到上一个兄弟条目下面（《前言》下面冒出第一部的各章）。
    fn new_item(&mut self) {
        if let Some(slot) = self.depth.checked_sub(1).and_then(|d| self.last_at.get_mut(d)) {
            *slot = None;
        }
    }
    fn set_last(&mut self, level: usize, idx: usize) {
        if self.last_at.len() < level {
            self.last_at.resize(level, None);
        }
        self.last_at[level - 1] = Some(idx);
    }
    fn push(&mut self, href: &str, raw_title: &str) {
        let file = href.split('#').next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
        let title = strip_tags(raw_title);
        if file.is_empty() || title.is_empty() {
            return;
        }
        let level = self.depth.max(1);
        let parent = if level >= 2 { self.last_at.get(level - 2).copied().flatten() } else { None };
        if let Some(existing) = self.entries.iter().position(|e| e.file == file) {
            // 同文件多个锚点（#片段）只记首个，页粒度分不开；这一层的子条目挂到已记的那条下面。
            self.set_last(level, existing);
            return;
        }
        self.entries.push(TocEntry { file, title, level: level as u8, parent });
        self.set_last(level, self.entries.len() - 1);
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

    /// 分组项是 `<span>`（没有链接）时，其子条目不能挂到上一个兄弟下面；它们成为顶层条目。
    #[test]
    fn nav_span_group_children_do_not_attach_to_previous_sibling() {
        let t = Toc::from_nav(r#"<ol><li><a href="pre.xhtml">前言</a></li><li><span>第一部</span><ol><li><a href="c1.xhtml">第一章</a></li><li><a href="c2.xhtml">第二章</a></li></ol></li></ol>"#);
        let v: Vec<(&str, u8, Option<usize>)> = t.entries.iter().map(|e| (e.title.as_str(), e.level, e.parent)).collect();
        assert_eq!(v, vec![("前言", 1, None), ("第一章", 2, None), ("第二章", 2, None)]);
        // 同文件的重复锚点被合并时，子条目挂到已记的那条下面（不会因为父条目被跳过而变成顶层）。
        let t = Toc::from_nav(r#"<ol><li><a href="p1.xhtml">第一部</a><ol><li><a href="p1.xhtml#c1">第一章</a><ol><li><a href="s1.xhtml">一节</a></li></ol></li></ol></li></ol>"#);
        let v: Vec<(&str, Option<usize>)> = t.entries.iter().map(|e| (e.title.as_str(), e.parent)).collect();
        assert_eq!(v, vec![("第一部", None), ("一节", Some(0))]);
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
