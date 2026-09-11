//! 正文排版归一：CJK 假段落、标题后首段顶格、外链 wash css 注入与生成、重复 id 计数。
use super::*;

/// 中文书两种"假段落"归一（2026-09-06 《人骨拼圖》2017 旧 EPUB 真机）：
/// ① 全书没有 `<p>`，每章一个 `<div>` 里 `<br/>` 分行、段首两个全角空格——`p{text-indent:2em}` 没有对象，xochitl 又把
///    U+3000 折叠掉 → 零缩进；KOReader 把 U+3000 按字体宽度画出来 → "换字体缩进跟着变"。→ 按 `<br>` 切成 `<p>`。
/// ② 段首烘死的全角空格 / nbsp（有 `<p>` 的书也常见）→ 剥掉，缩进统一走外链 css（字体无关的精确 2em）。
/// 只在文件里 `<br` 数 ≥ 4 且 `<p` 为 0 时做 ①；② 对所有 `<p>` 做。块级标签（h1–h6/div/section 的开闭、img、table）原样保留。
/// 结尾的块级闭合标签（`</div>` 等），`cjk_paragraphize` 剥尾随闭合标签用。
pub(super) fn block_close_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?i)^</(?:div|section|article|blockquote|ul|ol|li|table|tr|td|th|figure)>$"#).unwrap())
}

/// `<style>…</style>` 块（三段捕获：开标签 / 内容 / 闭标签）。
pub(super) fn style_block_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r#"(?is)(<style\b[^>]*>)(.*?)(</style>)"#).unwrap())
}

pub(super) fn cjk_paragraphize(html: &str) -> String {
    static LEAD: OnceLock<Regex> = OnceLock::new();
    static BR: OnceLock<Regex> = OnceLock::new();
    static BLOCK: OnceLock<Regex> = OnceLock::new();
    let lead = LEAD.get_or_init(|| Regex::new(r#"(?i)(<p\b[^>]*>)(?:\s|\u{3000}|&#12288;|&#x3000;|&nbsp;|&#160;|&#xa0;)+"#).unwrap());
    let out = lead.replace_all(html, "$1").into_owned();
    let n_br = out.matches("<br").count();
    static P_OPEN: OnceLock<Regex> = OnceLock::new();
    let n_p = P_OPEN.get_or_init(|| Regex::new(r#"(?i)<p\b"#).unwrap()).find_iter(&out).count();
    if n_br < 4 || n_p > 0 {
        return out;
    }
    let Some(bstart) = out.find("<body") else { return out };
    let Some(bopen_end) = out[bstart..].find('>').map(|i| bstart + i + 1) else { return out };
    let Some(bend) = out.rfind("</body>") else { return out };
    let (head, body, tail) = (&out[..bopen_end], &out[bopen_end..bend], &out[bend..]);
    let br = BR.get_or_init(|| Regex::new(r#"(?is)(?:\s*<br\b[^>]*>\s*)+"#).unwrap());
    // 块级开闭标签 / 整块元素：不裹进 p
    let block = BLOCK.get_or_init(|| Regex::new(r#"(?is)^\s*(?:</?(?:div|section|article|body|blockquote|ul|ol|li|table|tr|td|th|figure|figcaption)\b[^>]*>|<h[1-6]\b[^>]*>.*?</h[1-6]>|<img\b[^>]*>|<hr\b[^>]*>|<a\b[^>]*id="[^"]*"[^>]*>\s*</a>)\s*"#).unwrap());
    let mut res = String::with_capacity(body.len() + 64);
    for piece in br.split(body) {
        let mut rest = piece;
        // 剥前导块级标签
        loop {
            match block.find(rest) {
                Some(m) if m.start() == 0 => {
                    res.push_str(&rest[..m.end()]);
                    rest = &rest[m.end()..];
                }
                _ => break,
            }
        }
        // 剥尾随块级闭合标签
        let mut trailing = String::new();
        loop {
            let t = rest.trim_end();
            if let Some(i) = t.rfind('<') {
                let tag = &t[i..];
                if block_close_re().is_match(tag) {
                    trailing.insert_str(0, tag);
                    rest = &t[..i];
                    continue;
                }
            }
            break;
        }
        let text = rest.trim().trim_start_matches(|c: char| c == '\u{3000}' || c == '\u{a0}' || c.is_whitespace());
        if !text.is_empty() {
            res.push_str("<p>");
            res.push_str(text);
            res.push_str("</p>");
        }
        res.push_str(&trailing);
    }
    format!("{head}{res}{tail}")
}

/// 拉丁习惯：**标题后 / 章首 / 场景切换后的第一段不缩进**（英文排版惯例：只有紧接上一段的段落才缩进）。
/// xochitl 的 CSS 引擎（2026-09-06 八轮渲染缓存量化，书架白皮书 §03y）：不认内联 `style=""`；`text-indent:0` 当"没设"；
/// 类规则压过元素规则，但同为类规则时**先出现者胜**（书的表链接在前）；规则最后一个无分号的声明被丢。
/// 因此顶格段＝`<div class="cj-flush">`（**剥掉书的类与 style**，只留 cj-flush，id 等保留）+ 外链 `.cj-flush{text-indent:0;…;}`；
/// KOReader 走标准 CSS 同样顶格。判定"前面是标题/切换"的信号（2026-09-06 用《Tell Me Your Dreams》AZW3 定，它的章名不是 `<h>`
/// 而是加粗段落、场景切换是段末双 `<br/>`）：
///   ① 前一个块是 `</h1>`–`</h6>`；② 前一段是"标题样段落"：≤80 字且（全文加粗/strong/class 含 bold、或以
///   Chapter/Book/Part/Prologue/Epilogue 开头）且不以句末标点结尾；③ 前一段以 ≥2 个 `<br>` 结尾或本身是空段/`* * *`
///   之类的分隔（空段过多的书——用空段当段距——不按分隔算）；④ 文件里第一个有正文的段（章首）。幂等。
/// ⚠ xochitl 认不认内联 style 属性待真机核（书架白皮书 §05 Phase E ②）；不认也只是照常缩进 1.2em，无害。
pub(super) fn flush_first_para_after_heading(html: &str) -> String {
    static P: OnceLock<Regex> = OnceLock::new();
    static CLASS: OnceLock<Regex> = OnceLock::new();
    static STYLE: OnceLock<Regex> = OnceLock::new();
    static TAG: OnceLock<Regex> = OnceLock::new();
    static BR2: OnceLock<Regex> = OnceLock::new();
    static HEAD_WORD: OnceLock<Regex> = OnceLock::new();
    static SEP: OnceLock<Regex> = OnceLock::new();
    // 块序列：h 结束标签 / p 元素（p 内不再嵌 p，非贪婪到最近 </p> 够用）/ 上次洗出的 cj-flush div（重洗幂等）
    let p = P.get_or_init(|| Regex::new(r#"(?is)</h[1-6]>|<p\b([^>]*)>(.*?)</p>|<div\b([^>]*\bcj-flush\b[^>]*)>(.*?)</div>"#).unwrap());
    let class_re = CLASS.get_or_init(|| Regex::new(r#"(?i)\bclass="([^"]*)""#).unwrap());
    let style_re = STYLE.get_or_init(|| Regex::new(r#"(?i)\bstyle="[^"]*""#).unwrap());
    let tag = TAG.get_or_init(|| Regex::new(r#"(?s)<[^>]+>"#).unwrap());
    let br2 = BR2.get_or_init(|| Regex::new(r#"(?is)(<br\b[^>]*>\s*){2,}(</span>|</a>|\s)*$"#).unwrap());
    let head_word = HEAD_WORD.get_or_init(|| Regex::new(r#"(?i)^\s*(chapter|book|part|prologue|epilogue|section|interlude)\b"#).unwrap());
    let sep = SEP.get_or_init(|| Regex::new(r#"^[\s\*#~—–\-·•]*$"#).unwrap());
    let text_of = |inner: &str| -> String {
        let t = tag.replace_all(inner, "");
        t.replace("&#160;", " ").replace("&nbsp;", " ").trim().to_string()
    };
    // 空段太多（>20%）的书是拿空段当段距，不当场景分隔
    let total_p = p.captures_iter(html).filter(|c| c.get(1).is_some() || c.get(3).is_some()).count();
    let empty_p = p.captures_iter(html).filter(|c| c.get(2).or(c.get(4)).map(|m| sep.is_match(&text_of(m.as_str()))).unwrap_or(false)).count();
    let empty_is_sep = total_p == 0 || empty_p * 5 <= total_p;
    let mut flush_next = true; // ④ 章首
    p.replace_all(html, |c: &regex::Captures| {
        let (attrs, inner, already_div) = match (c.get(1), c.get(3)) {
            (Some(a), _) => (a.as_str(), &c[2], false),
            (None, Some(a)) => (a.as_str(), &c[4], true),
            (None, None) => {
                flush_next = true; // ① </hN>
                return c[0].to_string();
            }
        };
        let text = text_of(inner);
        if text.is_empty() || sep.is_match(&text) {
            if empty_is_sep {
                flush_next = true; // ③ 空段 / * * * 分隔
            }
            return c[0].to_string();
        }
        let bold_wrapped = (inner.contains("<b>") || inner.contains("<strong") || inner.contains("bold")) && text.chars().count() <= 80;
        let heading_like = !terminal_latin(&text) && text.chars().count() <= 80 && (bold_wrapped || head_word.is_match(&text));
        let out = if flush_next && !heading_like && !already_div {
            // 只留 cj-flush 一个类、去掉 style：书的类规则（如 `.calibre_ {text-indent:1.2em}`）在 xochitl 里同为类规则时
            // **先出现者胜**（诊断 13/14），带着书的类就压不住；元素/内联通道又都不通（诊断 7–10）。id 等其它属性保留。
            let mut kept = class_re.replace_all(attrs, "").to_string();
            kept = style_re.replace_all(&kept, "").to_string();
            let kept = kept.split_whitespace().collect::<Vec<_>>().join(" ");
            let sep = if kept.is_empty() { "" } else { " " };
            format!("<div class=\"cj-flush\"{sep}{kept}>{inner}</div>")
        } else {
            c[0].to_string()
        };
        // 下一段是否顶格：② 本段是标题样段落；③ 本段以双 <br> 结尾
        flush_next = heading_like || br2.is_match(inner);
        out
    }).into_owned()
}

/// 拉丁段落是否以句末标点结束（标题样段落判定用）。
pub(super) fn terminal_latin(t: &str) -> bool {
    t.trim_end().chars().last().map(|c| ".!?\"'”’)".contains(c)).unwrap_or(false)
}

/// 给 `<head>` 注入指向外链 wash css 的 `<link>`（`href`=该 html 相对 css 的路径）。幂等（已有则跳过）。
/// 无 `</head>` 时补一对 head；无 `<body` 也不动（异常文件）。
pub(super) fn inject_css_link(html: &str, href: &str) -> String {
    let marker = format!("href=\"{href}\"");
    if html.contains(&marker) {
        return html.to_string();
    }
    let link = format!("<link rel=\"stylesheet\" type=\"text/css\" href=\"{href}\"/>");
    if let Some(i) = html.find("</head>") {
        format!("{}{}{}", &html[..i], link, &html[i..])
    } else if let Some(i) = html.find("<body") {
        format!("{}<head>{}</head>{}", &html[..i], link, &html[i..])
    } else {
        html.to_string()
    }
}

/// 外链 wash css 的内容。按 `opts.lang` 注中/英首行缩进习惯 + 段落上下边距归零（除非 keep_para_spacing）。
/// ⚠ 两条 xochitl css 解析器的脆弱性（真机坐实）：
/// ① **只用裸元素选择器 `p{}`**——一条类/相邻/at-rule 选择器就让整表失效（《缩进诊断5》带 `.big` 时连 `p{}` 都不生效）。
/// ② **不用 `!important`**——带 `!important` 的外链规则不生效（v10 真机「都没缩进」），去掉即生效（diag6/《飘》验证）。
/// 我们的 `<link>` 注在 `</head>` 前、晚于书自带 css，同特异性靠源序后者胜，无需 `!important` 也能盖过书里的 `p{text-indent:0}`。
/// `opts.lang` 应已被 `wash_entries` 从 `Auto` 解析为具体值（此处把 `Auto` 兜底当 `Cjk`）。
pub fn wash_css(opts: &WashOpts) -> String {
    // 拉丁 1.2em / 中文 2em（Auto 兜底中文）。
    let indent = indent_for(opts);
    let mut decl = format!("text-indent:{indent};");
    if !opts.keep_para_spacing {
        decl.push_str("margin-top:0;margin-bottom:0;padding-top:0;padding-bottom:0;");
    }
    // ⚠ 每条声明都以 `;` 收尾：xochitl 会丢掉规则里最后一个没分号的声明（2026-09-06 诊断 11/12：`p{text-indent:2em}`
    // 整条不生效、`p{text-indent:2em;}` 生效）——keep_para_spacing 档位此前因此在 xochitl 上没缩进。
    // `.cj-flush`：拉丁首段顶格段（wash_html 换成的 `<div class="cj-flush">`），KOReader 靠它归零缩进/段距；
    // xochitl 上 div 本就无 p 规则、且书的类规则够不到（类已剥），此条只是保险。
    // ⚠ 值用 0.01em 不用 0：xochitl 把 `text-indent:0` 当"没设"→ 落回从外层 `<div class="calibre1">` 之类**继承**来的缩进
    //   （诊断 14 V1/V7 vs Sheldon v5，2026-09-06）；0.01em ≈ 0.1pt 肉眼不可见，KOReader 同样视为顶格。
    let flush = if opts.keep_para_spacing { ".cj-flush{text-indent:0.01em;}" } else { ".cj-flush{text-indent:0.01em;margin-top:0;margin-bottom:0;}" };
    // figure/figcaption：以前这条规则只管了 `<p>` 的边距，`article.rs` 网文管线常把图片包成
    // `<figure><img/><figcaption>…</figcaption></figure>`（真机书里也不算罕见），这两个元素
    // 完全没被清零过——默认（未洗）上下边距在"图片夹在正文中间"的场景会造成明显留白，2026-09-10
    // 真机拿 aeon.co 一篇网文复现坐实（诊断EPUB→投原生→量 xochitl 渲染 PDF，不肉眼猜，见书架
    // 白皮书 §03aq）。**两条规则分开写，不写成 `figure,figcaption{}`**——xochitl 的 CSS 解析器
    // 脆，只认裸元素选择器，逗号/复合选择器直接整条规则失效（`lang_aware_indent` 测试断言过
    // 这条红线，别在这里破例）。keep_para_spacing 档位同样清零：那档的意图是"保留正文段落之间
    // 的呼吸感"，不是"保留图片周围的默认边距"，两件事语义不同，不该被同一个开关连带控制。
    // 注释容器统一比正文小一号（相对单位，随用户当前字号缩放）——书自带 CSS 里已有注释规则的由
    // `filter_css` 就地改；这两条是给"书压根没给注释块写过 CSS"（纯靠我们自己生成的 `.footnotes`
    // 章末块 / `.cj-fnote` Inline 内联注释）兜底，不然那些书的注释永远跟正文同号，不满足这条通用
    // 要求。跟 `.cj-flush` 一样是单个裸类选择器，不逗号连写。
    format!(
        "p{{{decl}}}\n{flush}\nfigure{{margin:0;padding:0;}}\nfigcaption{{margin:0;padding:0;}}\n.footnotes{{font-size:{FOOTNOTE_FONT_SIZE};}}\n.cj-fnote{{font-size:{FOOTNOTE_FONT_SIZE};}}\n"
    )
}

pub fn count_dup_id_tags(html: &str) -> usize {
    static TAG: OnceLock<Regex> = OnceLock::new();
    static ID: OnceLock<Regex> = OnceLock::new();
    let tag = TAG.get_or_init(|| Regex::new(r#"(?s)<[a-zA-Z][^>]*>"#).unwrap());
    let id = ID.get_or_init(|| Regex::new(r#"\bid=""#).unwrap());
    tag.find_iter(html).filter(|m| id.find_iter(m.as_str()).count() > 1).count()
}
