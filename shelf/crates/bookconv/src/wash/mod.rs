//! 清洗层（对标 host `wash_epub.sh` 的 Calibre 规则，2026-09-03 移植；白皮书 §03i）。作用于**解包后的条目表**，
//! 由 `optimize::optimize_epub_with` 在优化器各遍之前调用，host CLI `epub-optimize` 与设备 book-serve 走同一份。
//!
//! 规则（与 Calibre 参数一一对应）：
//! 1. 伪 DRM 剥离（= `strip_pseudo_drm.py`）：`META-INF/encryption.xml` 只列样式/字体/脚本 → 丢弃这些文件 +
//!    encryption.xml + OPF manifest 项；列了正文/图片/导航 = 真 DRM → **报错停下**。
//! 2. CSS 锁剥离（**只剥字号/字体锁**：独立 .css、`<style>`、`style=""` 三处剥 `font-family`/`font-size`/`font`；
//!    补优化器 `strip_font_locks` 只剥内联字体锁的缺口）：2026-09-17 前 `color`/`background-color`/`text-align`
//!    也在剥离名单里，那是照抄 Calibre `--filter-css` 的通用参数、不是针对 xochitl 验证过的必要行为——EPUB 线
//!    原则明确要求保留原书颜色/加粗等元素样式，只解锁字号，故收窄。`background`/`background-image` 仍然剥
//!    （见规则⑧，xochitl 平铺背景图盖正文是坐实的渲染 bug，跟字号锁无关，不能一起放开）。
//! 3. 边距归零（= `--margin-* 0`）：body/html/@page 的 margin/padding 删掉，并注入 `html,body{margin:0;padding:0}`。
//! 4. 段距归零 + 首行缩进（= `--remove-paragraph-spacing --remove-paragraph-spacing-indent-size 2`）：p/div 的
//!    上下 margin/padding 归零（左右保留：blockquote/列表缩进不伤），`p{text-indent:2em}`；`keep_para_spacing` 时
//!    只注缩进（= `WASH_KEEP_PARA_SPACING=1`）。类规则须带元素名才压得过书自带类规则（见书架白皮书 §03y 的七条规则；xochitl 不认 `!important`）。
//! 5. 空页清理：正文无文字无图（Calibre MOBI 转出的 `mbppagebreak` 独占页）→ 从 spine/manifest/zip 删除，
//!    目录里指向它的条目改指下一篇。
//! 6. 自动目录（= `--use-auto-toc --level1-toc //h:h1 --level2-toc //h:h2`）：缺省**仅在书无目录时**从 h1/h2 生成
//!    `toc.ncx` + `nav.xhtml`（xochitl 两者都认）；`AutoToc::Always` 强制重建（原目录坏掉的书）。
//! 7. 单标签重复 `id=` 折叠（`collapse_dup_id_attrs`）：非法 XHTML 会让 xochitl 整章白屏，这里先修、质量门再拦。
//!
//! 全部规则幂等：注入块带 `class="cj-wash"` 标记，重复过不再叠加。
use crate::htmlproc::collapse_dup_id_attrs;
// zip 条目与 zip 内 posix 路径工具已迁到 `epubzip`；这里 re-export，保住 `crate::wash::Entry`/`wash::resolve` 等旧路径。
pub use crate::epubzip::{dir_of, is_html, is_html_entry, percent_decode, posix_norm, relative_to, resolve, Entry};
use crate::util::{is_image_ext, xml_escape};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

// 按职责拆成子模块（原 `wash.rs` 一个文件 2000+ 行）；子模块内的 `pub` 项在这里 glob re-export，`crate::wash::xxx` 旧路径不变，
// 兄弟模块之间的互相调用也走这层（各子模块 `use super::*`）。
mod cover;
mod css;
mod dead_refs;
mod drm;
mod empty_pages;
mod ncx_fix;
mod opf;
mod toc;
mod typeset;

pub use self::cover::*;
pub use self::css::*;
use self::dead_refs::*;
pub use self::drm::*;
use self::empty_pages::*;
use self::ncx_fix::*;
pub(crate) use self::opf::*;
pub use self::toc::*;
pub use self::typeset::*;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoToc {
    Off,
    /// 书无目录（nav/ncx 缺失或零条目）时生成。
    IfMissing,
    Always,
}

/// 正文排版语言（决定首行缩进/段落习惯）。`Auto` 由 `wash_entries` 按全书 CJK/拉丁字符占比判定。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LangMode {
    Auto,
    /// 中文习惯：首行缩进 2em（两个全角字）、段间无空。
    Cjk,
    /// 拉丁习惯：首行缩进 1.2em、标题后首段不缩进。
    Latin,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WashOpts {
    /// 保留原书段间距（诗集/剧本靠空行分节）。
    pub keep_para_spacing: bool,
    pub auto_toc: AutoToc,
    /// 剥掉的 CSS 属性（小写）。缺省与 host `--filter-css` 一致。
    pub filter_props: Vec<String>,
    /// 正文排版语言（`Auto`=自动探测）。
    pub lang: LangMode,
}

impl Default for WashOpts {
    fn default() -> Self {
        WashOpts { keep_para_spacing: false, auto_toc: AutoToc::IfMissing, filter_props: DEFAULT_FILTER_PROPS.iter().map(|s| s.to_string()).collect(), lang: LangMode::Auto }
    }
}

/// 注入排版规则的外链 css 文件名（放 OPF 同目录）。真机坐实（2026-09-04《缩进诊断6》/《飘》）：
/// **xochitl 只认外链 `.css` 文件里的规则，完全无视内联 `<style>` 块和元素 `style=` 属性**——所以
/// 排版规则（首行缩进/边距）必须写成外链 css 才在 xochitl 生效（KOReader/crengine 两者都认）。
/// ⚠ xochitl 的 css 解析器很脆：**只用裸元素选择器**（`p`/`body`），一条类/复杂选择器就可能让整表失效
/// （《缩进诊断5》带 `.big` 类规则时整表不生效，diag6 纯 `p{}` 生效）。
const WASH_CSS_NAME: &str = "cangjie-wash.css";

/// 条目名是不是本层写出的外链 wash css（任意目录下的 `cangjie-wash.css`）。
pub(crate) fn is_wash_css_name(name: &str) -> bool {
    name.rsplit('/').next() == Some(WASH_CSS_NAME)
}

// background / background-image：书常在 body/分卷页用 CSS 背景图（装饰纹样、分卷插画）。xochitl **无视
// no-repeat / background-size** → 把背景图**平铺**满页盖住正文（真机《飘》body.fen 的 `background:url() no-repeat`
// 被铺成多幅）。剥掉背景图声明即净页（章头 <img> 装饰不受影响，仍保留）。@font-face 的 src:url() 由 filter_css 豁免。
// ⚠ 2026-09-17 起不再剥 `color`/`background-color`/`text-align`——EPUB 线原则要求保留原书颜色/加粗等元素样式，
// 只解锁字号；这三项此前只是照抄 Calibre `--filter-css` 通用参数，没有真机验证过是必须剥的。放开后如果书里有
// "深底浅字"高亮块，Paper Pro Move 彩色 e-ink 屏在低对比场景下可能比剥离前更难读——`boost_text_contrast()`
// 目前只处理文字颜色/字重，不处理背景色对比度，真机验证时要专门挑一本带彩色底纹的书测。
pub const DEFAULT_FILTER_PROPS: &[&str] = &["font-family", "font-size", "font", "background-image", "background"];
const WASH_MARK: &str = "cj-wash";

#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct WashReport {
    pub pseudo_drm_stripped: Vec<String>,
    pub css_files: usize,
    pub html_files: usize,
    pub empty_pages_removed: Vec<String>,
    pub toc_generated: usize,
    pub dup_id_tags_collapsed: usize,
    /// `toc.ncx` 的 `dtb:uid` 跟 OPF 标识符不一致、被改到一致（0 或 1——一本书只有一个 ncx）。
    pub ncx_uid_fixed: usize,
    /// 书自带的扁平目录（"第X部　编号　章名"排版惯例）被重建成两级后的条目数；0＝没检测到这种
    /// 惯例、原样没动。
    pub toc_parts_restructured: usize,
    /// `toc.ncx` 里指向外部 DTD 的 `<!DOCTYPE>` 声明被剥掉（0 或 1）。
    pub ncx_doctype_stripped: usize,
    /// manifest 里 NCX 条目的 `id` 被改成 `"ncx"`（0 或 1）。见 `fix_ncx_manifest_id`。
    pub ncx_manifest_id_fixed: usize,
    /// 指向书内不存在文件的 `<img>` / 字体全缺的 `@font-face` 被去掉的个数。见 `drop_dead_refs`。
    pub dead_refs_removed: usize,
}


// ───────────────────────── 入口 ─────────────────────────

/// 全书 CJK vs 拉丁字符占比 → 主语言（Han 字数 ≥ 拉丁字母数 = Cjk）。扫全部 html 正文，早停够量即定。
fn detect_dominant_script(entries: &[Entry]) -> LangMode {
    let (mut han, mut latin) = (0u64, 0u64);
    for e in entries.iter().filter(|e| is_html_entry(&e.name, &e.data) && !is_toc_file(&e.name)) {
        let Ok(t) = std::str::from_utf8(&e.data) else { continue };
        for ch in plain_text(t).chars() {
            if matches!(ch, '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}') {
                han += 1;
            } else if ch.is_ascii_alphabetic() {
                latin += 1;
            }
        }
        if han + latin > 20_000 {
            break; // 够量即判，不必扫全书
        }
    }
    if han >= latin {
        LangMode::Cjk
    } else {
        LangMode::Latin
    }
}

/// 对条目表就地清洗。真 DRM 返回 Err（调用方应整体失败、原样不动）。
pub fn wash_entries(entries: &mut Vec<Entry>, opts: &WashOpts) -> Result<WashReport, String> {
    let mut rep = WashReport::default();
    strip_pseudo_drm(entries, &mut rep)?;
    remove_empty_pages(entries, &mut rep);
    drop_dead_refs(entries, &mut rep);
    // Auto → 探测主语言，解析成具体 Cjk/Latin 再逐文件注排版（探测在剥空页之后、注样式之前）。
    let opts = if opts.lang == LangMode::Auto {
        let mut o = opts.clone();
        o.lang = detect_dominant_script(entries);
        o
    } else {
        opts.clone()
    };
    let opts = &opts;
    // 外链 wash css 的 zip 路径（放 OPF 同目录；无 OPF 兜底放根）。排版规则写这里、逐 html 加 <link>——
    // xochitl 只认外链 css（内联 <style> 无视），见 WASH_CSS_NAME 注。
    let css_path = match find_opf(entries) {
        Some(i) => {
            let d = dir_of(&entries[i].name);
            if d.is_empty() { WASH_CSS_NAME.to_string() } else { format!("{d}/{WASH_CSS_NAME}") }
        }
        None => WASH_CSS_NAME.to_string(),
    };
    for e in entries.iter_mut() {
        let l = e.name.to_ascii_lowercase();
        if l.ends_with(".css") {
            if let Ok(t) = std::str::from_utf8(&e.data) {
                e.data = filter_css(t, opts).into_bytes();
                rep.css_files += 1;
            }
        } else if is_html_entry(&e.name, &e.data) && !is_toc_file(&e.name) {
            if let Ok(t) = std::str::from_utf8(&e.data) {
                let (out, dups) = wash_html(t, opts);
                let href = relative_to(dir_of(&e.name), &css_path);
                let out = inject_css_link(&out, &href);
                rep.dup_id_tags_collapsed += dups;
                e.data = out.into_bytes();
                rep.html_files += 1;
            }
        }
    }
    add_wash_css_entry(entries, &css_path, &wash_css(opts));
    fix_ncx_manifest_id(entries, &mut rep);
    restructure_existing_toc_parts(entries, opts.auto_toc, &mut rep);
    auto_toc(entries, opts.auto_toc, &mut rep);
    fix_ncx_uid(entries, &mut rep);
    strip_ncx_doctype(entries, &mut rep);
    Ok(rep)
}

/// 新增（或重优化时更新）外链 wash css 文件，并往 OPF manifest 补一条 `<item>`（幂等）。
fn add_wash_css_entry(entries: &mut Vec<Entry>, css_path: &str, content: &str) {
    if let Some(e) = entries.iter_mut().find(|e| e.name == css_path) {
        e.data = content.as_bytes().to_vec();
    } else {
        entries.push(Entry { name: css_path.to_string(), data: content.as_bytes().to_vec() });
    }
    if let Some(oi) = find_opf(entries) {
        let opf_dir = dir_of(&entries[oi].name).to_string();
        let href = relative_to(&opf_dir, css_path);
        let mut text = String::from_utf8_lossy(&entries[oi].data).into_owned();
        if !text.contains(&format!("href=\"{href}\"")) {
            if let Some(p) = text.find("</manifest>") {
                text.insert_str(p, &format!("<item id=\"cangjie-wash-css\" href=\"{href}\" media-type=\"text/css\"/>"));
                entries[oi].data = text.into_bytes();
            }
        }
    }
}
