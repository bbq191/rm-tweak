//! reading 线通用小工具——集中一处，消除各处重复（XML 转义、书名→文件名安全化）。
//! HTTP Agent 见 `crate::netimg::http_agent`。

/// XML/XHTML 文本与属性通用转义：`& < > "`（转义 `"` 对文本无害、对属性必需，故一个函数通吃）。
/// epub 章节、fb2/mobi/kf8 组装、稍后读正文、来源脚注等全共用，替代原先散落的 `xesc`/`xml_escape`。
pub fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_covers_amp_lt_gt_quote() {
        assert_eq!(xml_escape(r#"a&b<c>d"e"#), "a&amp;b&lt;c&gt;d&quot;e");
        assert_eq!(xml_escape("纯文本"), "纯文本");
    }

    #[test]
    fn sanitize_filename_strips_and_defaults() {
        assert_eq!(sanitize_filename("a/b:c?", "book"), "a_b_c_");
        assert_eq!(sanitize_filename("  ", "book"), "book");
        assert_eq!(sanitize_filename("   ", "article"), "article");
        assert_eq!(sanitize_filename("正常书名", "book"), "正常书名");
    }
}
