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
pub const OPTIMIZE_VERSION: &str = "14";

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

/// 第一遍 html 处理：归一同文件 href（part0004.html#x 写在 part0004.html 里→改裸锚 #x，否则下面
/// referenced/搬注释/拆环全把同章脚注误当跨文件）→ 剥字体锁 → 扫这章引用了哪些脚注 frag。
/// `optimize_epub_with`/`optimize_epub_file_streaming` 共用，避免两条路径的第一遍处理逻辑分叉走样。
fn first_pass_html(text: &str, name: &str) -> (String, Vec<String>) {
    let own = std::path::Path::new(name).file_name().and_then(|s| s.to_str()).unwrap_or("");
    let text = crate::htmlproc::normalize_self_hrefs(text, own);
    let stripped = crate::htmlproc::strip_font_locks(&text);
    let referenced = crate::htmlproc::referenced_note_frags(&stripped);
    (stripped, referenced)
}

/// 章节 html 最终变换链：解双向脚注互指环 → duokan 图片脚注标记换上标 → 封面拉伸/SVG 修复 →
/// 脚注就地关联重排 → e-ink 提对比 → 远程图内联 → 全书 id 去重。第一遍（`first_pass_html`）跟这遍
/// 分开是因为这遍要用到第一遍扫全书才拿得到的 `aside_index`（跨章注释索引），顺序不能换。返回
/// (最终字节, 这章新增的远程图资源 [(zip 路径, 字节)])。同上，两条优化路径共用。
fn transform_html_chapter(
    text: &str,
    name: &str,
    aside_index: &std::collections::HashMap<String, String>,
    footnote: FootnoteMode,
    remote_counter: &mut usize,
    img_agent: &ureq::Agent,
    seen_ids: &mut HashSet<String>,
) -> (Vec<u8>, Vec<(String, Vec<u8>)>) {
    let t = crate::htmlproc::break_footnote_cycles(text);
    let t = crate::htmlproc::fix_duokan_markers(&t);
    let t = fix_cover_aspect(&t);
    let t = svg_cover_to_img(&t);
    let t = crate::htmlproc::preserve_relink_footnotes(&t, aside_index, footnote);
    let t = crate::htmlproc::boost_text_contrast(&t);
    let chap_dir = std::path::Path::new(name).parent().and_then(|p| p.to_str()).unwrap_or("");
    let (t, imgs) = inline_remote_images(&t, chap_dir, remote_counter, remote_img_fetcher(img_agent));
    (crate::htmlproc::dedup_ids_in_chapter(&t, seen_ids).into_bytes(), imgs)
}

/// 图片最终变换：按漫画/文字书分流（EPUB 线原则④：漫画只裁边/适配屏幕，不许压画质）。
fn transform_image_bytes(bytes: &[u8], is_comic_book: bool) -> Vec<u8> {
    if is_comic_book {
        let trimmed = crate::imgopt::trim_margins(bytes).unwrap_or_else(|| bytes.to_vec());
        crate::imgopt::downscale_for_epub_comic(&trimmed).unwrap_or(trimmed)
    } else {
        crate::imgopt::downscale_for_epub(bytes).unwrap_or_else(|| bytes.to_vec())
    }
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

    // 漫画识别（EPUB 线原则④）：图 ≥20 张且平均每张图配的文字 <40 字判漫画，决定下面图片降采样时
    // 是否保原画（不跳过必要的屏幕适配缩放，但避免不必要的有损重编码）。用 wash 之后的 `ordered` 判——
    // wash 层已把空页清理、目录归一，判定更准，漫画书也不该被这些文字书专属步骤打扰。
    let is_comic_book = crate::comic_detect::is_comic(&ordered);

    let mut rep = Report { wash: wash_rep, total_files: 0, html_files: 0, bytes_before: epub.len(), bytes_after: 0 };

    // 第一遍：读所有条目。xhtml → strip_font_locks；同时扫全书 marker 得**被引用**的尾注 frag 集
    // （referenced），供下一步"只搬被引用的注释块"用。dir 条目跳过。
    let mut entries: Vec<(String, Vec<u8>, bool)> = Vec::new(); // (name, data, is_html)
    let mut referenced: HashSet<String> = HashSet::new(); // 被 marker 引用的注释 id（noteref + 跨文件普通<a>）
    for crate::wash::Entry { name, mut data } in ordered {
        let name = &name;
        rep.total_files += 1;
        let ish = is_html(name);
        if ish {
            if let Ok(text) = String::from_utf8(data.clone()) {
                let (stripped, refs) = first_pass_html(&text, name);
                referenced.extend(refs);
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
                        let (bytes, imgs) = transform_html_chapter(&text, name, &aside_index, opts.footnote, &mut remote_counter, &img_agent, &mut seen_ids);
                        fetched_imgs.extend(imgs);
                        bytes
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
                // 漫画书（EPUB 线原则④"不允许压画质，只能裁边/适配屏幕"）：先裁四边纯色留白，
                // 超限时改用更高 JPEG 质量重编码。
                transform_image_bytes(data, is_comic_book)
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

/// [`optimize_epub_with`] 的流式版：路径进、路径出，峰值内存不随书体积线性涨。**真机 2026-09-19
/// 坐实**——552MB《镖人》全集走内存版优化，`VmRSS` 几十秒内冲到 1.4GB+，真机系统可用内存探底到
/// ~25MB（book-serve `systemd` 的 `MemoryMax=192M` 没生效——这台设备的 systemd 压根没把 memory
/// 控制器代理进 `system.slice` 子树，这条"软限"从来没真正兜住过），逼近全系统级 OOM（内核会不分
/// 青红皂白挑内存最大的进程杀，可能殃及 xochitl 本体），手动重启服务才止血。
///
/// 内存的大头几乎全是图片字节——漫画书尤其如此，正文 html/css/opf/ncx 这些结构信息本来就很小。
/// 两阶段拆开：**阶段一**只把非图片条目（html/css/opf/ncx/字体等）整份读进内存（本来就小，全书
/// 一起拿着无所谓），图片条目只记名字、字节留空占位——后续 wash 层（空页清理/自动目录/目录分部
/// 重建/dtb:uid 同步）跟漫画识别只看 html 文字内容和 `<img>` 标签*引用*，从来不需要图片真实字节，
/// 占位不影响任何判断。**阶段二**（最终写出）按 `entries` 顺序重新遍历：非图片条目直接用阶段一
/// 已经处理好的字节；图片条目才从源文件按需流式读回这一张的真实字节、处理、立刻写进目标文件，
/// 读完这张就丢，从不会有第二张同时留在内存里。输出直接流式写文件（`ZipWriter` 包 `BufWriter<File>`），
/// 不再攒一份完整产物在内存里。峰值内存量级降到"一张图 + 全书文字部分"，不随书变大线性涨。
///
/// 跟 [`optimize_epub_with`] 共用 [`first_pass_html`]/[`transform_html_chapter`]/
/// [`transform_image_bytes`] 这几个抽出来的变换函数——两条路径的业务逻辑是同一份代码，不会因为
/// "整本内存版"跟"流式版"分叉走样；`optimize_epub_with` 继续保留给测试/CLI 小书场景用（签名不变，
/// 100+ 既有单测零改动），book-serve 真机场景（真书可能上百 MB）改走这条流式路径。
///
/// `on_progress(done, total)`（2026-09-19 补，给调用方画进度条用）：阶段二每写完一个条目回调一次，
/// `total`＝这本书要写出的条目总数（`entries.len()`，含 mimetype 之外的所有文本/图片条目，不含
/// 末尾的 marker）。只在阶段二回调——阶段一（读入+清洗）对文字书通常是毫秒级，真正拖时间的是阶段
/// 二逐张图片的重编码，回调粒度对齐"真正在做的工作"，跟 `comic_split::deliver_split_streaming` 的
/// `upload_piece` 进度粒度同一个道理。**这个回调纯粹是可观测性，不改变内存峰值**——阶段二本来就是
/// 逐条目处理+立刻写文件+立刻丢，回调只是在这个已有的循环里多做一次通知，不持有任何额外数据。
pub fn optimize_epub_file_streaming(input_path: &std::path::Path, output_path: &std::path::Path, opts: &OptimizeOpts, mut on_progress: impl FnMut(usize, usize)) -> Result<Report, String> {
    let in_file = std::fs::File::open(input_path).map_err(|e| format!("打开输入失败: {e}"))?;
    let bytes_before = in_file.metadata().map(|m| m.len() as usize).unwrap_or(0);
    let mut archive = ZipArchive::new(std::io::BufReader::new(in_file)).map_err(|e| format!("解 EPUB(非 zip?): {e}"))?;

    // 阶段一：非图片条目整份读；图片条目占位（真实字节留到阶段二按需流式读）。
    let mut raw: Vec<crate::wash::Entry> = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut f = archive.by_index(i).map_err(|e| format!("读 EPUB 条目 {i}: {e}"))?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        let data = if crate::imgopt::is_downscalable(&name) {
            Vec::new()
        } else {
            let mut d = Vec::new();
            f.read_to_end(&mut d).map_err(|e| e.to_string())?;
            d
        };
        raw.push(crate::wash::Entry { name, data });
    }
    let wash_rep = match &opts.wash {
        Some(w) => Some(crate::wash::wash_entries(&mut raw, w)?),
        None => None,
    };
    let mut ordered: Vec<crate::wash::Entry> = Vec::with_capacity(raw.len());
    if let Some(i) = raw.iter().position(|e| e.name == "mimetype") {
        ordered.push(raw[i].clone());
    }
    for e in raw.into_iter() {
        if e.name != "mimetype" && e.name != OPTIMIZE_MARKER {
            ordered.push(e);
        }
    }
    // 漫画识别只看 html 文字里的 <img> 计数 + 正文字数，图片占位（空字节）不影响判定。
    let is_comic_book = crate::comic_detect::is_comic(&ordered);

    let mut rep = Report { wash: wash_rep, total_files: 0, html_files: 0, bytes_before, bytes_after: 0 };

    let mut entries: Vec<(String, Vec<u8>, bool)> = Vec::new();
    let mut referenced: HashSet<String> = HashSet::new();
    for crate::wash::Entry { name, mut data } in ordered {
        let name = &name;
        rep.total_files += 1;
        let ish = is_html(name);
        if ish {
            if let Ok(text) = String::from_utf8(data.clone()) {
                let (stripped, refs) = first_pass_html(&text, name);
                referenced.extend(refs);
                data = stripped.into_bytes();
                rep.html_files += 1;
            }
        }
        entries.push((name.clone(), data, ish));
    }

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

    // 阶段二：流式写出。非图片条目用阶段一已处理好的字节；图片条目现在才从源文件按需读回真实
    // 字节，处理完立刻写文件、立刻丢——峰值只有"当前这一张"，不会随全书图片数量线性涨。
    let out_file = std::fs::File::create(output_path).map_err(|e| format!("建输出文件失败: {e}"))?;
    let mut zw = ZipWriter::new(std::io::BufWriter::new(out_file));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut seen_ids: HashSet<String> = HashSet::new();
    let img_agent = crate::netimg::http_agent(15);
    let mut remote_counter = 0usize;
    let mut fetched_imgs: Vec<(String, Vec<u8>)> = Vec::new();
    let total_entries = entries.len();
    for (i, (name, data, ish)) in entries.iter().enumerate() {
        let final_data: Vec<u8> = if *ish {
            match String::from_utf8(data.clone()) {
                Ok(text) => {
                    let (bytes, imgs) = transform_html_chapter(&text, name, &aside_index, opts.footnote, &mut remote_counter, &img_agent, &mut seen_ids);
                    fetched_imgs.extend(imgs);
                    bytes
                }
                Err(_) => data.clone(),
            }
        } else if name.to_lowercase().ends_with(".css") {
            match String::from_utf8(data.clone()) {
                Ok(text) => crate::htmlproc::boost_contrast_css(&text).into_bytes(),
                Err(_) => data.clone(),
            }
        } else if crate::imgopt::is_downscalable(name) {
            // 这一张的真实字节现在才读——archive 支持随时按名字重新 seek 读，跟阶段一是同一个源文件。
            let mut f = archive.by_name(name).map_err(|e| format!("重读图片 {name} 失败: {e}"))?;
            let mut real_bytes = Vec::new();
            f.read_to_end(&mut real_bytes).map_err(|e| e.to_string())?;
            transform_image_bytes(&real_bytes, is_comic_book)
        } else {
            data.clone()
        };
        let file_opts = if name == "mimetype" { stored } else { deflated };
        zw.start_file(name.as_str(), file_opts).map_err(|e| e.to_string())?;
        zw.write_all(&final_data).map_err(|e| e.to_string())?;
        on_progress(i + 1, total_entries);
    }
    for (path, bytes) in &fetched_imgs {
        zw.start_file(path.as_str(), deflated).map_err(|e| e.to_string())?;
        zw.write_all(bytes).map_err(|e| e.to_string())?;
    }
    zw.start_file(OPTIMIZE_MARKER, deflated).map_err(|e| e.to_string())?;
    let marker = marker_value(opts.wash.is_some());
    zw.write_all(marker.as_bytes()).map_err(|e| e.to_string())?;
    // `finish()` 只保证写完中央目录，底下 `BufWriter` 自己的缓冲区不一定落盘——显式 flush，
    // 不指望 Drop 的静默兜底（出错会被吞掉）。
    let mut out = zw.finish().map_err(|e| e.to_string())?;
    out.flush().map_err(|e| e.to_string())?;
    // 产物文件大小（跟内存版 out_buf.len() 同语义——压缩后的 zip 体积），直接 stat 落盘文件，比
    // 流式写的时候自己攒一份计数更简单也更准确。
    rep.bytes_after = std::fs::metadata(output_path).map(|m| m.len() as usize).unwrap_or(0);
    Ok(rep)
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

    #[test]
    fn comic_epub_gets_higher_quality_reencode_than_text_book() {
        use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView, RgbImage};
        // x、y 都要变化——否则整张图任意一列（或行）颜色恒定，会被 trim_margins 的"纯色留白"判据
        // 误判成可裁的边框，让漫画路径意外比对照组多裁一刀，干扰这条测试本身要验证的"质量差异"。
        let big = DynamicImage::ImageRgb8(RgbImage::from_fn(2000, 3000, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 150])));
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&big).unwrap();

        // 漫画书：25 张纯图片页（其中一张是超框大图）
        let mut comic_buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut comic_buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            let items: String = (1..=25).map(|i| format!(r#"<item id="c{i}" href="c{i}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
            let spine: String = (1..=25).map(|i| format!(r#"<itemref idref="c{i}"/>"#)).collect();
            zw.start_file("content.opf", stored).unwrap();
            zw.write_all(format!(r#"<package version="3.0"><metadata><dc:title>漫画</dc:title></metadata><manifest>{items}</manifest><spine>{spine}</spine></package>"#).as_bytes()).unwrap();
            for i in 1..=25 {
                zw.start_file(format!("c{i}.xhtml"), stored).unwrap();
                zw.write_all(format!(r#"<html><body><img src="p{i}.jpg"/></body></html>"#).as_bytes()).unwrap();
            }
            zw.start_file("p1.jpg", stored).unwrap();
            zw.write_all(&jpg).unwrap();
            zw.finish().unwrap();
        }
        // 文字书：同一张超框大图，但正文是长文字（不判漫画）
        let long_text = "正".repeat(500);
        let mut text_buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut text_buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            zw.start_file("content.opf", stored).unwrap();
            zw.write_all(r#"<package version="3.0"><metadata><dc:title>文字书</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#.as_bytes()).unwrap();
            zw.start_file("c1.xhtml", stored).unwrap();
            zw.write_all(format!("<html><body><p>{long_text}</p><img src=\"p1.jpg\"/></body></html>").as_bytes()).unwrap();
            zw.start_file("p1.jpg", stored).unwrap();
            zw.write_all(&jpg).unwrap();
            zw.finish().unwrap();
        }

        let (comic_out, _) = optimize_epub(&comic_buf).unwrap();
        let (text_out, _) = optimize_epub(&text_buf).unwrap();
        let mut comic_img = Vec::new();
        ZipArchive::new(Cursor::new(&comic_out)).unwrap().by_name("p1.jpg").unwrap().read_to_end(&mut comic_img).unwrap();
        let mut text_img = Vec::new();
        ZipArchive::new(Cursor::new(&text_out)).unwrap().by_name("p1.jpg").unwrap().read_to_end(&mut text_img).unwrap();

        assert_eq!(
            image::load_from_memory(&comic_img).unwrap().dimensions(),
            image::load_from_memory(&text_img).unwrap().dimensions(),
            "两边尺寸约束一致，都要缩进屏幕框"
        );
        assert!(comic_img.len() > text_img.len(), "漫画书判定应触发更高质量重编码，体积应更大: comic={} text={}", comic_img.len(), text_img.len());
    }

    #[test]
    fn streaming_matches_in_memory_output_for_footnote_book() {
        // 内存版跟流式版共用 first_pass_html/transform_html_chapter，这条测试证明两条路径对同一本
        // 带跨文件脚注的书产出一致的正文——不是"抽了函数就当一样"，是真跑两条路径对拍。
        let epub = make_crossfile_endnote_epub();
        let (mem_out, mem_rep) = optimize_epub(&epub).unwrap();

        let t = tempfile::tempdir().unwrap();
        let input_path = t.path().join("in.epub");
        let output_path = t.path().join("out.epub");
        std::fs::write(&input_path, &epub).unwrap();
        let mut progresses = Vec::new();
        let stream_rep = optimize_epub_file_streaming(&input_path, &output_path, &OptimizeOpts::default(), |done, total| progresses.push((done, total))).unwrap();
        let stream_out = std::fs::read(&output_path).unwrap();

        assert_eq!(mem_rep.total_files, stream_rep.total_files);
        assert_eq!(mem_rep.html_files, stream_rep.html_files);
        assert!(!progresses.is_empty(), "阶段二应该至少回调一次进度");
        assert!(progresses.iter().all(|(_, total)| *total == progresses[0].1), "total 全程不变");
        assert_eq!(progresses.last().unwrap().0, progresses[0].1, "最后一次回调 done 应该等于 total（全部写完）");
        assert!(progresses.windows(2).all(|w| w[0].0 < w[1].0), "done 应该严格递增，不重复不倒退");

        let read = |bytes: &[u8], n: &str| {
            let mut ar = ZipArchive::new(Cursor::new(bytes)).unwrap();
            let mut s = String::new();
            ar.by_name(n).unwrap().read_to_string(&mut s).unwrap();
            s
        };
        for name in ["ch1.xhtml", "ch2.xhtml"] {
            let mem_ch = read(&mem_out, name);
            let stream_ch = read(&stream_out, name);
            assert_eq!(mem_ch, stream_ch, "{name} 内存版跟流式版应产出完全一致的正文");
        }
    }

    #[test]
    fn streaming_downscales_comic_images_same_as_in_memory() {
        // 内存版跟流式版共用 transform_image_bytes，图片处理结果应该逐字节一致——流式版的差别只在
        // "什么时候、从哪读图片字节"，不该影响处理结果本身。
        use image::{codecs::jpeg::JpegEncoder, DynamicImage, GenericImageView, RgbImage};
        let big = DynamicImage::ImageRgb8(RgbImage::from_fn(2000, 3000, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 150])));
        let mut jpg = Vec::new();
        JpegEncoder::new_with_quality(&mut jpg, 90).encode_image(&big).unwrap();

        let mut comic_buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut comic_buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            let items: String = (1..=25).map(|i| format!(r#"<item id="c{i}" href="c{i}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
            let spine: String = (1..=25).map(|i| format!(r#"<itemref idref="c{i}"/>"#)).collect();
            zw.start_file("content.opf", stored).unwrap();
            zw.write_all(format!(r#"<package version="3.0"><metadata><dc:title>漫画</dc:title></metadata><manifest>{items}</manifest><spine>{spine}</spine></package>"#).as_bytes()).unwrap();
            for i in 1..=25 {
                zw.start_file(format!("c{i}.xhtml"), stored).unwrap();
                zw.write_all(format!(r#"<html><body><img src="p{i}.jpg"/></body></html>"#).as_bytes()).unwrap();
            }
            zw.start_file("p1.jpg", stored).unwrap();
            zw.write_all(&jpg).unwrap();
            zw.finish().unwrap();
        }

        let (mem_out, _) = optimize_epub(&comic_buf).unwrap();
        let t = tempfile::tempdir().unwrap();
        let input_path = t.path().join("comic.epub");
        let output_path = t.path().join("comic_out.epub");
        std::fs::write(&input_path, &comic_buf).unwrap();
        optimize_epub_file_streaming(&input_path, &output_path, &OptimizeOpts::default(), |_, _| {}).unwrap();
        let stream_out = std::fs::read(&output_path).unwrap();

        let mut mem_img = Vec::new();
        ZipArchive::new(Cursor::new(&mem_out)).unwrap().by_name("p1.jpg").unwrap().read_to_end(&mut mem_img).unwrap();
        let mut stream_img = Vec::new();
        ZipArchive::new(Cursor::new(&stream_out)).unwrap().by_name("p1.jpg").unwrap().read_to_end(&mut stream_img).unwrap();
        assert_eq!(mem_img, stream_img, "同一张图内存版跟流式版处理结果应逐字节一致");
        assert!(image::load_from_memory(&stream_img).unwrap().dimensions().0 <= 954, "流式版也该按漫画框约束缩放");
    }

    #[test]
    fn streaming_rejects_missing_input_and_leaves_no_partial_output() {
        let t = tempfile::tempdir().unwrap();
        let output_path = t.path().join("out.epub");
        let err = optimize_epub_file_streaming(&t.path().join("does-not-exist.epub"), &output_path, &OptimizeOpts::default(), |_, _| {}).unwrap_err();
        assert!(err.contains("打开输入失败"), "{err}");
        assert!(!output_path.exists(), "输入都打不开，不该产生任何输出文件");
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

    /// 带真实 content.opf 的最小书：spine 里第一页是一份「目录页」（链到一堆不同章节文件，形状
    /// 跟 Calibre 常见排版一致），后面跟几章正文。
    fn make_epub_with_html_toc_page(n_chapters: usize) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut zw = ZipWriter::new(Cursor::new(&mut buf));
            let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            zw.start_file("mimetype", stored).unwrap();
            zw.write_all(b"application/epub+zip").unwrap();
            let links: String = (1..=n_chapters).map(|i| format!(r#"<li><a href="c{i}.xhtml">第{i}章</a></li>"#)).collect();
            zw.start_file("OEBPS/toc.html", stored).unwrap();
            zw.write_all(format!(r#"<html><body><h1>目录</h1><ul>{links}</ul></body></html>"#).as_bytes()).unwrap();
            for i in 1..=n_chapters {
                zw.start_file(format!("OEBPS/c{i}.xhtml"), stored).unwrap();
                zw.write_all(format!("<html><body><p>第{i}章正文</p></body></html>").as_bytes()).unwrap();
            }
            let manifest_chapters: String = (1..=n_chapters).map(|i| format!(r#"<item id="c{i}" href="c{i}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
            let spine_chapters: String = (1..=n_chapters).map(|i| format!(r#"<itemref idref="c{i}"/>"#)).collect();
            zw.start_file("OEBPS/content.opf", stored).unwrap();
            zw.write_all(
                format!(
                    r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="toc-html" href="toc.html" media-type="application/xhtml+xml"/>{manifest_chapters}</manifest><spine><itemref idref="toc-html"/>{spine_chapters}</spine></package>"#
                )
                .as_bytes(),
            )
            .unwrap();
            zw.finish().unwrap();
        }
        buf
    }

    #[test]
    fn html_toc_page_kept_in_spine_not_stripped_as_redundant() {
        // 真机回归（2026-09-19，《疯探》）：书自带的 HTML 目录页（链到几十个章节文件）曾被旧逻辑当
        // "跟原生 TOC 冗余"从 spine 删掉，翻页再也看不到目录——违背 EPUB 线原则①"保留目录页"。
        let (out, _) = optimize_epub(&make_epub_with_html_toc_page(20)).unwrap();
        let mut ar = ZipArchive::new(Cursor::new(&out)).unwrap();
        let mut opf = String::new();
        ar.by_name("OEBPS/content.opf").unwrap().read_to_string(&mut opf).unwrap();
        assert!(opf.contains(r#"idref="toc-html""#), "目录页的 itemref 不该从 spine 被删: {opf}");
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
