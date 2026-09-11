//! 通用 EPUB 优化器：吃**任意结构**的 EPUB，只就地改每个 (x)html（字体解锁 + 清洗），
//! 其余文件(css/图片/opf/ncx/mimetype)原样保留、结构不动，重打包。用于把用户导入的
//! 第三方书也拉进优化(尤其"字体改不动"——第三方常内联硬写死 font)。
//! 不重组目录/spine，最大限度兼容各家 EPUB。

use regex::Regex;
use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::sync::OnceLock;
use zip::{ZipArchive, ZipWriter};

/// 幂等标记：优化器把这个文件埋进产物 EPUB，内容=优化器版本号。
/// 判"是否优化过"以它为权威——**跟书走**(云同步不丢、换设备仍在、对第三方书和墨香书一视同仁)，
/// 比记设备本地 uuid(会被云同步 churn)或书名后缀(用户改名即失效)都鲁棒。放 META-INF/ 下
/// (EPUB 规范允许该目录放额外文件，阅读器忽略)，STORED 存。
pub const OPTIMIZE_MARKER: &str = "META-INF/com.cangjie.optimized";
/// 优化逻辑版本。升级 strip_font_locks / preserve_relink_footnotes 等行为时 bump，
/// 据此可把旧版本产物挑出来重优化。v2：加 duokan 图片脚注标记修复（fix_duokan_markers）。
/// v3：按 Move 屏规格设备优化——① 图片降采样到 1696px 长边（imgopt）+ ② e-ink 提对比（灰字→纯黑、
/// 细字重→400，作用于 html style / <style> / .css）。v4：远程图内联（微信读书等下载书内嵌
/// `src="https://res.weread.qq.com/..."` 远程图，设备离线加载不到 → reMarkable 破图占位=大放大镜；
/// 优化时抓下来降采样内联进 EPUB，抓不到就删掉该 img 免放大镜）。**新导入书自动应用**；存量书 autoopt 不
/// 会自动重优化（跳过任何已带标记的书），需要时 `POST /optimize {uuid, force:true}` 强制重优化以应用。
/// v5：Calibre 洗书形态——同文件带文件名 href 归一裸锚（normalize_self_hrefs）+ 真 img 版 duokan 标记
/// 换上标且保留 id（`wash_epub.sh` 产物经 host CLI `epub-optimize` 走同一函数）；图片降采样加短边 ≤954
/// 约束（真机探针：块级图缩到正文列宽，方图只卡长边白留 1.8× 像素）。
/// v6：清洗层（`wash`）可选前置——伪 DRM 剥离、CSS 文件级锁剥离、边距/段距归零+2em 缩进、空页清理、
/// 自动目录、单标签双 id 折叠（对标 host `wash_epub.sh`，`OptimizeOpts::wash`；weread 线缺省不开）。
/// v7：做精做强——① 中英文各按阅读习惯注排版（`wash::LangMode` 自动探测：中文首行 2em / 拉丁 1.2em+标题后首段不缩进）；
/// ② 自动目录从 h1/h2 扩到 **h1–h6** 并多级嵌套（只用 h3 当章标题的书不再漏目录）。
/// v8：真机《飘》两修——① 内联脚注丢弃图标 marker（xochitl 按固有尺寸渲染图标=巨大且每条重复）；
/// ② EPUB 内嵌图改竖向框（宽≤954）防行内横幅溢出竖屏；③ 清洗层剥 CSS `background`/`background-image`
/// （xochitl 无视 no-repeat 把背景图平铺满页盖正文，真机《飘》分卷页坐实）——章头 `<img>` 装饰不受影响。
/// v10：真机《缩进诊断6》/《飘》坐实——xochitl **只认外链 `.css` 文件里的规则，完全无视内联 `<style>` 块和元素
/// `style=` 属性**（此前 v6–v9 注入的内联 cj-wash 排版规则在 xochitl 从未生效！）。改：排版规则（首行缩进/边距）
/// 写成**外链 `cangjie-wash.css`** + 每章 `<link>` + OPF manifest 补 item（xochitl/KOReader 都认）。⚠ xochitl css
/// 解析器脆，外链 css **只用裸 `p{}` 元素选择器**（一条类/复杂选择器就让整表失效，《缩进诊断5》坐实）。撤回 v9 的
/// nbsp 段首缩进（nbsp 宽随字体变、且被折叠，做不到精确 2 字；外链 text-indent 精确且字体无关）。
/// v11：真机《疯探》坐实——删掉 `remove_toc_from_spine`（指向 ≥10 个不同 html 文件的页面曾被当
/// "跟原生 TOC 冗余"的目录页从 spine 剥掉）。假设站不住：这类页面是书籍正文本身，不是能丢的冗余物，
/// 违背 EPUB 线原则①"保留目录页"；已优化过的旧书需 `force:true` 重优化才能拿回被剥掉的目录页。
/// v12：真机《疯探》vs《雪人》对照坐实——`toc.ncx` 的 `dtb:uid` 跟 OPF `dc:identifier` 不一致时
/// （第三方生成器常见 bug，如"番茄小说 EPUB Generator"）reMarkable 原生目录面板**直接不显示目录
/// 入口**（不是空列表），navMap 结构再完整都没用；`dtb:uid` 匹配的书目录入口就在。新增
/// `wash::fix_ncx_uid` 把 `dtb:uid` 同步成 OPF 实际标识符（含我们自己 `build_ncx` 生成的也一并
/// 从硬编码 `cj-wash` 改用真实标识符）；旧书需 `force:true` 重优化。
/// v13：dtb:uid 修一致后《疯探》原生目录入口真机复测仍不出现——跟《雪人》剩下唯一的结构性差异是
/// `toc.ncx` 带外部 DTD 引用（`http://www.daisy.org/...dtd`），《雪人》没有。新增
/// `wash::strip_ncx_doctype` 无条件剥掉这个声明（不改变 NCX 语义，纯粹去掉外部依赖，真机 USB/WiFi
/// 隧道环境很可能因为解析器联网取 DTD 卡住/失败而让整份 NCX 被判不可用）；旧书需 `force:true`。
/// v15：EPUB 漫画页补白目标从屏幕比例 954:1696 改成 xochitl 图片框比例 303:462.1（`imgopt::EPUB_FRAME_ASPECT`，画布 954×1458，
/// 补白容差收紧到 0.3%），配合阅读器页边距 1（由 book-serve + `shelf-comic-margins.qmd` 代理设置，实验室开关 `comicMinMargin`）：真机同图 A/B 图片宽 260→303pt、
/// 左右留白 20.0/22.9pt → 约 0.3/0.7pt；旧漫画需重新优化才生效（从原始文件重跑，别对已优化产物二次优化——多一代 JPEG 有损）。
pub const OPTIMIZE_VERSION: &str = "15";

/// 脚注呈现方式。xochitl 无弹窗脚注（穷尽真机实测判死）；weread/pkm 线与第三方书历史行为、
/// EPUB 线设备侧优化（母版库「优化」）2026-09-17 起统一用 `Anchor`（章末可见 + 同章锚点跳转 +
/// 原生「返回」浮标）。⚠ 同日曾短暂加过 `ParagraphEnd`（注释移到引用它的段落末尾），真机验证后
/// 撤回并整个删除——用户真实期望是"翻到哪页注释固定在那页最下面"，EPUB 流式重排做不到真正的
/// 页底部定位（"页"是阅读器翻页时才算出来的，做书时不知道内容最终落在第几页），`ParagraphEnd`
/// 那种"跟着段落走"的近似方案不符合预期；真正的页底部定位是固定版式（PDF 线）的能力，不是
/// EPUB 能原生支持的，见书架白皮书 §03av 补记。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FootnoteMode {
    /// 注释移章末 `<div class="footnotes">` + marker 改同章锚点，点跳、原生浮标返回。
    #[default]
    Anchor,
    /// 注释文字就地内联显示在引用处 `<span class="cj-fnote">〔…〕</span>`，始终可见、不跳转。
    Inline,
}

/// 优化选项：`wash=Some` 时先过清洗层（书架母版库「优化」与 host `epub-optimize` 缺省开；weread 线不开）。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OptimizeOpts {
    pub wash: Option<crate::wash::WashOpts>,
    /// 脚注呈现方式（缺省 `Anchor` 保持历史行为；母版库「优化」目前也传 `Anchor`，见 book-serve `staging/optimizing.rs`）。
    pub footnote: FootnoteMode,
    /// 漫画页补白到哪种页框（缺省 `Screen` = 历史行为）。由 book-serve 按"实验室→漫画页边距"开关传入。
    pub comic_frame: crate::imgopt::EpubComicFrame,
    /// 翻页方向（2026-09-25，母版库按书手动指定）：`Some` 时把 OPF `<spine page-progression-direction>` 写成这个值，
    /// `None`（缺省）＝保留原书，产物与加这个字段之前逐字节相同。见 [`crate::direction`]。
    pub page_direction: Option<crate::direction::PageDirection>,
}

/// 优化统计，供回执。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub wash: Option<crate::wash::WashReport>,
    pub total_files: usize,
    pub html_files: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
}

use crate::epubzip::is_html_entry;

// 按职责拆成子模块（原 `optimize.rs` 一个文件 1100+ 行）：`html_pass`（逐条变换）· `streaming`（流式路径 + 构建器）· `marker`（幂等标记）；
// `pub` 项在这里 glob re-export，`crate::optimize::xxx` 旧路径不变。
mod html_pass;
mod marker;
mod streaming;

use self::html_pass::*;
pub use self::marker::*;
pub use self::streaming::*;

#[cfg(test)]
mod tests;


/// 阶段一的产物（内存版与流式版共用，2026-09-20 审计：这段 ~110 行原先两处逐行复制）。
struct Prepared {
    /// (条目名, 字节, 是否 html)，已过封面声明/清洗/第一遍 html 处理/注释块搬出；`mimetype` 在最前。
    entries: Vec<(String, Vec<u8>, bool)>,
    /// 全书"被引用的注释块"索引（id → 块 html），第二遍 `preserve_relink_footnotes` 搬进引用它的那一章。
    aside_index: std::collections::HashMap<String, String>,
    is_comic_book: bool,
    /// 要改 OPF 时（`title=Some` 或指定了翻页方向）的 OPF 条目名，第二遍据此改 `dc:title` / spine 方向。
    opf_name: Option<String>,
    rep: Report,
}

/// 阶段一：`raw` → 封面声明 → 清洗 → 排序（mimetype 置首、旧标记剔除）→ 漫画识别 → 第一遍 html → 注释块搬出。
/// 图片条目可以是空占位（流式路径）——这里所有判断只看 html 文字与 `<img>` 引用，不需要图片真实字节。
fn prepare_entries(mut raw: Vec<crate::epubzip::Entry>, opts: &OptimizeOpts, bytes_before: usize, title: Option<&str>) -> Result<Prepared, String> {
    // 保证 OPF 声明了有效封面（设备日志核查发现 7/9 本已投的书没有封面，见 `wash::ensure_cover_declared`）。
    // 必须在清洗之前：清洗会把只含 SVG 封面的 titlepage 当空页删掉。
    crate::wash::ensure_cover_declared(&mut raw);
    let wash_rep = match &opts.wash {
        Some(w) => Some(crate::wash::wash_entries(&mut raw, w)?),
        None => None,
    };
    // mimetype 必须首个且 STORED（EPUB 规范），其余原序；旧标记剔除(结尾统一重写当前版本，避免重优化时残留两条)。
    let mut ordered: Vec<crate::epubzip::Entry> = Vec::with_capacity(raw.len());
    if let Some(i) = raw.iter().position(|e| e.name == "mimetype") {
        ordered.push(raw[i].clone());
    }
    for e in raw.into_iter() {
        if e.name != "mimetype" && e.name != OPTIMIZE_MARKER {
            ordered.push(e);
        }
    }

    // 漫画识别（EPUB 线原则④）：图 ≥20 张且平均每张图配的文字 <40 字判漫画，决定下面图片降采样时
    // 是否保原画（不跳过必要的屏幕适配缩放，但避免不必要的有损重编码）。用 wash 之后的 `ordered` 判——
    // wash 层已把空页清理、目录归一，判定更准，漫画书也不该被这些文字书专属步骤打扰。
    let is_comic_book = crate::comic_detect::is_comic(&ordered);
    let opf_name: Option<String> = (title.is_some() || opts.page_direction.is_some()).then(|| crate::wash::parse_opf(&ordered).map(|o| ordered[o.index].name.clone())).flatten();

    let mut rep = Report { wash: wash_rep, total_files: 0, html_files: 0, bytes_before, bytes_after: 0 };

    // 第一遍：读所有条目。xhtml → strip_font_locks；同时扫全书 marker 得**被引用**的尾注 frag 集
    // （referenced），供下一步"只搬被引用的注释块"用。dir 条目跳过。
    let mut entries: Vec<(String, Vec<u8>, bool)> = Vec::with_capacity(ordered.len());
    let mut referenced: HashSet<String> = HashSet::new(); // 被 marker 引用的注释 id（noteref + 跨文件普通<a>）
    for crate::epubzip::Entry { name, data } in ordered {
        rep.total_files += 1;
        let ish = is_html_entry(&name, &data);
        // 非 UTF-8 的 html 原样保留（`from_utf8` 失败时把字节还回来，不克隆）。
        let data = if ish {
            match String::from_utf8(data) {
                Ok(text) => {
                    let (stripped, refs) = first_pass_html(&text, &name);
                    referenced.extend(refs);
                    rep.html_files += 1;
                    stripped.into_bytes()
                }
                Err(e) => e.into_bytes(),
            }
        } else {
            data
        };
        entries.push((name, data, ish));
    }

    // 第一遍后半：把**被引用**的注释块（aside/p/li 且带注释语义）从各章移除、建全书索引 aside_index，
    // 交给第二遍 preserve_relink_footnotes 搬进引用它的那一章。未被引用的块原样留在原处（零丢失）。
    let mut aside_index: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (_, data, ish) in entries.iter_mut() {
        if !*ish {
            continue;
        }
        let Ok(text) = std::str::from_utf8(data) else { continue };
        let (cleaned, notes) = crate::htmlproc::collect_footnote_notes(text, &referenced, true);
        if !notes.is_empty() {
            aside_index.extend(notes);
            *data = cleaned.into_bytes();
        }
    }
    // 漫画 + 页边距最小化页框：纯文字页（版权/前情提要/章节标题）补左右留白，否则边距 1 下文字贴屏幕边。
    // 放在最后：此时 html 已过第一遍、注释块已搬出，判"纯文字页"看到的就是最终页面结构。
    if is_comic_book && opts.comic_frame == crate::imgopt::EpubComicFrame::MinMargin {
        crate::comic_pad::pad_text_pages(&mut entries);
        crate::comic_pad::free_media_pages(&mut entries);
        crate::comic_pad::pad_mixed_text_blocks(&mut entries);
    }
    Ok(Prepared { entries, aside_index, is_comic_book, opf_name, rep })
}

/// 第二遍的"文本类条目"变换器：html 章节 / 独立 css / （改书名时）OPF。跨条目状态（远程图计数、全书 id 去重表、
/// 抓到的远程图）都在这里，内存版与流式版共用同一份逻辑。图片条目不归它管（内存版直接调
/// `transform_image_bytes`，流式版走 `imgpool` 并行）。
struct EntryXform<'a> {
    aside_index: &'a std::collections::HashMap<String, String>,
    footnote: FootnoteMode,
    title: Option<&'a str>,
    page_direction: Option<crate::direction::PageDirection>,
    opf_name: Option<&'a str>,
    seen_ids: HashSet<String>, // 跨章累积，dedup_ids_in_chapter 用
    img_agent: ureq::Agent,    // 远程图抓取（仅当章内有远程 img 才发请求；离线→抓不到→删 img）
    remote_counter: usize,
    fetched_imgs: Vec<(String, Vec<u8>)>,
}

impl<'a> EntryXform<'a> {
    fn new(prep_aside: &'a std::collections::HashMap<String, String>, footnote: FootnoteMode, title: Option<&'a str>, page_direction: Option<crate::direction::PageDirection>, opf_name: Option<&'a str>) -> EntryXform<'a> {
        EntryXform { aside_index: prep_aside, footnote, title, page_direction, opf_name, seen_ids: HashSet::new(), img_agent: crate::netimg::http_agent(15), remote_counter: 0, fetched_imgs: Vec::new() }
    }

    /// 文本类条目 → `Some(最终字节)`（无法按 UTF-8 解读的原样借回）；不是文本类（图片/其它）→ `None`，调用方自己处理。
    fn transform_text<'d>(&mut self, name: &str, data: &'d [u8], is_html: bool) -> Option<std::borrow::Cow<'d, [u8]>> {
        use std::borrow::Cow;
        if is_html {
            return Some(match std::str::from_utf8(data) {
                Ok(text) => {
                    let (bytes, imgs) = transform_html_chapter(text, name, self.aside_index, self.footnote, &mut self.remote_counter, &self.img_agent, &mut self.seen_ids);
                    self.fetched_imgs.extend(imgs);
                    Cow::Owned(bytes)
                }
                Err(_) => Cow::Borrowed(data),
            });
        }
        if name.to_lowercase().ends_with(".css") {
            // ② e-ink 提对比：独立 .css 文件里的灰字→纯黑、细字重→400。
            return Some(match std::str::from_utf8(data) {
                Ok(text) => Cow::Owned(crate::htmlproc::boost_contrast_css(text).into_bytes()),
                Err(_) => Cow::Borrowed(data),
            });
        }
        if self.opf_name == Some(name) && (self.title.is_some() || self.page_direction.is_some()) {
            return Some(match std::str::from_utf8(data) {
                Ok(text) => {
                    let text = match self.title {
                        Some(title) => crate::placeholder::set_opf_title(text, title),
                        None => text.to_string(),
                    };
                    let text = match self.page_direction {
                        Some(dir) => crate::direction::set_spine_direction(&text, dir),
                        None => text,
                    };
                    Cow::Owned(text.into_bytes())
                }
                Err(_) => Cow::Borrowed(data),
            });
        }
        None
    }
}

/// 两条路径共同的收尾：写入抓取到的远程图资源（与引用它的章同目录、src 已改本地名），再在结尾埋幂等标记
/// （内容=优化器版本号，供 optimized_version/is_optimized 判据）。
fn write_tail<W: Write + std::io::Seek>(zw: &mut ZipWriter<W>, fetched_imgs: &[(String, Vec<u8>)], full: bool) -> Result<(), String> {
    let deflated = crate::epubzip::deflated();
    for (path, bytes) in fetched_imgs {
        crate::epubzip::put_entry(zw, path, deflated, bytes)?;
    }
    crate::epubzip::put_entry(zw, OPTIMIZE_MARKER, deflated, marker_value(full).as_bytes())
}

/// 解包 → 每个 (x)html 走 strip_font_locks → 原样保留其余 → 重打包。返回 (新epub, 统计)。
pub fn optimize_epub(epub: &[u8]) -> Result<(Vec<u8>, Report), String> {
    optimize_epub_with(epub, &OptimizeOpts::default())
}

/// 带选项：`opts.wash` 有值则先过清洗层（真 DRM 在此报错、原样不动）。
pub fn optimize_epub_with(epub: &[u8], opts: &OptimizeOpts) -> Result<(Vec<u8>, Report), String> {
    // 读失败必须整体报错——绝不能静默跳过条目产出残缺 EPUB（会破坏原书）。
    let raw = crate::check::read_entries(epub)?;
    let Prepared { entries, aside_index, is_comic_book, opf_name, mut rep } = prepare_entries(raw, opts, epub.len(), None)?;

    // 第二遍：xhtml → preserve_relink_footnotes(marker 保留原样、去 epub:type、注释移章末) →
    //   dedup_ids_in_chapter(全书 id 去重，防跨章 id 撞车)。打包。
    let mut out_buf: Vec<u8> = Vec::new();
    let mut xf = EntryXform::new(&aside_index, opts.footnote, None, opts.page_direction, opf_name.as_deref());
    {
        let mut zw = ZipWriter::new(Cursor::new(&mut out_buf));
        // mimetype 必须首个且 STORED（EPUB 规范）；其余用 Deflated 压缩，否则文本不压缩体积翻倍。
        let (stored, deflated) = (crate::epubzip::stored(), crate::epubzip::deflated());
        for (name, data, ish) in &entries {
            let final_data: std::borrow::Cow<[u8]> = match xf.transform_text(name, data, *ish) {
                Some(t) => t,
                // ① 按 Move 屏竖向框（宽≤954）降采样超大图——EPUB 图可能行内，宽超 954 会溢出竖屏（缩不动/失败则原样）。
                // 漫画书（EPUB 线原则④"不允许压画质，只能裁边/适配屏幕"）：先裁四边纯色留白，
                // 超限时改用更高 JPEG 质量重编码。
                None if crate::imgopt::is_downscalable(name) => transform_image_bytes(data, is_comic_book, opts.comic_frame).map_or(std::borrow::Cow::Borrowed(data.as_slice()), std::borrow::Cow::Owned),
                None => std::borrow::Cow::Borrowed(data.as_slice()),
            };
            crate::epubzip::put_entry(&mut zw, name, if name == "mimetype" { stored } else { deflated }, &final_data)?;
        }
        write_tail(&mut zw, &xf.fetched_imgs, opts.wash.is_some())?;
        zw.finish().map_err(|e| e.to_string())?;
    }
    rep.bytes_after = out_buf.len();
    Ok((out_buf, rep))
}
