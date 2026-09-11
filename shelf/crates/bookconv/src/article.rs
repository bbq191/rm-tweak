//! 网文抓取：网页文章 URL → 抓取 → 可读性抽取（Mozilla Readability 纯 Rust 移植 `readability-rust`）→
//! 白名单重序列化成合法 XHTML → 组干净 EPUB 字节。**不落盘、不上传**，去向由调用方定。
//!
//! 2026-09-05 从 `reading/device-rs/src/readlater.rs` 下沉进 bookconv（去掉设备上传半 `save_article`/`inject`），
//! 供 book-serve「网文获取→落母版库」与 device-rs `save_article` 共用同一抽取+组装核心。
//!
//! 能力边界（据实告知，非 bug）：可读性抽取对**静态 HTML 的文章/博客/新闻**效果好；SPA（纯 JS 渲染）、
//! 付费墙、反爬站抽不出——纯 Rust 无头浏览器的天花板。图片 best-effort（抓不到就丢该图，不整篇失败）。
//! 当前恒产**单章**（单篇文章足够；多章连载的目录抓取+分章列为后续）。

use crate::convert::common;
use crate::epub::{Book, BookMeta, Chapter, Resource};
use crate::netimg::{fetch_image, http_agent, UA};
use crate::util::xml_escape;
use readability_rust::Readability;
use regex::Regex;

const MAX_IMAGES: usize = 40; // 单篇配图上限，避免异常页把设备拖住

/// 网页文章 URL → 抓取+抽取+组优化 EPUB。返回 (epub 字节, 标题)。
pub fn build_article_epub(url: &str) -> Result<(Vec<u8>, String), String> {
    let url = strip_bad_params(url.trim());
    let url = url.as_str();
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("请输入 http(s) 网址".into());
    }
    let html = fetch_text(url)?;
    let mut r = Readability::new_with_base_uri(&html, url, None) // base_uri：相对图片/链接解析成绝对
        .map_err(|e| format!("解析 HTML 失败: {e:?}"))?;
    let article = r.parse().ok_or("抽取失败（可能是 SPA/付费墙/非文章页）")?;

    let title = article
        .title
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| host_of(url));
    let content = article.content.filter(|c| !c.trim().is_empty()).ok_or("正文为空")?;
    let lang = article.lang.filter(|l| !l.trim().is_empty()).unwrap_or_else(|| "zh".into());
    let author = article.byline.unwrap_or_default();

    let (body, resources) = process_content(&content, url);
    if body.trim().is_empty() {
        return Err("正文处理后为空".into());
    }
    // 文章来源 + 原链接放正文末尾（可回溯）。
    let body = format!("{body}<hr/><p><small>来源：{} · <a href=\"{}\">{}</a></small></p>", xml_escape(&host_of(url)), xml_escape(url), xml_escape(url));

    let mut book = Book {
        meta: BookMeta {
            book_id: format!("readlater:{}", common::sanitize_id(&title)),
            title: title.clone(),
            author,
            language: lang,
            publisher: host_of(url),
            cover: None,
            cover_ext: "jpg".into(),
            cover_media_type: "image/jpeg".into(),
        },
        chapters: vec![Chapter { title: title.clone(), html_body: body, level: 1 }],
        resources,
        nav: Vec::new(),
    };
    let epub = common::assemble_optimized(&mut book)?;
    Ok((epub, title))
}

/// 去掉已知会触发反爬拦截的查询参数（当前：微信公众号的 `poc_token`——带它返回"请在微信打开"拦截页，
/// 去掉后裸链 `/s/<id>` 正文完整可抓）。只删这个已知有害参数，其余查询原样保留。
fn strip_bad_params(url: &str) -> String {
    static R: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r#"(?i)([?&])poc_token=[^&#]*"#).unwrap());
    let s = re.replace_all(url, "$1").into_owned();
    // 清理替换后残留的分隔符：`?&`→`?`、`&&`→`&`、尾部 `?`/`&`
    s.replace("?&", "?").replace("&&", "&").trim_end_matches(['?', '&']).to_string()
}

/// 抓网页 HTML 文本（UTF-8；非 UTF-8 站点暂不支持，属已知边界）。
fn fetch_text(url: &str) -> Result<String, String> {
    let resp = http_agent(30).get(url).set("User-Agent", UA).set("Accept", "text/html,application/xhtml+xml").call().map_err(|e| format!("抓取失败: {e}"))?;
    resp.into_string().map_err(|e| format!("读取网页正文失败: {e}"))
}

/// 处理抽取出的正文 HTML → 合法 XHTML 正文 + 内嵌图片资源。
/// **用真 HTML 解析器（scraper）重解析 + 白名单重序列化**——readability 输出是 HTML5，直接塞进 XHTML
/// 章会崩（`<link>`/`<source>` 空元素不自闭、`&nbsp;` 实体、`data-mw` 属性里塞未转义 `<`；正则补不了）。
/// 重解析后按白名单只保排版标签、属性/文本全转义、空元素自闭，无论输入多脏都产出合法 XHTML。
fn process_content(content: &str, page_url: &str) -> (String, Vec<Resource>) {
    let frag = scraper::Html::parse_fragment(content);
    let ag = http_agent(20);
    let referer = origin_of(page_url); // 抓图带 Referer（微信 mmbiz 等防盗链需要）
    let mut resources: Vec<Resource> = Vec::new();
    let mut out = String::new();
    for child in frag.tree.root().children() {
        emit_node(child, &mut out, &mut resources, &ag, &referer);
    }
    (out, resources)
}

/// URL 的 origin（scheme://host/），作抓图 Referer。
fn origin_of(url: &str) -> String {
    let after = match url.split_once("://") {
        Some((scheme, rest)) => format!("{scheme}://{}", rest.split('/').next().unwrap_or(rest)),
        None => return String::new(),
    };
    format!("{after}/")
}

/// 白名单排版标签（其余标签「拆壳」——丢标签保子内容）。
fn is_whitelisted(name: &str) -> bool {
    matches!(
        name,
        "p" | "div" | "span" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "a" | "em" | "strong" | "b" | "i" | "u" | "s" | "sub" | "sup" | "code" | "pre" | "blockquote" | "ul" | "ol" | "li" | "table" | "thead" | "tbody" | "tr" | "td" | "th" | "caption" | "figure" | "figcaption" | "br" | "hr" | "small" | "mark" | "section" | "article" | "dl" | "dt" | "dd"
    )
}
/// 整棵丢弃（含子内容）的标签——脚本/样式/媒体/表单/页面 chrome/元数据。
fn is_drop_subtree(name: &str) -> bool {
    matches!(
        name,
        "script" | "style" | "noscript" | "link" | "meta" | "head" | "title" | "video" | "audio" | "iframe" | "object" | "embed" | "source" | "track" | "svg" | "canvas" | "form" | "input" | "button" | "select" | "textarea" | "nav" | "footer" | "header" | "aside"
    )
}

/// 递归发射一个节点成 XHTML。
fn emit_node(node: ego_tree::NodeRef<scraper::node::Node>, out: &mut String, resources: &mut Vec<Resource>, ag: &ureq::Agent, referer: &str) {
    use scraper::node::Node;
    match node.value() {
        Node::Text(t) => out.push_str(&xml_escape(&t.text)),
        Node::Element(el) => {
            let name = el.name();
            if is_drop_subtree(name) {
                return;
            }
            if name == "img" {
                // 懒加载兜底：真实图 URL 常在 data-src（微信/知乎等），src 缺失或是 data: 占位时用 data-src。
                let src = el.attr("src").filter(|s| !s.is_empty() && !s.starts_with("data:")).or_else(|| el.attr("data-src"));
                if resources.len() < MAX_IMAGES {
                    if let Some(src) = src {
                        if let Some((bytes, ext, mime)) = fetch_image(ag, src, referer) {
                            let path = format!("images/rl{}.{ext}", resources.len() + 1);
                            resources.push(Resource { path: path.clone(), media_type: mime.into(), bytes });
                            let alt = el.attr("alt").unwrap_or("");
                            out.push_str(&format!("<img src=\"{path}\" alt=\"{}\"/>", xml_escape(alt)));
                        }
                    }
                }
                return; // 空元素，抓不到就整个丢
            }
            if !is_whitelisted(name) {
                for c in node.children() {
                    emit_node(c, out, resources, ag, referer); // 拆壳：丢未知标签、保子内容
                }
                return;
            }
            if name == "br" || name == "hr" {
                out.push('<');
                out.push_str(name);
                out.push_str("/>");
                return;
            }
            out.push('<');
            out.push_str(name);
            for (k, v) in el.attrs() {
                if keep_attr(name, k) {
                    out.push_str(&format!(" {k}=\"{}\"", xml_escape(v)));
                }
            }
            out.push('>');
            for c in node.children() {
                emit_node(c, out, resources, ag, referer);
            }
            out.push_str(&format!("</{name}>"));
        }
        _ => {} // 注释/doctype/PI 丢弃
    }
}

/// 属性白名单：只保排版必需（链接 href、表格跨行列），其余（class/style/data-*/id 等）全丢——
/// xochitl 用自己的样式，脏属性只会带来 XHTML 风险。
fn keep_attr(tag: &str, attr: &str) -> bool {
    match tag {
        "a" => attr == "href",
        "td" | "th" => attr == "colspan" || attr == "rowspan",
        _ => false,
    }
}

fn host_of(url: &str) -> String {
    url.split("://").nth(1).and_then(|rest| rest.split('/').next()).unwrap_or(url).trim_start_matches("www.").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_produces_valid_xhtml_from_dirty_html() {
        // 脏输入：style/script 块、未自闭 <br>/<link>、未知标签 <custom>、data 属性、&nbsp;
        let dirty = r#"<div class="x" data-mw="{&quot;a&quot;:&quot;<b>&quot;}"><style>.y{}</style>
            <p>正文<br>换行</p><script>bad()</script><link rel="z">
            <custom>拆壳保留</custom><a href="http://e.com" class="lnk">链接</a>
            <span typeof="mw:Entity">&nbsp;</span></div>"#;
        let (body, _res) = process_content(dirty, "https://e.com/a");
        // 包成 XHTML 章能被 XML 解析器接受（严格良构）
        let doc = format!(r#"<?xml version="1.0"?><root xmlns="http://www.w3.org/1999/xhtml">{body}</root>"#);
        let mut r = quick_xml::Reader::from_str(&doc);
        loop {
            match r.read_event() {
                Ok(quick_xml::events::Event::Eof) => break,
                Ok(_) => {}
                Err(e) => panic!("产物非良构 XHTML: {e}\n{body}"),
            }
        }
        assert!(body.contains("<br/>"), "br 自闭: {body}");
        assert!(body.contains("正文") && body.contains("换行"), "正文保留: {body}");
        assert!(body.contains("拆壳保留"), "未知标签拆壳保子内容: {body}");
        assert!(body.contains(r#"<a href="http://e.com">链接</a>"#), "链接保 href 去 class: {body}");
        assert!(!body.contains("style") && !body.contains("script") && !body.contains("data-mw"), "脏东西去净: {body}");
    }

    #[test]
    fn host_of_strips_scheme_and_www() {
        assert_eq!(host_of("https://www.example.com/a/b"), "example.com");
        assert_eq!(host_of("http://blog.rust-lang.org/x"), "blog.rust-lang.org");
    }

    #[test]
    fn strip_poc_token_keeps_other_params() {
        assert_eq!(strip_bad_params("https://mp.weixin.qq.com/s/ID?poc_token=ABC"), "https://mp.weixin.qq.com/s/ID");
        assert_eq!(strip_bad_params("https://x/s/ID?a=1&poc_token=ABC&b=2"), "https://x/s/ID?a=1&b=2");
        assert_eq!(strip_bad_params("https://x/s/ID"), "https://x/s/ID"); // 无 token 原样
    }

    #[test]
    fn origin_of_extracts_scheme_host() {
        assert_eq!(origin_of("https://mp.weixin.qq.com/s/ID?x=1"), "https://mp.weixin.qq.com/");
        assert_eq!(origin_of("notaurl"), "");
    }

    #[test]
    fn build_rejects_non_http() {
        assert!(build_article_epub("ftp://x").is_err());
        assert!(build_article_epub("not a url").is_err());
    }
}
