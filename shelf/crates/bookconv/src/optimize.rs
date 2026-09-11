//! 通用 EPUB 优化器：吃**任意结构**的 EPUB，只就地改每个 (x)html（字体解锁 + 清洗），
//! 其余文件(css/图片/opf/ncx/mimetype)原样保留、结构不动，重打包。用于把用户导入的
//! 第三方书也拉进优化(尤其"字体改不动"——第三方常内联硬写死 font)。
//! 不重组目录/spine，最大限度兼容各家 EPUB。

use regex::Regex;
use std::collections::HashSet;
use std::io::{Cursor, Read, Write};
use std::sync::OnceLock;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

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
pub const OPTIMIZE_VERSION: &str = "10";

/// 脚注呈现方式。xochitl 无弹窗脚注（穷尽真机实测判死），故给它 `Inline` 内联常显=「自动呈现」；
/// weread/pkm 线与第三方书历史行为用 `Anchor`（章末可见 + 同章锚点跳转 + 原生「返回」浮标）。
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
    /// 脚注呈现方式（缺省 `Anchor` 保持历史行为；母版库「优化」传 `Inline`）。
    pub footnote: FootnoteMode,
}

/// HTML 里的远程图（http(s)/协议相对 `//`）→ 抓取降采样内联进 EPUB：抓到→存进 zip（与本章同目录，
/// src 改本地文件名，免相对路径计算）；抓不到→**删掉该 `<img>`**（避免 reMarkable 破图占位=大放大镜）。
/// weread 下载书常含 `res.weread.qq.com` 远程图（logo/图片脚注）。`chap_dir`=本章 zip 内目录；
/// `counter` 跨章递增保资源名唯一。`fetch(src)->Some((字节,ext))|None`（依赖注入便于测试，生产传抓图闭包）。
/// 返回（改写后 html, 新增资源 [(zip路径, 字节)]）。**离线时全部抓不到 → 全删**（放大镜必消，图丢但离线本
/// 就是放大镜，删胜于留）。不加 manifest：reMarkable 按 src 直渲图、不查 manifest（连不在 manifest 的远程
/// URL 都尝试渲染=才有放大镜），故本地图同样直渲（真机验证）。
fn inline_remote_images<F>(
    html: &str,
    chap_dir: &str,
    counter: &mut usize,
    fetch: F,
) -> (String, Vec<(String, Vec<u8>)>)
where
    F: Fn(&str) -> Option<(Vec<u8>, &'static str)>,
{
    static RE_IMG: OnceLock<Regex> = OnceLock::new();
    static RE_SRC: OnceLock<Regex> = OnceLock::new();
    let re_img = RE_IMG.get_or_init(|| Regex::new(r#"(?is)<img\b[^>]*>"#).unwrap());
    let re_src = RE_SRC.get_or_init(|| Regex::new(r#"(?is)\ssrc="([^"]*)""#).unwrap());
    let mut resources: Vec<(String, Vec<u8>)> = Vec::new();
    let out = re_img.replace_all(html, |c: &regex::Captures| {
        let tag = &c[0];
        let src = match re_src.captures(tag).and_then(|m| m.get(1)) {
            Some(s) => s.as_str().to_string(),
            None => return tag.to_string(),
        };
        let remote = src.starts_with("http://") || src.starts_with("https://") || src.starts_with("//");
        if !remote {
            return tag.to_string();
        }
        match fetch(&src) {
            Some((bytes, ext)) => {
                let fname = format!("cj_remote_{}.{ext}", *counter);
                *counter += 1;
                let path = if chap_dir.is_empty() { fname.clone() } else { format!("{chap_dir}/{fname}") };
                resources.push((path, bytes));
                re_src.replace(tag, regex::NoExpand(&format!(r#" src="{fname}""#))).into_owned()
            }
            None => String::new(), // 抓不到 → 删掉 img（无放大镜）
        }
    });
    (out.into_owned(), resources)
}

/// 生产抓图闭包：`//`→https、Referer=图自身 origin（满足多数 CDN 同源防盗链）、抓取+降采样。
fn remote_img_fetcher(ag: &ureq::Agent) -> impl Fn(&str) -> Option<(Vec<u8>, &'static str)> + '_ {
    move |src: &str| {
        let abs = if let Some(r) = src.strip_prefix("//") { format!("https://{r}") } else { src.to_string() };
        let referer = abs
            .find("://")
            .and_then(|i| abs[i + 3..].find('/').map(|j| &abs[..i + 3 + j + 1]))
            .unwrap_or("")
            .to_string();
        crate::netimg::fetch_image(ag, src, &referer).map(|(b, ext, _mime)| (b, ext))
    }
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

/// 从任意可 seek 的 zip 读端取标记（内存字节与磁盘文件共用；ZipArchive 只读中央目录 + 标记那一条，不解压正文）。
fn marker_in<R: Read + std::io::Seek>(reader: R) -> Option<String> {
    let mut ar = ZipArchive::new(reader).ok()?;
    let mut f = ar.by_name(OPTIMIZE_MARKER).ok()?;
    let mut s = String::new();
    f.read_to_string(&mut s).ok()?;
    Some(s.trim().to_string())
}

/// 读 EPUB 判是否已被本优化器处理过。返回内埋的版本串(Some=已优化)。
/// 非 zip / 损坏 / 无标记都当"未优化"(None)。
pub fn optimized_version(epub: &[u8]) -> Option<String> {
    marker_in(Cursor::new(epub))
}

/// 是否已优化过(任意版本)。
pub fn is_optimized(epub: &[u8]) -> bool {
    optimized_version(epub).is_some()
}

/// 标记值分等级（2026-09-05，修"已优化徽章说谎"）：**含清洗层的完整优化 = 版本号本身**；只跑核心遍
/// （`wash=None`，如网文 / 格式转换产物的 `assemble_optimized`）= `<版本>-core`。母版库据此显示
/// 「已优化 / 已优化·未清洗」并只对 full 隐藏「优化」按钮；weread 线只看"有无标记"（`is_optimized`），不受影响。
pub fn marker_value(full: bool) -> String {
    if full { OPTIMIZE_VERSION.to_string() } else { format!("{OPTIMIZE_VERSION}-core") }
}

/// 同 optimized_version，但直接开文件——不把整本 epub 读进内存，供书库列表逐本轻量标注。
pub fn optimized_version_file(path: &str) -> Option<String> {
    marker_in(std::fs::File::open(path).ok()?)
}

use crate::wash::is_html;

/// 修封面拉伸变形：calibre 封面页 SVG 常用 preserveAspectRatio="none"（强制铺满、不保宽高比，
/// 封面被拉伸放大变形），改成 "xMidYMid meet"（保持比例缩放到适配）。覆盖小写/标准两种写法。
fn fix_cover_aspect(html: &str) -> String {
    html.replace("preserveaspectratio=\"none\"", "preserveAspectRatio=\"xMidYMid meet\"")
        .replace("preserveAspectRatio=\"none\"", "preserveAspectRatio=\"xMidYMid meet\"")
}

/// 目录页判据：一个 (x)html 指向多少个**不同的** html 文件。目录页会指向全书几十个章节文件，
/// 正文页的脚注是同文件 `#frag`（不带 .html）——两者天差地别，阈值取 10 足以区分。
const TOC_LINK_THRESHOLD: usize = 10;

fn count_distinct_html_links(html: &str) -> usize {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r#"href="([^"]+)""#).unwrap());
    let mut set: HashSet<String> = HashSet::new();
    for c in re.captures_iter(html) {
        let file = c[1].split('#').next().unwrap_or("");
        let l = file.to_lowercase();
        if l.ends_with(".html") || l.ends_with(".xhtml") || l.ends_with(".htm") {
            let base = file.rsplit('/').next().unwrap_or(file);
            set.insert(base.to_string());
        }
    }
    set.len()
}

/// 封面 SVG 换普通 img：calibre 封面页 `<svg ...><image (xlink:)href="X"/></svg>` 被 xochitl
/// 拉伸放大（改 preserveAspectRatio 都不吃），换成标准 `<img src="X" style=max-width:100%>` 更可控。
fn svg_cover_to_img(html: &str) -> String {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = R.get_or_init(|| {
        Regex::new(r#"(?is)<svg\b[^>]*>.*?<image\b[^>]*?(?:xlink:)?href="([^"]+)"[^>]*?/?>.*?</svg>"#).unwrap()
    });
    re.replace_all(html, |c: &regex::Captures| {
        format!(
            r#"<img src="{}" alt="cover" style="display:block;margin:0 auto;max-width:100%;height:auto;"/>"#,
            &c[1]
        )
    })
    .into_owned()
}

/// 从 opf 的 spine 移除目录页——把 toc_basenames 里的文件对应的 `<itemref>` 删掉，阅读翻页即跳过
/// 冗余 HTML 目录页（reMarkable 有自己的 TOC）。只动 spine，manifest 条目保留（无害、不产悬空）。
fn remove_toc_from_spine(opf: &str, toc_basenames: &HashSet<String>) -> String {
    if toc_basenames.is_empty() {
        return opf.to_string();
    }
    let item_re = Regex::new(r#"(?is)<item\b[^>]*?/?>"#).unwrap();
    let href_re = Regex::new(r#"href="([^"]+)""#).unwrap();
    let id_re = Regex::new(r#"\bid="([^"]+)""#).unwrap();
    let mut toc_ids: HashSet<String> = HashSet::new();
    for m in item_re.find_iter(opf) {
        let tag = m.as_str();
        let href = match href_re.captures(tag) {
            Some(c) => c[1].to_string(),
            None => continue,
        };
        let base = href.split('#').next().unwrap_or("").rsplit('/').next().unwrap_or("").to_string();
        if toc_basenames.contains(&base) {
            if let Some(c) = id_re.captures(tag) {
                toc_ids.insert(c[1].to_string());
            }
        }
    }
    let mut out = opf.to_string();
    for id in &toc_ids {
        let re = Regex::new(&format!(r#"(?s)<itemref\b[^>]*?\bidref="{}"[^>]*?/?>\s*"#, regex::escape(id))).unwrap();
        out = re.replace_all(&out, "").into_owned();
    }
    out
}

/// 解包 → 每个 (x)html 走 strip_font_locks → 原样保留其余 → 重打包。返回 (新epub, 统计)。
pub fn optimize_epub(epub: &[u8]) -> Result<(Vec<u8>, Report), String> {
    optimize_epub_with(epub, &OptimizeOpts::default())
}

/// 带选项：`opts.wash` 有值则先过清洗层（真 DRM 在此报错、原样不动）。
pub fn optimize_epub_with(epub: &[u8], opts: &OptimizeOpts) -> Result<(Vec<u8>, Report), String> {
    // 读失败必须整体报错——绝不能静默跳过条目产出残缺 EPUB（会破坏原书）。
    let mut raw = crate::check::read_entries(epub)?;
    let wash_rep = match &opts.wash {
        Some(w) => Some(crate::wash::wash_entries(&mut raw, w)?),
        None => None,
    };
    // mimetype 必须首个且 STORED（EPUB 规范），其余原序；旧标记剔除(结尾统一重写当前版本，避免重优化时残留两条)。
    let mut ordered: Vec<crate::wash::Entry> = Vec::with_capacity(raw.len());
    if let Some(i) = raw.iter().position(|e| e.name == "mimetype") {
        ordered.push(raw[i].clone());
    }
    for e in raw.into_iter() {
        if e.name != "mimetype" && e.name != OPTIMIZE_MARKER {
            ordered.push(e);
        }
    }

    let mut rep = Report { wash: wash_rep, total_files: 0, html_files: 0, bytes_before: epub.len(), bytes_after: 0 };

    // 第一遍：读所有条目。xhtml → strip_font_locks；同时扫全书 marker 得**被引用**的尾注 frag 集
    // （referenced），供下一步"只搬被引用的注释块"用。dir 条目跳过。
    let mut entries: Vec<(String, Vec<u8>, bool)> = Vec::new(); // (name, data, is_html)
    let mut referenced: HashSet<String> = HashSet::new(); // 被 marker 引用的注释 id（noteref + 跨文件普通<a>）
    let mut toc_basenames: HashSet<String> = HashSet::new(); // 指向大量 html 的目录页(basename)
    for crate::wash::Entry { name, mut data } in ordered {
        let name = &name;
        rep.total_files += 1;
        let ish = is_html(name);
        if ish {
            if let Ok(text) = String::from_utf8(data.clone()) {
                // 目录页判据：指向 >= 阈值 个不同 html 文件 → 记为目录页，第二遍从 spine 删。
                if count_distinct_html_links(&text) >= TOC_LINK_THRESHOLD {
                    if let Some(base) = std::path::Path::new(name).file_name().and_then(|s| s.to_str()) {
                        toc_basenames.insert(base.to_string());
                    }
                }
                // 同文件带文件名 href（Calibre 写法 part0004.html#x 写在 part0004.html 里）先归一成裸锚，
                // 否则下面 referenced/搬注释/拆环全把同章脚注误当跨文件（v5）。
                let own = std::path::Path::new(name).file_name().and_then(|s| s.to_str()).unwrap_or("");
                let text = crate::htmlproc::normalize_self_hrefs(&text, own);
                let stripped = crate::htmlproc::strip_font_locks(&text);
                for f in crate::htmlproc::referenced_note_frags(&stripped) {
                    referenced.insert(f);
                }
                data = stripped.into_bytes();
                rep.html_files += 1;
            }
        }
        entries.push((name.clone(), data, ish));
    }

    // 第一遍后半：把**被引用**的注释块（aside/p/li 且带注释语义）从各章移除、建全书索引 aside_index，
    // 交给第二遍 preserve_relink_footnotes 搬进引用它的那一章。未被引用的块原样留在原处（零丢失）。
    let mut aside_index: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for (name, data, ish) in entries.iter_mut() {
        if !*ish {
            continue;
        }
        if let Ok(text) = String::from_utf8(data.clone()) {
            let (cleaned, notes) = crate::htmlproc::collect_footnote_notes(&text, &referenced, true);
            if !notes.is_empty() {
                for (id, note) in notes {
                    aside_index.insert(id, note);
                }
                *data = cleaned.into_bytes();
            }
            let _ = name;
        }
    }

    // 第二遍：xhtml → preserve_relink_footnotes(marker 保留原样、去 epub:type、注释移章末) →
    //   dedup_ids_in_chapter(全书 id 去重，防跨章 id 撞车)。打包。
    let mut out_buf: Vec<u8> = Vec::new();
    let mut seen_ids: HashSet<String> = HashSet::new(); // 跨章累积，dedup_ids_in_chapter 用
    let img_agent = crate::netimg::http_agent(15); // 远程图抓取（仅当章内有远程 img 才发请求；离线→抓不到→删 img）
    let mut remote_counter = 0usize;
    let mut fetched_imgs: Vec<(String, Vec<u8>)> = Vec::new();
    {
        let mut zw = ZipWriter::new(Cursor::new(&mut out_buf));
        // mimetype 必须首个且 STORED（EPUB 规范）；其余用 Deflated 压缩，否则文本不压缩体积翻倍。
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        for (name, data, ish) in &entries {
            let final_data: Vec<u8> = if *ish {
                match String::from_utf8(data.clone()) {
                    Ok(text) => {
                        // break_footnote_cycles：拆双向脚注互指对（reMarkable 索引器遇互指整对丢弃→点不动），
                        //   calibre filepos 脚注/微信读书脚注都是这个形态。封面：先试 aspect，再 SVG→img。
                        let t = crate::htmlproc::break_footnote_cycles(&text);
                        // duokan 图片脚注标记（注释块同文件、img 是转义死图/远程 CDN 不可点）换成可点上标。
                        // 与下载路径 inline_footnotes 共用同一处理，导入的 duokan 书也固化（不再分情况漏）。
                        let t = crate::htmlproc::fix_duokan_markers(&t);
                        let t = fix_cover_aspect(&t);
                        let t = svg_cover_to_img(&t);
                        let t = crate::htmlproc::preserve_relink_footnotes(&t, &aside_index, opts.footnote);
                        // ② e-ink 提对比：灰字→纯黑、细字重→400（style 属性 + <style> 块）。
                        let t = crate::htmlproc::boost_text_contrast(&t);
                        // 远程图内联（抓下降采样进 zip / 抓不到删 img，免大放大镜）。
                        let chap_dir = std::path::Path::new(name).parent().and_then(|p| p.to_str()).unwrap_or("");
                        let (t, imgs) = inline_remote_images(&t, chap_dir, &mut remote_counter, remote_img_fetcher(&img_agent));
                        fetched_imgs.extend(imgs);
                        crate::htmlproc::dedup_ids_in_chapter(&t, &mut seen_ids).into_bytes()
                    }
                    Err(_) => data.clone(),
                }
            } else if name.to_lowercase().ends_with(".css") {
                // ② e-ink 提对比：独立 .css 文件里的灰字→纯黑、细字重→400。
                match String::from_utf8(data.clone()) {
                    Ok(text) => crate::htmlproc::boost_contrast_css(&text).into_bytes(),
                    Err(_) => data.clone(),
                }
            } else if crate::imgopt::is_downscalable(name) {
                // ① 按 Move 屏竖向框（宽≤954）降采样超大图——EPUB 图可能行内，宽超 954 会溢出竖屏（缩不动/失败则原样）。
                crate::imgopt::downscale_for_epub(data).unwrap_or_else(|| data.clone())
            } else if name.to_lowercase().ends_with(".opf") {
                // opf：从 spine 删目录页 itemref（去掉冗余 HTML 目录，reMarkable 有自己的 TOC）。
                match String::from_utf8(data.clone()) {
                    Ok(text) => remove_toc_from_spine(&text, &toc_basenames).into_bytes(),
                    Err(_) => data.clone(),
                }
            } else {
                data.clone()
            };
            let opts = if name == "mimetype" { stored } else { deflated };
            zw.start_file(name.as_str(), opts).map_err(|e| e.to_string())?;
            zw.write_all(&final_data).map_err(|e| e.to_string())?;
        }
        // 写入抓取到的远程图资源（与引用它的章同目录、src 已改本地名）。
        for (path, bytes) in &fetched_imgs {
            zw.start_file(path.as_str(), deflated).map_err(|e| e.to_string())?;
            zw.write_all(bytes).map_err(|e| e.to_string())?;
        }
        // 埋幂等标记(结尾)：内容=优化器版本号，供 optimized_version/is_optimized 判据。
        zw.start_file(OPTIMIZE_MARKER, deflated).map_err(|e| e.to_string())?;
        zw.write_all(marker_value(opts.wash.is_some()).as_bytes()).map_err(|e| e.to_string())?;
        zw.finish().map_err(|e| e.to_string())?;
    }
    rep.bytes_after = out_buf.len();
    Ok((out_buf, rep))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inline_remote_images_fetches_removes_and_keeps_local() {
        // 本地图不动；远程抓到→内联改本地名+进资源；远程抓不到→删 img(免放大镜)
        let html = r#"<p><img src="local.png"/><img class="c" src="https://x.com/a.png"/><img src="//y.com/b.png"/></p>"#;
        let mut n = 0usize;
        let (out, res) = inline_remote_images(html, "OEBPS", &mut n, |src| {
            if src.contains("a.png") { Some((vec![1, 2, 3], "png")) } else { None } // b 抓不到
        });
        assert!(out.contains(r#"src="local.png""#), "本地图应原样: {out}");
        assert!(out.contains(r#"src="cj_remote_0.png""#), "远程抓到应改本地名: {out}");
        assert!(out.contains(r#"class="c""#), "改 src 应保留其它属性: {out}");
        assert!(!out.contains("x.com") && !out.contains("y.com"), "远程 URL 应消失: {out}");
        assert!(!out.contains("b.png"), "抓不到的远程 img 应被删: {out}");
        assert_eq!(res.len(), 1, "只有 1 张抓到");
        assert_eq!(res[0].0, "OEBPS/cj_remote_0.png", "资源落本章目录");
        assert_eq!(res[0].1, vec![1, 2, 3]);
    }

    /// 端到端设备优化：EPUB 含超大 JPEG + 灰字 CSS + 灰字/细体内联 style，过优化器后
    /// ① 图片缩到 ≤1696px、② 灰字→纯黑、细字重→400。
    #[test]
    fn device_tuning_downscales_image_and_blackens_text() {
        use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView, RgbImage};
        // 超大 JPEG（3392×1908 = 2× 屏）
        let big = DynamicImage::ImageRgb8(RgbImage::from_fn(3392, 1908, |x, _| {
            image::Rgb([(x % 256) as u8, 100, 150])
        }));
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&big).unwrap();

        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("OEBPS/style.css", stored).unwrap();
            zw.write_all(b"body{color:#333}a{color:blue}").unwrap();
            zw.start_file("OEBPS/img/big.jpg", stored).unwrap();
            zw.write_all(&jpg).unwrap();
            zw.start_file("OEBPS/c1.xhtml", stored).unwrap();
            zw.write_all(r#"<html><body><p style="color:#666;font-weight:300">灰字</p></body></html>"#.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        let (out, _rep) = optimize_epub(&buf).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        // ① 图片缩小
        let mut ib = Vec::new();
        ar.by_name("OEBPS/img/big.jpg").unwrap().read_to_end(&mut ib).unwrap();
        let (w, h) = image::load_from_memory(&ib).unwrap().dimensions();
        assert!(w <= 1696 && h <= 1696, "图应缩到 ≤1696，实为 {w}x{h}");
        assert!(ib.len() < jpg.len(), "缩后体积应变小");
        // ② css 灰字→黑、彩色不动
        let mut css = String::new();
        ar.by_name("OEBPS/style.css").unwrap().read_to_string(&mut css).unwrap();
        assert_eq!(css, "body{color:#000000}a{color:blue}", "css 灰字→黑、彩色不动: {css}");
        // ② 内联 style 灰字→黑、细体→400
        let mut x = String::new();
        ar.by_name("OEBPS/c1.xhtml").unwrap().read_to_string(&mut x).unwrap();
        assert!(x.contains(r#"style="color:#000000;font-weight:400""#), "内联提对比: {x}");
    }

    /// 造一个最小 EPUB(mimetype + 一章带锁字体的 xhtml)，过优化器后字体锁应被剥掉、结构保留。
    fn make_epub() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("EPUB/css/x.css", stored).unwrap();
            zw.write_all(b"body{margin:0}").unwrap();
            zw.start_file("EPUB/xhtml/Section01.xhtml", stored).unwrap();
            zw.write_all(
                r#"<html><body><p style="font-size:16px;font-family:'PingFang SC';">正文</p></body></html>"#.as_bytes(),
            )
            .unwrap();
            zw.finish().unwrap();
        }
        buf
    }

    #[test]
    fn strips_font_keeps_structure() {
        let (out, rep) = optimize_epub(&make_epub()).unwrap();
        assert_eq!(rep.html_files, 1);
        // 重新解开验证
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        // mimetype 首个
        assert_eq!(ar.by_index(0).unwrap().name(), "mimetype");
        // css 原样保留
        let mut css = String::new();
        ar.by_name("EPUB/css/x.css").unwrap().read_to_string(&mut css).unwrap();
        assert_eq!(css, "body{margin:0}");
        // xhtml 字体锁被剥
        let mut x = String::new();
        ar.by_name("EPUB/xhtml/Section01.xhtml").unwrap().read_to_string(&mut x).unwrap();
        assert!(!x.contains("font-family"), "字体锁未剥: {x}");
        assert!(x.contains("<p>正文</p>"), "正文结构被破坏: {x}");
    }

    /// Calibre `wash_epub.sh` 洗过的 duokan 脚注（《人骨拼图》AZW3→EPUB 真实形态）：同文件 href 带文件名、
    /// 标记是真 `<img>` + `<a id="c_2_1">`、注释块 `<li id="a_2_1">` 内回链 `href="part0004.html#c_2_1"`
    /// 构成真 2-环。优化后：href 归一裸锚、标记换上标且 id 保留、回链去链、注释留在原 li 里不被搬成
    /// 无效嵌套（v4 前两条红线全踩：id 丢=回链悬空、`<p id><p>` 嵌套）。
    #[test]
    fn calibre_washed_duokan_footnote_survives() {
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("text/part0004.html", stored).unwrap();
            zw.write_all(r##"<html><body><p class="a">退休储蓄基金<sup class="calibre4"><a class="duokan-footnote" href="part0004.html#a_2_1" id="c_2_1"><img alt="注释1" class="duokan-footnote1" src="../images/00003.png"/></a></sup>，她可以</p>
<ol class="duokan-footnote-content">
<li class="duokan-footnote-item" id="a_2_1">
<p class="pfootnotetext"><a class="calibre6" href="part0004.html#c_2_1">美国一项延税储蓄计划。</a></p>
</li>
</ol></body></html>"##.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        let (out, _) = optimize_epub(&buf).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        let mut x = String::new();
        ar.by_name("text/part0004.html").unwrap().read_to_string(&mut x).unwrap();
        assert!(!x.contains("part0004.html#"), "同文件 href 未归一裸锚: {x}");
        assert!(x.contains(r##"<a href="#a_2_1" id="c_2_1"><sup>1</sup></a>"##), "标记未换上标/丢 id: {x}");
        assert!(!x.contains("<img"), "标记死图未清: {x}");
        assert!(!x.contains(r##"href="#c_2_1""##), "回链未去链(互指对整对丢弃): {x}");
        assert!(x.contains("延税储蓄计划"), "注释文本丢失: {x}");
        assert!(x.contains(r##"<li class="duokan-footnote-item" id="a_2_1">"##), "注释块应原地留在 li 里: {x}");
        assert!(!x.contains("<div class=\"footnotes\">"), "同章注释不该被当跨文件搬走: {x}");
        assert!(!x.contains("<p id=\"a_2_1\">\n"), "不该产出无效嵌套 <p id><p>: {x}");
    }

    /// 造一本"正文章引用书末尾注文件"的 EPUB（bug 复现形态）：两章各引用 notes.xhtml 里的一条
    /// 普通尾注（<a href="notes.xhtml#nX">，非 noteref；注释为 <p id="nX">）。优化后每章 marker 应变
    /// 同章锚点、注释搬进对应章，且两章共用形态不撞 id。
    fn make_crossfile_endnote_epub() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("ch1.xhtml", stored).unwrap();
            zw.write_all(r#"<html><body><p>第一章正文<a href="notes.xhtml#n1">1</a>结束</p></body></html>"#.as_bytes()).unwrap();
            zw.start_file("ch2.xhtml", stored).unwrap();
            zw.write_all(r#"<html><body><p>第二章正文<a href="notes.xhtml#n2">1</a>结束</p></body></html>"#.as_bytes()).unwrap();
            zw.start_file("notes.xhtml", stored).unwrap();
            zw.write_all(r#"<html><body><p class="footnote" id="n1">第一章的注释</p><p class="footnote" id="n2">第二章的注释</p></body></html>"#.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        buf
    }

    #[test]
    fn crossfile_endnotes_relinked_per_chapter() {
        let (out, _) = optimize_epub(&make_crossfile_endnote_epub()).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        let read = |ar: &mut ZipArchive<Cursor<&Vec<u8>>>, n: &str| {
            let mut s = String::new();
            ar.by_name(n).unwrap().read_to_string(&mut s).unwrap();
            s
        };
        let ch1 = read(&mut ar, "ch1.xhtml");
        assert!(ch1.contains(r##"<a href="#n1">1</a>"##), "ch1 marker 未改同章锚点: {ch1}");
        assert!(ch1.contains("第一章的注释"), "ch1 未搬入其注释: {ch1}");
        assert!(!ch1.contains("notes.xhtml"), "ch1 仍残留跨文件 href: {ch1}");
        let ch2 = read(&mut ar, "ch2.xhtml");
        assert!(ch2.contains("第二章的注释"), "ch2(中间章)未搬入其注释: {ch2}");
        assert!(ch2.contains(r##"<a href="#n2">1</a>"##), "ch2 marker 未改同章锚点: {ch2}");
        // 注释已从 notes.xhtml 移走（不重复渲染）
        let notes = read(&mut ar, "notes.xhtml");
        assert!(!notes.contains("第一章的注释") && !notes.contains("第二章的注释"), "注释未从源文件移除: {notes}");
    }

    #[test]
    fn double_optimize_inline_footnote_no_dup() {
        // 版本升级会重优化已优化过的旧书——重优化不得把已内联的注释再翻倍。
        let opts = OptimizeOpts { wash: Some(crate::wash::WashOpts::default()), footnote: FootnoteMode::Inline };
        let (out, _) = optimize_epub_with(&make_crossfile_endnote_epub(), &opts).unwrap();
        let (out2, _) = optimize_epub_with(&out, &opts).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out2)).unwrap();
        let mut ch1 = String::new();
        ar.by_name("ch1.xhtml").unwrap().read_to_string(&mut ch1).unwrap();
        let n = ch1.matches("第一章的注释").count();
        assert_eq!(n, 1, "重优化后注释重复 {n} 次: {ch1}");
    }

    #[test]
    fn reoptimize_relinked_footnote_no_dup() {
        // 模拟旧版本(v6/v7)产物：注释已移同章末尾 <div class="footnotes"> + marker 已是同章锚点。
        // 版本升级重优化这类书时，不得把注释再翻倍。
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("ch1.xhtml", stored).unwrap();
            zw.write_all(r##"<html><body><p>正文<a href="#n1">1</a>结束</p><div class="footnotes"><p id="n1">第一章的注释</p></div></body></html>"##.as_bytes()).unwrap();
            zw.finish().unwrap();
        }
        let opts = OptimizeOpts { wash: Some(crate::wash::WashOpts::default()), footnote: FootnoteMode::Inline };
        let (out, _) = optimize_epub_with(&buf, &opts).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        let mut ch1 = String::new();
        ar.by_name("ch1.xhtml").unwrap().read_to_string(&mut ch1).unwrap();
        let n = ch1.matches("第一章的注释").count();
        assert_eq!(n, 1, "重优化旧版脚注结构翻倍 {n} 次: {ch1}");
    }

    #[test]
    fn marks_and_detects_optimized() {
        let raw = make_epub();
        assert!(!is_optimized(&raw), "原始 EPUB 不该带标记");
        // 默认 optimize_epub 无清洗层 → 只算"核心遍"标记，不能冒充完整优化
        let (out, _) = optimize_epub(&raw).unwrap();
        assert_eq!(optimized_version(&out).as_deref(), Some(format!("{OPTIMIZE_VERSION}-core").as_str()), "无 wash 应标 -core");
        assert!(is_optimized(&out) && optimized_version(&out).as_deref() != Some(OPTIMIZE_VERSION), "有标记但不算当前完整优化");
        // 带清洗层 → 完整标记
        let (full, _) = optimize_epub_with(&raw, &OptimizeOpts { wash: Some(crate::wash::WashOpts::default()), footnote: FootnoteMode::Anchor }).unwrap();
        assert_eq!(optimized_version(&full).as_deref(), Some(OPTIMIZE_VERSION), "含 wash 应标完整版本");
        // 重优化幂等：标记只有一条(不残留旧标记)、版本仍正确
        let (out2, _) = optimize_epub(&out).unwrap();
        assert_eq!(optimized_version(&out2).as_deref(), Some(format!("{OPTIMIZE_VERSION}-core").as_str()));
        let mut ar = ZipArchive::new(Cursor::new(&out2)).unwrap();
        let marker_count = (0..ar.len())
            .filter(|&i| ar.by_index(i).unwrap().name() == OPTIMIZE_MARKER)
            .count();
        assert_eq!(marker_count, 1, "重优化不应残留重复标记条目");
    }
}
