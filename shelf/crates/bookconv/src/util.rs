//! reading 线通用小工具——集中一处，消除各处重复（XML 转义、书名→文件名安全化）。
//! HTTP Agent 见 `crate::netimg::http_agent`。

/// XML/XHTML 文本与属性通用转义：`& < > "`（转义 `"` 对文本无害、对属性必需，故一个函数通吃）。
/// epub 章节、fb2/mobi/kf8 组装、稍后读正文、来源脚注等全共用，替代原先散落的 `xesc`/`xml_escape`。
/// 顺带丢弃 XML 1.0 不允许出现的字符（见 [`is_xml_char`]）——转义救不了它们，留着整份文档就不是合法
/// XML：2026-09-23 真机《T.E.双语》PDF 标题是 UTF-16BE，被当 UTF-8 解出一串 `\0`，写进 OPF 的
/// `dc:title` 后 xochitl 解析 OPF 失败、整本只渲染出 1 页。
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        push_xml_escaped(&mut out, c);
    }
    out
}

/// 单个字符按 [`xml_escape`] 的规则追加进 `out`（逐字符生成正文的调用方用，免得每个字符分配临时 `String`）。
pub fn push_xml_escaped(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        c if !is_xml_char(c) => {}
        _ => out.push(c),
    }
}

/// [`xml_escape`] 的反向：把 XML 文本里的字符引用还原（`&amp; &lt; &gt; &quot; &apos;` 与 `&#N;`/`&#xH;`）；认不出的
/// `&…` 原样保留。从 OPF/正文**读出**文字再**写进**别处时用——不先还原就再转义一遍，`A &amp; B` 会变成
/// `A &amp;amp; B`，设备上显示成字面的 "A &amp; B"。
pub fn xml_unescape(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('&') {
        return std::borrow::Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|&j| j <= 12).and_then(|j| {
            let ent = &rest[1..j];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => ent
                    .strip_prefix("#x")
                    .or_else(|| ent.strip_prefix("#X"))
                    .map(|h| u32::from_str_radix(h, 16))
                    .or_else(|| ent.strip_prefix('#').map(|d| d.parse::<u32>()))
                    .and_then(|r| r.ok())
                    .and_then(char::from_u32),
            };
            c.map(|c| (c, j + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

/// XML 1.0 §2.2 允许的字符：`#x9 | #xA | #xD | [#x20-#xD7FF] | [#xE000-#xFFFD] | [#x10000-#x10FFFF]`
/// （Rust `char` 本来就不含代理区）。
pub fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..='\u{10FFFF}')
}

/// HTML 片段里最后一个可见字符（跳过末尾的标签与空白），用于判断一段话是否以句末标点收尾。
pub fn strip_tags_tail_char(html: &str) -> Option<char> {
    let mut in_tag = false;
    for c in html.chars().rev() {
        match c {
            '>' => in_tag = true,
            '<' => in_tag = false,
            _ if in_tag || c.is_whitespace() => {}
            _ => return Some(c),
        }
    }
    None
}

/// 路径/文件名是不是常见位图（按扩展名，忽略大小写）：jpg/jpeg/png/gif/webp。封面声明、占位封面探测共用；
/// 注意跟 `imgopt::is_downscalable`（只认 jpg/jpeg/png——能重编码降采样的那几种）是两个不同的判据。
pub fn is_image_ext(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".jpg") || l.ends_with(".jpeg") || l.ends_with(".png") || l.ends_with(".gif") || l.ends_with(".webp")
}

/// 图片路径的扩展名（小写、不带点）：取**文件名**最后一个 `.` 之后；文件名没有扩展名时当 `jpg`（EPUB 里绝大多数图是
/// JPEG）。占位封面与漫画分卷重新落名图片共用——此前两处直接 `rsplit('.')`，无扩展名的路径会把整段路径连同 `/`
/// 当扩展名，写出 `cover.images/x`、`images/0001.oebps/images/x` 这种条目名。
pub(crate) fn image_ext_of(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    match base.rsplit_once('.') {
        Some((_, e)) if !e.is_empty() => e.to_ascii_lowercase(),
        _ => "jpg".into(),
    }
}

/// 图片扩展名（不带点、小写）→ media-type；认不出的当 JPEG（EPUB 里绝大多数图是 JPEG）。
pub fn image_media_type_of_ext(ext: &str) -> &'static str {
    match ext {
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
}

/// "先产出到临时文件、成功才改名覆盖目标、失败清掉半成品"的统一外壳（母版库优化的三处原先各写一遍
/// `产出→出错删 tmp→rename→map_err`）。`produce(tmp)` 负责把产物写到 `tmp` 并返回任意结果（如统计报告）；
/// 它出错或最后 `rename` 失败，`tmp` 都会被删掉，不在目录里留半成品。`tmp` 应与 `target` 同分区（rename 才原子）。
pub fn produce_then_replace<T>(tmp: &std::path::Path, target: &std::path::Path, produce: impl FnOnce(&std::path::Path) -> Result<T, String>) -> Result<T, String> {
    let value = match produce(tmp) {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_file(tmp);
            return Err(e);
        }
    };
    if let Err(e) = std::fs::rename(tmp, target) {
        let _ = std::fs::remove_file(tmp);
        return Err(format!("回写母版库失败: {e}"));
    }
    Ok(value)
}

/// 书名 → 安全文件名：控制字符与路径字符（`/\:*?"<>|`）换下划线、去首尾空白、截断 80 字符；
/// 空则用 `default`。ingest（转换落名）、autoopt（优化落名）、readlater（文章落名）共用。
pub fn sanitize_filename(title: &str, default: &str) -> String {
    let t: String = title
        .chars()
        .map(|c| if c.is_control() || "/\\:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let t = t.trim();
    if t.is_empty() {
        default.to_string()
    } else {
        t.chars().take(80).collect()
    }
}

/// 分卷投递的分卷文件名各自独立截断的字节预算——ext4 等主流 Linux 文件系统单个文件名硬限
/// 255 字节，两段预算相加（100+100=200）+ ` - `/`.`/扩展名 留出的十来字节，稳稳落在限内。
const PIECE_NAME_STEM_MAX_BYTES: usize = 100;
const PIECE_NAME_TITLE_MAX_BYTES: usize = 100;

/// 拼分卷投递的文件名："{原书名} - {分卷标题}.{ext}"。**两段都要独立截断，不能只截一段**：
/// 原书名本身可能已经很长（下载站描述性文件名常见，常见破百字节）；分卷标题又可能来自源文件
/// 自己的目录/书签——那是不受我们控制的外部数据，某些书整份目录只有一条、内容恰好就是文件名
/// 本身（single-NCX-entry 的书）。两段各自可能单独超标，只截一段兜不住任意组合；真机撞过
/// 两段相加 278 字节（超过 255 上限）触发 xochitl 泛化的 `"Filesystem error"`——错误信息本身
/// 完全没提字节数/文件名，2026-09-19《乱马1/2 典藏版 19卷》两卷分卷全部投不上，靠手算原书名+
/// 分卷标题拼出来的真实字节数才坐实根因是文件名超限，不是别的什么"文件系统错误"。
pub fn safe_piece_filename(stem: &str, title: &str, ext: &str) -> String {
    let stem = truncate_utf8_bytes(stem, PIECE_NAME_STEM_MAX_BYTES);
    let title = truncate_utf8_bytes(title, PIECE_NAME_TITLE_MAX_BYTES);
    format!("{stem} - {title}.{ext}")
}

/// 按字节预算截断，绝不切在 UTF-8 多字节字符中间（截出来的前缀本身也必须是合法 UTF-8）。
fn truncate_utf8_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_piece_filename_caps_both_segments_independently() {
        let long_stem = "亂馬1⁄2 典藏版 - 19卷 -- 高橋留美子 -- 19, 2019 -- 尖端 -- 03220cf1a8939e8ef2b2253cddd33408 -- Anna\u{2019}s Archive";
        // 真机撞到的真实场景：分卷标题恰好等于原书名本身（single-NCX-entry 的书），两段
        // 各自都超 100 字节预算——修复前 `{stem} - {title}.pdf` 会拼出 278 字节。
        let name = safe_piece_filename(long_stem, long_stem, "pdf");
        assert!(name.len() < 230, "拼出来的完整文件名应该稳稳落在文件系统 255 字节限之内，实际 {} 字节: {name}", name.len());
        assert!(name.ends_with(".pdf"));
    }

    #[test]
    fn safe_piece_filename_leaves_short_names_untouched() {
        assert_eq!(safe_piece_filename("镖人", "第 1-50 页", "pdf"), "镖人 - 第 1-50 页.pdf");
    }

    #[test]
    fn truncate_utf8_bytes_never_splits_a_multibyte_char() {
        let s = "中文中文中文"; // 每字 3 字节
        let t = truncate_utf8_bytes(s, 7); // 7 不是 3 的倍数，必须回退到字符边界
        assert!(s.is_char_boundary(t.len()));
        assert!(std::str::from_utf8(t.as_bytes()).is_ok());
        assert_eq!(t, "中文"); // 截到 6 字节（2 个字），不是硬切出半个字符
    }

    #[test]
    fn produce_then_replace_swaps_on_success_and_cleans_tmp_on_failure() {
        let d = tempfile::tempdir().unwrap();
        let (tmp, target) = (d.path().join(".t.tmp"), d.path().join("t.bin"));
        std::fs::write(&target, b"old").unwrap();
        let n = produce_then_replace(&tmp, &target, |t| std::fs::write(t, b"new").map(|_| 7).map_err(|e| e.to_string())).unwrap();
        assert_eq!(n, 7);
        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!tmp.exists(), "rename 后 tmp 不该残留");
        // 产出失败：原文件不动，半成品被清
        let err = produce_then_replace(&tmp, &target, |t| -> Result<(), String> {
            std::fs::write(t, b"half").unwrap();
            Err("boom".into())
        })
        .unwrap_err();
        assert_eq!(err, "boom");
        assert_eq!(std::fs::read(&target).unwrap(), b"new", "失败不动原文件");
        assert!(!tmp.exists(), "失败清掉半成品");
        // rename 失败（目标是个非空目录）：报回写失败并清 tmp
        let dir_target = d.path().join("dir");
        std::fs::create_dir_all(dir_target.join("x")).unwrap();
        let err = produce_then_replace(&tmp, &dir_target, |t| std::fs::write(t, b"z").map_err(|e| e.to_string())).unwrap_err();
        assert!(err.contains("回写母版库失败"), "{err}");
        assert!(!tmp.exists());
    }

    #[test]
    fn image_ext_of_takes_file_extension_only() {
        assert_eq!(image_ext_of("OEBPS/images/Cv.JPG"), "jpg");
        assert_eq!(image_ext_of("a.b/images/cover"), "jpg", "文件名没扩展名时不能把目录里的点当扩展名");
        assert_eq!(image_ext_of("cover."), "jpg");
        assert_eq!(image_ext_of("x.png"), "png");
    }

    #[test]
    fn image_ext_and_media_type() {
        assert!(is_image_ext("a/B.JPG") && is_image_ext("c.webp") && is_image_ext("x.gif"));
        assert!(!is_image_ext("cover.txt") && !is_image_ext("style.css"));
        assert_eq!(image_media_type_of_ext("png"), "image/png");
        assert_eq!(image_media_type_of_ext("gif"), "image/gif");
        assert_eq!(image_media_type_of_ext("jpg"), "image/jpeg");
        assert_eq!(image_media_type_of_ext("bmp"), "image/jpeg", "认不出当 JPEG");
    }

    #[test]
    fn xml_escape_covers_amp_lt_gt_quote() {
        assert_eq!(xml_escape(r#"a&b<c>d"e"#), "a&amp;b&lt;c&gt;d&quot;e");
        assert_eq!(xml_escape("纯文本"), "纯文本");
        assert_eq!(xml_escape("a\u{0}b\u{1}c\td\u{FFFE}e"), "abc\tde", "XML 1.0 不允许的字符直接丢弃");
    }

    #[test]
    fn xml_unescape_reverses_escape_and_char_refs() {
        let raw = "A & B <c> \"d\" 中";
        assert_eq!(xml_unescape(&xml_escape(raw)), raw);
        assert_eq!(xml_unescape("&#20013;&#x6587;&apos;"), "中文'");
        assert_eq!(xml_unescape("a & b &unknown; &#xZZ; &"), "a & b &unknown; &#xZZ; &", "认不出的原样保留");
        assert!(matches!(xml_unescape("plain"), std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn sanitize_filename_strips_and_defaults() {
        assert_eq!(sanitize_filename("a/b:c?", "book"), "a_b_c_");
        assert_eq!(sanitize_filename("  ", "book"), "book");
        assert_eq!(sanitize_filename("   ", "article"), "article");
        assert_eq!(sanitize_filename("正常书名", "book"), "正常书名");
    }
}
