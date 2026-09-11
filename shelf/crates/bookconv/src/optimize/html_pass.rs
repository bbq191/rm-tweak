//! 章节 html / 图片的逐条变换（第一遍规整、第二遍脚注重排+提对比+远程图内联、图片降采样）。
use super::*;

/// HTML 里的远程图（http(s)/协议相对 `//`）→ 抓取降采样内联进 EPUB：抓到→存进 zip（与本章同目录，
/// src 改本地文件名，免相对路径计算）；抓不到→**删掉该 `<img>`**（避免 reMarkable 破图占位=大放大镜）。
/// weread 下载书常含 `res.weread.qq.com` 远程图（logo/图片脚注）。`chap_dir`=本章 zip 内目录；
/// `counter` 跨章递增保资源名唯一。`fetch(src)->Some((字节,ext))|None`（依赖注入便于测试，生产传抓图闭包）。
/// 返回（改写后 html, 新增资源 [(zip路径, 字节)]）。**离线时全部抓不到 → 全删**（放大镜必消，图丢但离线本
/// 就是放大镜，删胜于留）。不加 manifest：reMarkable 按 src 直渲图、不查 manifest（连不在 manifest 的远程
/// URL 都尝试渲染=才有放大镜），故本地图同样直渲（真机验证）。
pub(super) fn inline_remote_images<F>(
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
pub(super) fn remote_img_fetcher(ag: &ureq::Agent) -> impl Fn(&str) -> Option<(Vec<u8>, &'static str)> + '_ {
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

/// 修封面拉伸变形：calibre 封面页 SVG 常用 preserveAspectRatio="none"（强制铺满、不保宽高比，
/// 封面被拉伸放大变形），改成 "xMidYMid meet"（保持比例缩放到适配）。覆盖小写/标准两种写法。
pub(super) fn fix_cover_aspect(html: &str) -> String {
    html.replace("preserveaspectratio=\"none\"", "preserveAspectRatio=\"xMidYMid meet\"")
        .replace("preserveAspectRatio=\"none\"", "preserveAspectRatio=\"xMidYMid meet\"")
}

/// 封面 SVG 换普通 img：calibre 封面页 `<svg ...><image (xlink:)href="X"/></svg>` 被 xochitl
/// 拉伸放大（改 preserveAspectRatio 都不吃），换成标准 `<img src="X" style=max-width:100%>` 更可控。
pub(super) fn svg_cover_to_img(html: &str) -> String {
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
pub(super) fn first_pass_html(text: &str, name: &str) -> (String, Vec<String>) {
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
pub(super) fn transform_html_chapter(
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
/// 返回 `None` = 无需改动、沿用原字节（调用方自己决定借用还是移走，不为"没变"整张图克隆一份）。
///
/// 解码器遇到畸形图片偶发 panic（第三方书的坏 JPEG/PNG 是外部输入）：这里兜住、按"失败原样保留"处理——此前 panic 会从
/// 图片 worker 线程一路把整本书的优化搞砸（`thread::scope` 把子线程 panic 重新抛给调用方），只为一张坏图不值得。
pub(super) fn transform_image_bytes(bytes: &[u8], is_comic_book: bool, comic_frame: crate::imgopt::EpubComicFrame) -> Option<Vec<u8>> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if is_comic_book {
            // 单趟（解码/编码各一次、灰度保持、缩放走 SIMD）——此前三道串联的问题见 `prepare_comic_page_for_epub`。
            crate::imgopt::prepare_comic_page_for_epub(bytes, comic_frame)
        } else {
            crate::imgopt::downscale_for_epub(bytes)
        }
    }))
    .unwrap_or(None)
}
