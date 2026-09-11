//! 清洗层单测（原 `wash.rs` 内联的 `mod tests`；用例跨多个子模块，集中放这里）。
    use super::*;

    fn e(name: &str, data: &str) -> Entry {
        Entry { name: name.into(), data: data.as_bytes().to_vec() }
    }
    fn s(entries: &[Entry], name: &str) -> String {
        String::from_utf8(entries.iter().find(|e| e.name == name).unwrap().data.clone()).unwrap()
    }
    const OPF: &str = r#"<?xml version="1.0"?><package version="2.0"><metadata><dc:title>测试书</dc:title></metadata><manifest><item id="css" href="style.css" media-type="text/css"/><item id="dk" href="dkagent.css" media-type="text/css"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="pb" href="pb.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="pb"/><itemref idref="c2"/></spine></package>"#;

    #[test]
    fn decl_filter_and_spacing() {
        let f: Vec<String> = DEFAULT_FILTER_PROPS.iter().map(|s| s.to_string()).collect();
        assert_eq!(filter_decls("font-family:'A';color:#333;text-indent:1em;font:12px x", &f, Spacing::Keep), "color:#333;text-indent:1em;", "color 不再剥离（EPUB 线保留原书颜色）");
        assert_eq!(filter_decls("margin:1em 2em;padding-top:3px;padding-left:4px", &f, Spacing::Vertical), "margin:0 2em;padding-left:4px;");
        assert_eq!(filter_decls("margin:1em 2em 3em 4em", &f, Spacing::Vertical), "margin:0 2em 0 4em;");
        assert_eq!(filter_decls("margin:5pt", &f, Spacing::Vertical), "margin:0 5pt;");
        assert_eq!(filter_decls("margin:5pt;padding:2px;line-height:1.5", &f, Spacing::All), "line-height:1.5;");
        assert_eq!(filter_decls("font-family: &#39;A&#39;; text-indent:2em", &f, Spacing::Keep), "text-indent:2em;", "实体分号不截断");
        assert_eq!(selector_spacing("p.calibre1"), Spacing::Vertical);
        assert_eq!(selector_spacing("div > p"), Spacing::Vertical);
        assert_eq!(selector_spacing("body"), Spacing::All);
        assert_eq!(selector_spacing("@page"), Spacing::All);
        assert_eq!(selector_spacing(".calibre1"), Spacing::Keep);
        assert_eq!(selector_spacing("span, pre"), Spacing::Keep);
    }

    #[test]
    fn css_file_filter_keeps_font_face_and_media() {
        let o = WashOpts::default();
        let css = "@font-face{font-family:X;src:url(x.ttf)} body{margin:5pt;color:#333} p{margin:1em 0;text-align:justify;text-indent:0} @media print{ p{margin-top:2em} } .c{margin:1em}";
        let out = filter_css(css, &o);
        assert!(out.contains("@font-face{font-family:X;src:url(x.ttf)}"), "{out}");
        assert!(out.contains("body{color:#333;}"), "margin 因 Spacing::All 剥、color 保留(不再剥): {out}");
        assert!(out.contains(" p{margin:0 0;text-align:justify;text-indent:0;}"), "text-align 保留(不再剥): {out}");
        assert!(out.contains("p{}"), "media 内规则也处理(单独 margin-top 整条丢弃，与 color/text-align 剥离与否无关): {out}");
        assert!(out.contains(".c{margin:1em;}"), "类选择器不动: {out}");
        let k = WashOpts { keep_para_spacing: true, ..Default::default() };
        assert!(filter_css("p{margin:1em 0}", &k).contains("p{margin:1em 0;}"));
    }

    /// 真机《甲午：摇摆的战争》坐实：`.duokan-footnote-item{font-weight:bold}` 导致全书注释永远加粗，
    /// 用户报的是"跳转注释后字体不对"、追下去其实是加粗。2026-09-23 拍板：只剥**注释容器类**的字重
    /// （不碰正文语义加粗，§03av 原则②仍然有效），注释字号固定比正文小一档（相对单位）。
    #[test]
    fn footnote_container_selector_loses_bold_gains_smaller_relative_size() {
        let o = WashOpts::default();
        assert!(is_footnote_container_selector(".duokan-footnote-item"));
        assert!(is_footnote_container_selector(".footnotes"));
        assert!(is_footnote_container_selector(".cj-fnote"));
        assert!(!is_footnote_container_selector("p"), "普通正文选择器不该被当成注释容器");

        let css = ".duokan-footnote-item{margin:0 0.6em;font-weight:bold;text-align:justify}";
        let out = filter_css(css, &o);
        assert!(!out.contains("font-weight"), "注释容器类的字重必须剥: {out}");
        assert!(out.contains(&format!("font-size:{FOOTNOTE_FONT_SIZE}")), "注释容器类要补字号: {out}");
        assert!(out.contains("text-align:justify"), "其它声明不受影响: {out}");

        // 正文语义加粗不受影响（§03av 原则②）。
        let body = filter_css("p.emphasis{font-weight:bold;color:#333}", &o);
        assert!(body.contains("font-weight:bold"), "正文加粗必须保留: {body}");
    }

    #[test]
    fn strips_background_image_keeps_font_src() {
        let o = WashOpts::default();
        // 分卷页背景图（xochitl 平铺盖正文）应剥；@font-face 的 src:url 保留。
        let css = "@font-face{font-family:F;src:url(f.ttf)} body.fen{background:url(bg.png) no-repeat bottom center;background-size:100% auto;margin:0} .x{background-image:url(y.png);color:#333;text-indent:2em}";
        let out = filter_css(css, &o);
        assert!(!out.contains("bg.png") && !out.contains("y.png"), "背景图应剥: {out}");
        assert!(out.contains("f.ttf"), "@font-face src 保留: {out}");
        assert!(out.contains("text-indent:2em"), "非背景声明保留: {out}");
    }

    #[test]
    fn html_wash_filters_styles_and_collapses_dup_ids() {
        let o = WashOpts::default();
        let html = r#"<html><head><link rel="stylesheet" href="s.css"/></head><body style="margin:5pt"><p id="a" id="b" style="font-size:12px;margin-top:1em;margin-left:2em">x</p><div style="color:gray">y</div><style>p{color:#333;margin:1em}</style></body></html>"#;
        let (out, dups) = wash_html(html, &o);
        assert_eq!(dups, 1);
        assert!(out.contains(r#"<p id="a" style="margin-left:2em;">x</p>"#), "{out}");
        assert!(out.contains(r#"<div style="color:gray;">y</div>"#), "color 不再剥离、style 非空保留: {out}");
        assert!(out.contains("<body>"), "{out}");
        assert!(out.contains("<style>p{color:#333;margin:0 1em;}</style>"), "color 保留 + 1em 四边→上下归零左右保留: {out}");
        // wash_html 不再注入内联 <style>（排版规则改外链 css，由 wash_entries 注）——见 external_css_injected_and_linked。
        assert!(!out.contains(&format!(r#"class="{WASH_MARK}""#)), "不该再注入内联 cj-wash: {out}");
        // 旧版内联 cj-wash 块重洗时清掉
        let (cleaned, _) = wash_html(r#"<html><head><style class="cj-wash">p{text-indent:2em!important}</style></head><body><p>z</p></body></html>"#, &o);
        assert!(!cleaned.contains("cj-wash") && !cleaned.contains("text-indent"), "旧内联块未清: {cleaned}");
    }

    #[test]
    fn pseudo_drm_stripped_and_real_drm_rejected() {
        let mut v = vec![
            e("mimetype", "application/epub+zip"),
            e("META-INF/encryption.xml", r#"<encryption><EncryptedData><CipherData><CipherReference URI="dkagent.css"/></CipherData></EncryptedData></encryption>"#),
            e("content.opf", OPF),
            e("dkagent.css", "secret"),
            e("style.css", "p{}"),
            e("c1.xhtml", "<html><body><p>a</p></body></html>"),
            e("pb.xhtml", "<html><body><p>b</p></body></html>"),
            e("c2.xhtml", "<html><body><p>c</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.pseudo_drm_stripped, vec!["dkagent.css"]);
        assert!(!v.iter().any(|x| x.name == "dkagent.css" || x.name == "META-INF/encryption.xml"));
        assert!(!s(&v, "content.opf").contains("dkagent.css"), "manifest 项删除");
        let mut real = vec![e("META-INF/encryption.xml", r#"<CipherReference URI="OEBPS/c1.xhtml"/><CipherReference URI="a.ttf"/>"#), e("OEBPS/c1.xhtml", "")];
        let err = wash_entries(&mut real, &WashOpts::default()).unwrap_err();
        assert!(err.contains("真 DRM") && err.contains("OEBPS/c1.xhtml"), "{err}");
    }

    #[test]
    fn drop_opf_refs_removes_only_listed_ids_in_one_pass() {
        // 一遍扫描删多个页的 item/itemref：带空闭合标签/属性顺序不同/相邻空白都要处理干净，不误伤 id 相近的项（p1 vs p10、idref 别名）。
        let opf = concat!(
            "<manifest>\n<item id=\"p1\" href=\"p1.xhtml\"/>\n<item href=\"p10.xhtml\" id=\"p10\" media-type=\"x\"></item>\n",
            "<item id=\"p2\" href=\"p2.xhtml\"/>\n<item id=\"img\" href=\"a.png\"/></manifest>\n",
            "<spine>\n<itemref idref=\"p1\"/>\n<itemref linear=\"yes\" idref=\"p10\"></itemref>\n<itemref idref=\"p2\"/>\n</spine>"
        );
        let ids: HashSet<&str> = ["p1", "p10"].into_iter().collect();
        let out = drop_opf_refs(opf, &ids);
        assert!(!out.contains("p1.xhtml") && !out.contains("p10.xhtml") && !out.contains("idref=\"p1\"") && !out.contains("idref=\"p10\""), "{out}");
        assert!(out.contains("<item id=\"p2\" href=\"p2.xhtml\"/>") && out.contains("<itemref idref=\"p2\"/>") && out.contains("a.png"), "无关项原样: {out}");
        assert_eq!(drop_opf_refs(opf, &HashSet::new()), opf, "空集合 = 原样");
    }

    #[test]
    fn empty_page_removed_and_toc_retargeted() {
        let mut v = vec![
            e("content.opf", OPF),
            e("style.css", ""),
            e("dkagent.css", ""),
            e("toc.ncx", r#"<ncx><navMap><navPoint><content src="c1.xhtml"/></navPoint><navPoint><content src="pb.xhtml#x"/></navPoint></navMap></ncx>"#),
            e("c1.xhtml", "<html><body><p>a</p></body></html>"),
            e("pb.xhtml", r#"<html><body><div class="mbppagebreak"></div>&nbsp;</body></html>"#),
            e("c2.xhtml", "<html><body><p>c</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts { auto_toc: AutoToc::Off, ..Default::default() }).unwrap();
        assert_eq!(rep.empty_pages_removed, vec!["pb.xhtml"]);
        assert!(!v.iter().any(|x| x.name == "pb.xhtml"));
        let opf = s(&v, "content.opf");
        assert!(!opf.contains(r#"idref="pb""#) && !opf.contains(r#"id="pb""#), "{opf}");
        assert!(s(&v, "toc.ncx").contains(r#"src="c2.xhtml""#), "指向空页的目录改指下一篇: {}", s(&v, "toc.ncx"));
        // 有图的页不算空
        assert!(!is_empty_page(r#"<html><body><img src="a.png"/></body></html>"#));
    }

    fn cover_book(meta: &str) -> Vec<Entry> {
        let opf = format!(r#"<package version="2.0"><metadata><dc:title>书</dc:title>{meta}</metadata><manifest><item id="p1" href="Text/p1.xhtml" media-type="application/xhtml+xml"/><item id="img1" href="Images/001.jpg" media-type="image/jpeg"/><item id="cover.txt" href="cover.txt" media-type="text/plain"/></manifest><spine><itemref idref="p1"/></spine></package>"#);
        vec![
            e("content.opf", &opf),
            e("Text/p1.xhtml", r#"<html><body><img src="../Images/001.jpg"/></body></html>"#),
            Entry { name: "Images/001.jpg".into(), data: Vec::new() },
            e("cover.txt", "not an image"),
        ]
    }

    #[test]
    fn ensure_cover_adds_declaration_from_first_spine_page_image() {
        let mut v = cover_book("");
        assert!(ensure_cover_declared(&mut v));
        let opf = s(&v, "content.opf");
        assert!(opf.contains(r#"<meta name="cover" content="img1"/>"#), "{opf}");
        assert!(opf.contains(r#"id="img1" href="Images/001.jpg" media-type="image/jpeg" properties="cover-image""#) || opf.contains("cover-image"), "{opf}");
        assert!(!ensure_cover_declared(&mut v), "已有有效声明，第二次不该再动（幂等）");
    }

    #[test]
    fn ensure_cover_repairs_declaration_pointing_to_non_image() {
        // Calibre 产物：<meta name="cover" content="cover.txt"/> 指向 txt——xochitl 取不到封面（真机日志 null cover image）
        let mut v = cover_book(r#"<meta name="cover" content="cover.txt"/>"#);
        assert!(ensure_cover_declared(&mut v));
        let opf = s(&v, "content.opf");
        assert!(opf.contains(r#"<meta name="cover" content="img1"/>"#) && !opf.contains(r#"content="cover.txt""#), "{opf}");
    }

    #[test]
    fn ensure_cover_finds_svg_titlepage_image_before_page_referencing_bad_file() {
        // 《镖人(卷四)》：spine 首页是只含 SVG <image> 的 titlepage（真封面），第二页 cover.xhtml 的 <img> 指向坏文件 cover.txt。
        let opf = r#"<package version="2.0"><metadata><meta name="cover" content="cover.txt"/></metadata><manifest><item id="titlepage" href="titlepage.xhtml" media-type="application/xhtml+xml"/><item id="cover.xhtml" href="EPUB/xhtml/cover.xhtml" media-type="application/xhtml+xml"/><item id="cover.txt" href="EPUB/images/cover.txt" media-type="application/xhtml+xml"/><item id="image_000.jpg" href="EPUB/images/image_000.jpg" media-type="image/jpeg"/></manifest><spine><itemref idref="titlepage"/><itemref idref="cover.xhtml"/></spine></package>"#;
        let mut v = vec![
            e("content.opf", opf),
            e("titlepage.xhtml", r#"<html><body><svg xmlns:xlink="x"><image width="600" xlink:href="EPUB/images/image_000.jpg"/></svg></body></html>"#),
            e("EPUB/xhtml/cover.xhtml", r#"<html><body><img src="../images/cover.txt"/></body></html>"#),
            e("EPUB/images/cover.txt", "<?xml not an image"),
            Entry { name: "EPUB/images/image_000.jpg".into(), data: Vec::new() },
        ];
        assert!(ensure_cover_declared(&mut v));
        let o = s(&v, "content.opf");
        assert!(o.contains(r#"<meta name="cover" content="image_000.jpg"/>"#) && !o.contains(r#"content="cover.txt""#), "{o}");
    }

    #[test]
    fn ensure_cover_falls_back_to_first_real_image_when_cover_file_itself_is_broken() {
        // 封面页引用的图片文件本身是坏的（txt 残片）：跳过，兜底用后面页面里第一张真实图片。
        let opf = r#"<package version="2.0"><metadata><meta name="cover" content="cover.txt"/></metadata><manifest><item id="titlepage" href="titlepage.xhtml" media-type="application/xhtml+xml"/><item id="p2" href="p2.xhtml" media-type="application/xhtml+xml"/><item id="cover.txt" href="images/cover.txt" media-type="application/xhtml+xml"/><item id="image_000.jpg" href="images/image_000.jpg" media-type="image/jpeg"/></manifest><spine><itemref idref="titlepage"/><itemref idref="p2"/></spine></package>"#;
        let mut v = vec![
            e("content.opf", opf),
            e("titlepage.xhtml", r#"<html><body><svg><image xlink:href="images/cover.txt"/></svg></body></html>"#),
            e("p2.xhtml", r#"<html><body><img src="images/image_000.jpg"/></body></html>"#),
            e("images/cover.txt", "<?xml"),
            Entry { name: "images/image_000.jpg".into(), data: Vec::new() },
        ];
        assert!(ensure_cover_declared(&mut v));
        assert!(s(&v, "content.opf").contains(r#"<meta name="cover" content="image_000.jpg"/>"#));
    }

    #[test]
    fn ensure_cover_adds_property_when_meta_is_valid_but_item_lacks_cover_image() {
        // 火影 09：meta 有效、条目 id 带点（x00000001.jpg），但没有 properties="cover-image"——真机对照实验证明 xochitl 因此取不到封面。
        let mut v = cover_book(r#"<meta name="cover" content="img1"/>"#);
        assert!(ensure_cover_declared(&mut v), "meta 有效但缺属性也要补");
        assert!(s(&v, "content.opf").contains(r#"properties="cover-image""#));
        assert!(!ensure_cover_declared(&mut v), "补完幂等");
    }

    #[test]
    fn ensure_cover_leaves_fully_valid_declaration_and_unfindable_alone() {
        let mut ok = cover_book(r#"<meta name="cover" content="img1"/>"#);
        ok[0] = e("content.opf", &s(&ok, "content.opf").replace(r#"<item id="img1" href="Images/001.jpg" media-type="image/jpeg"/>"#, r#"<item id="img1" href="Images/001.jpg" media-type="image/jpeg" properties="cover-image"/>"#));
        let before = s(&ok, "content.opf");
        assert!(!ensure_cover_declared(&mut ok));
        assert_eq!(s(&ok, "content.opf"), before);
        // 第一页没有图：没有候选，不乱猜
        let mut none = cover_book("");
        none[1] = e("Text/p1.xhtml", "<html><body><p>纯文字</p></body></html>");
        assert!(!ensure_cover_declared(&mut none));
    }

    #[test]
    fn image_only_book_with_empty_ncx_gets_page_range_toc() {
        // 乱马/火影同款：25 个纯图片页、NCX 存在但 navMap 为空 → 生成"第 N–M 页"分段目录，且指向真实页面。
        let items: String = (1..=25).map(|i| format!(r#"<item id="p{i}" href="Text/p{i}.xhtml" media-type="application/xhtml+xml"/>"#)).collect();
        let spine: String = (1..=25).map(|i| format!(r#"<itemref idref="p{i}"/>"#)).collect();
        let opf = format!(r#"<package version="2.0" unique-identifier="id"><metadata><dc:title>漫画</dc:title><dc:identifier id="id">urn:x</dc:identifier></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>{items}</manifest><spine toc="ncx">{spine}</spine></package>"#);
        let mut es = vec![
            e("content.opf", &opf),
            e("toc.ncx", r#"<ncx><head/><docTitle><text>Unknown</text></docTitle><navMap></navMap></ncx>"#),
        ];
        for i in 1..=25 {
            es.push(e(&format!("Text/p{i}.xhtml"), r#"<html><body><img src="../Images/x.jpg"/></body></html>"#));
        }
        assert_eq!(toc_entry_count(&es), 0);
        let mut rep = WashReport::default();
        auto_toc(&mut es, AutoToc::IfMissing, &mut rep);
        assert_eq!(rep.toc_generated, 2, "25 页 → 20+5 两段");
        let ncx = String::from_utf8_lossy(&es.iter().find(|x| x.name == "toc.ncx").unwrap().data).to_string();
        assert!(ncx.contains("第 1–20 页") && ncx.contains("第 21–25 页"), "{ncx}");
        assert!(ncx.contains("Text/p1.xhtml") && ncx.contains("Text/p21.xhtml"), "目录必须指向段首页: {ncx}");
        assert!(toc_entry_count(&es) > 0);
    }

    #[test]
    fn auto_toc_generated_only_when_missing() {
        let mk = || vec![
            e("OEBPS/content.opf", r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="c1" href="text/c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="text/c2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#),
            e("OEBPS/text/c1.xhtml", "<html><body><h1>第一章</h1><p>a</p><h2 id=\"s1\">一节</h2></body></html>"),
            e("OEBPS/text/c2.xhtml", "<html><body><h1>第<i>二</i>章</h1><p>b</p></body></html>"),
        ];
        let mut v = mk();
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.toc_generated, 3);
        let ncx = s(&v, "OEBPS/toc.ncx");
        assert!(ncx.contains(r#"src="text/c1.xhtml#cj-toc-1""#) && ncx.contains(r#"src="text/c1.xhtml#s1""#) && ncx.contains("第二章"), "{ncx}");
        let nav = s(&v, "OEBPS/nav.xhtml");
        assert!(nav.contains(r#"<li><a href="text/c1.xhtml#cj-toc-1">第一章</a><ol><li><a href="text/c1.xhtml#s1">一节</a></li></ol></li>"#), "{nav}");
        let opf = s(&v, "OEBPS/content.opf");
        assert!(opf.contains(r#"toc="ncx""#) && opf.contains(r#"properties="nav""#), "{opf}");
        assert!(s(&v, "OEBPS/text/c1.xhtml").contains(r#"<h1 id="cj-toc-1">"#));
        assert_eq!(toc_entry_count(&v), 6, "ncx 3 + nav 3");
        // 已有目录 → IfMissing 不动
        let mut w = mk();
        w.push(e("OEBPS/toc.ncx", r#"<ncx><navMap><navPoint><content src="text/c1.xhtml"/></navPoint></navMap></ncx>"#));
        let rep = wash_entries(&mut w, &WashOpts::default()).unwrap();
        assert_eq!(rep.toc_generated, 0);
        assert!(!w.iter().any(|x| x.name == "OEBPS/nav.xhtml"));
    }

    /// 标题/书名里的字符引用（`&amp;`、`&#12288;` 全角空格）：自动目录只转义一次，不再出现 `&amp;amp;`、`&amp;#12288;`。
    #[test]
    fn auto_toc_titles_are_not_double_escaped() {
        let mut v = vec![
            e("OEBPS/content.opf", r#"<package version="3.0"><metadata><dc:title>Tom &amp; Jerry</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#),
            e("OEBPS/c1.xhtml", "<html><body><h1>第一章&#12288;猫 &amp; 鼠 &lt;上&gt;</h1><p>a</p></body></html>"),
        ];
        wash_entries(&mut v, &WashOpts::default()).unwrap();
        for f in ["OEBPS/toc.ncx", "OEBPS/nav.xhtml"] {
            let t = s(&v, f);
            assert!(t.contains("第一章 猫 &amp; 鼠 &lt;上&gt;"), "{f}: {t}");
            assert!(!t.contains("&amp;amp;") && !t.contains("&amp;#") && !t.contains("&amp;lt;"), "{f}: {t}");
        }
        assert!(s(&v, "OEBPS/toc.ncx").contains("<docTitle><text>Tom &amp; Jerry</text></docTitle>"));
    }

    #[test]
    fn existing_ncx_dtb_uid_synced_to_opf_identifier() {
        // 真机回归（2026-09-19，《疯探》）：navMap 结构完全正确，但 dtb:uid 是第三方生成器随手写的
        // 另一个 uuid，跟 OPF 的 dc:identifier 对不上——reMarkable 原生目录面板遇到这种不匹配
        // 直接不显示目录入口（不是空列表），换一本 dtb:uid 匹配的书目录入口就在。
        let opf = r#"<package version="2.0" unique-identifier="bookid"><metadata><dc:title>书</dc:title><dc:identifier id="bookid">urn:uuid:real-book-id</dc:identifier></metadata><manifest><item id="toc" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="toc"><itemref idref="c1"/></spine></package>"#;
        let mut v = vec![
            e("content.opf", opf),
            e("toc.ncx", r#"<ncx><head><meta name="dtb:uid" content="urn:uuid:stale-generator-id"/></head><navMap><navPoint><navLabel><text>章一</text></navLabel><content src="c1.xhtml"/></navPoint></navMap></ncx>"#),
            e("c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.ncx_uid_fixed, 1);
        let ncx = s(&v, "toc.ncx");
        assert!(ncx.contains(r#"content="urn:uuid:real-book-id""#), "dtb:uid 该改成跟 OPF 一致: {ncx}");
        assert!(!ncx.contains("stale-generator-id"), "旧的错误 uid 不该残留: {ncx}");
        // 已经一致时不误报、不改动字节（幂等）
        let mut w = vec![
            e("content.opf", opf),
            e("toc.ncx", r#"<ncx><head><meta name="dtb:uid" content="urn:uuid:real-book-id"/></head><navMap><navPoint><navLabel><text>章一</text></navLabel><content src="c1.xhtml"/></navPoint></navMap></ncx>"#),
            e("c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep2 = wash_entries(&mut w, &WashOpts::default()).unwrap();
        assert_eq!(rep2.ncx_uid_fixed, 0, "已经一致不该误报修复过");
    }

    #[test]
    fn ncx_manifest_id_renamed_to_ncx_and_spine_toc_synced() {
        // 真机回归（2026-09-19，《疯探》，反编译 xochitl 二进制坐实）：dtb:uid、DOCTYPE 都修一致
        // 后原生目录入口依然不出现——根因是 xochitl 定位目录文件硬编码死查 manifest 里 id="ncx"，
        // 不是走 `<spine toc="IDREF">`；《疯探》完全合规的 `id="toc"` + `<spine toc="toc">`
        // 因此找不到。只改这一个 id 名字，真机验证 94 条章节标题全部恢复。
        let opf = r#"<package version="2.0"><metadata><dc:title>疯探</dc:title></metadata><manifest><item id="toc" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="text/c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="toc"><itemref idref="c1"/></spine></package>"#;
        let mut v = vec![
            e("content.opf", opf),
            e("toc.ncx", r#"<ncx><navMap><navPoint><navLabel><text>第一章</text></navLabel><content src="text/c1.xhtml"/></navPoint></navMap></ncx>"#),
            e("text/c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.ncx_manifest_id_fixed, 1);
        let opf_out = s(&v, "content.opf");
        assert!(opf_out.contains(r#"<item id="ncx" href="toc.ncx""#), "manifest 里 ncx 条目的 id 该改成 \"ncx\": {opf_out}");
        assert!(opf_out.contains(r#"<spine toc="ncx">"#), "spine 的 toc 属性该同步指向新 id: {opf_out}");
        assert!(!opf_out.contains(r#"id="toc""#), "旧 id 不该残留: {opf_out}");
        // 已经叫 "ncx" 时原样不动、不误报（幂等）
        let already_ok = r#"<package version="2.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="ncx"><itemref idref="c1"/></spine></package>"#;
        let mut w = vec![
            e("content.opf", already_ok),
            e("toc.ncx", r#"<ncx><navMap><navPoint><navLabel><text>第一章</text></navLabel><content src="c1.xhtml"/></navPoint></navMap></ncx>"#),
            e("c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep2 = wash_entries(&mut w, &WashOpts::default()).unwrap();
        assert_eq!(rep2.ncx_manifest_id_fixed, 0, "已经叫 ncx 不该误报修复过");
    }

    #[test]
    fn ncx_external_doctype_stripped() {
        // 真机回归（2026-09-19，dtb:uid 修一致后原生目录入口仍不出现）：《疯探》"番茄小说 EPUB
        // Generator" 产物的 toc.ncx 带外部 DTD 引用（daisy.org），《雪人》没有——这是两本书唯一
        // 剩下的结构性差异。剥掉不改变 NCX 语义，只去掉这个外部依赖。
        let ncx_with_doctype = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE ncx PUBLIC \"-//NISO//DTD ncx 2005-1//EN\" \"http://www.daisy.org/z3986/2005/ncx-2005-1.dtd\">\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\"><navMap><navPoint><navLabel><text>章一</text></navLabel><content src=\"c1.xhtml\"/></navPoint></navMap></ncx>";
        let mut v = vec![
            e("content.opf", r#"<package version="2.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="toc" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="toc"><itemref idref="c1"/></spine></package>"#),
            e("toc.ncx", ncx_with_doctype),
            e("c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.ncx_doctype_stripped, 1);
        let ncx = s(&v, "toc.ncx");
        assert!(!ncx.to_ascii_uppercase().contains("DOCTYPE"), "DOCTYPE 该被剥掉: {ncx}");
        assert!(ncx.contains("<navPoint>") || ncx.contains(r#"<content src="c1.xhtml"/>"#), "navMap 内容不该被动: {ncx}");
        // 没有 DOCTYPE 的书原样不动、不误报
        let mut w = vec![
            e("content.opf", r#"<package version="2.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="toc" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="toc"><itemref idref="c1"/></spine></package>"#),
            e("toc.ncx", r#"<ncx><navMap><navPoint><navLabel><text>章一</text></navLabel><content src="c1.xhtml"/></navPoint></navMap></ncx>"#),
            e("c1.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep2 = wash_entries(&mut w, &WashOpts::default()).unwrap();
        assert_eq!(rep2.ncx_doctype_stripped, 0, "没有 DOCTYPE 不该误报剥过");
    }

    #[test]
    fn existing_flat_toc_with_part_prefix_restructured_into_two_levels() {
        // 真机回归（2026-09-19，《雪人》）：书自带扁平 toc.ncx，条目形如"第一部　01　雪人"
        // （首条，部+编号+章名）/"　02　卵石眼"（后续，只有编号+章名，隐式归属同一部）——原生
        // 目录面板显示的是完全扁平的列表，"01 雪人"没有嵌在"第一部"下面。
        let opf = r#"<package version="2.0" unique-identifier="BookId"><metadata><dc:title>雪人</dc:title><dc:identifier id="BookId">www.haodoo.net</dc:identifier></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="2.xhtml" media-type="application/xhtml+xml"/><item id="c10" href="10.xhtml" media-type="application/xhtml+xml"/><item id="c11" href="11.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="ncx"><itemref idref="c1"/><itemref idref="c2"/><itemref idref="c10"/><itemref idref="c11"/></spine></package>"#;
        let ncx = r#"<ncx><head><meta name="dtb:uid" content="www.haodoo.net"/></head><navMap>
<navPoint><navLabel><text>第一部　01　雪人</text></navLabel><content src="1.xhtml"/></navPoint>
<navPoint><navLabel><text>　02　卵石眼</text></navLabel><content src="2.xhtml"/></navPoint>
<navPoint><navLabel><text>第二部　10　粉筆</text></navLabel><content src="10.xhtml"/></navPoint>
<navPoint><navLabel><text>　11　死亡面具</text></navLabel><content src="11.xhtml"/></navPoint>
</navMap></ncx>"#;
        let mut v = vec![
            e("content.opf", opf),
            e("toc.ncx", ncx),
            e("1.xhtml", "<html><body><p>正文</p></body></html>"),
            e("2.xhtml", "<html><body><p>正文</p></body></html>"),
            e("10.xhtml", "<html><body><p>正文</p></body></html>"),
            e("11.xhtml", "<html><body><p>正文</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.toc_parts_restructured, 6, "2 个分部标题 + 4 条章节＝6 条");
        let out = s(&v, "toc.ncx");
        // 分部标题单独成父级、指向自己这条的目标（第一部开篇就是 1.xhtml）
        assert!(out.contains(r#"<text>第一部</text></navLabel><content src="1.xhtml"/>"#), "{out}");
        // "01 雪人" 降一级挂在"第一部"下面，同样指向 1.xhtml（分部标题那条自己也是这一章）；
        // plain_text 会把全角空格归一成半角（split_whitespace 统一处理，跟标题里其它空白一视同仁）
        assert!(out.contains(r#"<text>01 雪人</text></navLabel><content src="1.xhtml"/>"#), "{out}");
        // "02 卵石眼"（原来没有分部前缀）也降一级，挂在当前活跃的"第一部"下
        assert!(out.contains(r#"<text>02 卵石眼</text></navLabel><content src="2.xhtml"/>"#), "{out}");
        assert!(out.contains(r#"<text>第二部</text></navLabel><content src="10.xhtml"/>"#), "{out}");
        assert!(out.contains(r#"<text>10 粉筆</text></navLabel><content src="10.xhtml"/>"#), "{out}");
        // dtb:depth 该反映真的有两层
        assert!(out.contains(r#"<meta name="dtb:depth" content="2""#), "{out}");
        // 没有分部信号的书原样不动（不该被误伤）
        let opf2 = r#"<package version="2.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/></manifest><spine toc="ncx"><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#;
        let ncx2 = r#"<ncx><navMap><navPoint><navLabel><text>楔子</text></navLabel><content src="c1.xhtml"/></navPoint><navPoint><navLabel><text>尾声</text></navLabel><content src="c2.xhtml"/></navPoint></navMap></ncx>"#;
        let mut w = vec![e("content.opf", opf2), e("toc.ncx", ncx2), e("c1.xhtml", "<html><body><p>a</p></body></html>"), e("c2.xhtml", "<html><body><p>b</p></body></html>")];
        let rep2 = wash_entries(&mut w, &WashOpts::default()).unwrap();
        assert_eq!(rep2.toc_parts_restructured, 0, "没有'第X部'前缀不该重建");
    }

    #[test]
    fn lang_aware_indent() {
        // 只用裸 p{} 元素选择器（xochitl 解析器脆）、不带 !important（xochitl 吃不下）；中文 2em / 拉丁 1.2em。
        let cjk = wash_css(&WashOpts { lang: LangMode::Cjk, ..Default::default() });
        assert!(cjk.contains("text-indent:2em;") && !cjk.contains("1.2em") && !cjk.contains("!important"));
        assert!(cjk.starts_with("p{") && !cjk.contains(',') && !cjk.contains('+') && !cjk.contains('@'), "禁用复杂/逗号选择器: {cjk}");
        assert!(cjk.contains("padding-bottom:0;}"), "每条规则以分号收尾（xochitl 丢最后一个无分号声明）: {cjk}");
        let lat = wash_css(&WashOpts { lang: LangMode::Latin, ..Default::default() });
        assert!(lat.contains("text-indent:1.2em") && !lat.contains("!important"));
        // keep_para_spacing 时不归零段距
        let keep = wash_css(&WashOpts { keep_para_spacing: true, ..Default::default() });
        assert!(!keep.contains("margin-top:0") && keep.contains("p{text-indent:2em;}"), "keep-spacing 也要尾分号: {keep}");
    }

    #[test]
    fn figure_and_figcaption_margin_zeroed_as_separate_bare_rules() {
        // 2026-09-10 真机 aeon.co 网文复现：figure/figcaption 默认边距没清零，图片夹在正文中间
        // 造成留白。修法＝跟 p 一样清零，但必须各自一条裸元素选择器规则——xochitl 解析器脆，
        // `figure,figcaption{}` 这种逗号选择器整条规则会失效（lang_aware_indent 测试断言过这条
        // 红线：不带逗号/复合选择器）。
        let css = wash_css(&WashOpts::default());
        assert!(css.contains("figure{margin:0;padding:0;}"), "{css}");
        assert!(css.contains("figcaption{margin:0;padding:0;}"), "{css}");
        assert!(!css.contains("figure,figcaption") && !css.contains("figcaption,figure"), "禁止逗号选择器: {css}");
        // keep_para_spacing 只管段落呼吸感，不该连带保留图片边距——不管这个档位开没开，figure/figcaption 都清零。
        let keep = wash_css(&WashOpts { keep_para_spacing: true, ..Default::default() });
        assert!(keep.contains("figure{margin:0;padding:0;}") && keep.contains("figcaption{margin:0;padding:0;}"), "{keep}");
    }

    /// 兜底：书压根没给注释块写过 CSS（纯靠我们自己生成的 `.footnotes`/`.cj-fnote`）时，"注释比正文
    /// 小一号"这条要求也得满足，不能只靠 `filter_css` 改书自带规则那条路（那条路对这种书压根碰不到）。
    #[test]
    fn wash_css_gives_footnote_classes_smaller_relative_size_as_bare_selectors() {
        let css = wash_css(&WashOpts::default());
        assert!(css.contains(&format!(".footnotes{{font-size:{FOOTNOTE_FONT_SIZE};}}")), "{css}");
        assert!(css.contains(&format!(".cj-fnote{{font-size:{FOOTNOTE_FONT_SIZE};}}")), "{css}");
        assert!(!css.contains(".footnotes,.cj-fnote") && !css.contains(".cj-fnote,.footnotes"), "禁止逗号选择器: {css}");
    }

    #[test]
    fn external_css_injected_and_linked() {
        // 端到端：英文书 wash 后——排版规则进外链 cangjie-wash.css、每章 <link> 指向它、OPF manifest 补 item。
        let mut v = vec![
            e("OEBPS/content.opf", r#"<package version="3.0"><metadata><dc:title>B</dc:title></metadata><manifest><item id="c1" href="Text/c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#),
            e("OEBPS/Text/c1.xhtml", "<html><head></head><body><h2>Chapter One</h2><p>English prose flowing across the page with many words indeed here</p></body></html>"),
        ];
        wash_entries(&mut v, &WashOpts::default()).unwrap();
        // 外链 css 存在且带拉丁缩进（英文书探测为 Latin）
        let css = s(&v, "OEBPS/cangjie-wash.css");
        assert!(css.contains("text-indent:1.2em") && !css.contains("!important"), "外链 css 应含拉丁缩进、无 !important: {css}");
        // 章节 <link> 相对路径正确（Text/ 下 → ../cangjie-wash.css），且不再有内联 text-indent
        let c1 = s(&v, "OEBPS/Text/c1.xhtml");
        assert!(c1.contains(r#"href="../cangjie-wash.css""#), "章节 link 路径错: {c1}");
        assert!(!c1.contains("text-indent:1.2em"), "通用缩进规则不该内联进 html: {c1}");
        assert!(c1.contains(r#"Chapter One</h2><div class="cj-flush">"#), "拉丁：标题后首段换 div 顶格: {c1}");
        assert!(css.contains(".cj-flush{text-indent:0.01em;margin-top:0;margin-bottom:0;}"), "外链 css 带 cj-flush 规则（0.01em 压继承）且尾分号: {css}");
        // OPF manifest 补了 item（相对 opf 目录 = cangjie-wash.css）
        let opf = s(&v, "OEBPS/content.opf");
        assert!(opf.contains(r#"href="cangjie-wash.css""#) && opf.contains("text/css"), "manifest 未补 item: {opf}");
        // 幂等：重洗不重复加 link / item
        wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(s(&v, "OEBPS/Text/c1.xhtml").matches("cangjie-wash.css").count(), 1, "link 重复");
        assert_eq!(s(&v, "OEBPS/content.opf").matches("cangjie-wash.css").count(), 1, "manifest item 重复");
    }

    #[test]
    fn book_css_indent_harmonized_and_first_para_flush() {
        // 书自带类规则的非零 text-indent 改成本书缩进；0 与负值保留；拉丁标题后首段内联不缩进且幂等
        let lat = WashOpts { lang: LangMode::Latin, ..Default::default() };
        let css = filter_css(".calibre_ {display:block;text-indent:2em;margin:0} .quote{text-indent:0;margin-left:2em} .hang{text-indent:-1.5em} p{text-indent:3%}", &lat);
        assert!(css.contains(".calibre_ {display:block;text-indent:1.2em;margin:0;}"), "{css}");
        assert!(css.contains(".quote{text-indent:0;margin-left:2em;}") && css.contains(".hang{text-indent:-1.5em;}"), "{css}");
        assert!(css.contains("p{text-indent:1.2em;}"), "百分比也算非零: {css}");
        let cjk = WashOpts { lang: LangMode::Cjk, ..Default::default() };
        assert!(filter_css(".calibre_ {text-indent:1.2em}", &cjk).contains("text-indent:2em"));
        let (h, _) = wash_html(r#"<html><body><h1 id="a">T</h1>
<div class="x"><p class="c" style="color:red;text-indent:2em">first</p><p>second</p></div><h2>U</h2><p style="text-indent:0">already</p></body></html>"#, &lat);
        assert!(h.contains(r#"<div class="cj-flush">first</div>"#), "只留 cj-flush、剥 class/style: {h}");
        assert!(h.contains(r#"<p>second</p>"#), "第二段不动: {h}");
        assert_eq!(h.matches("cj-flush").count(), 2, "h1 后与 h2 后各一段: {h}");
        let (h2, _) = wash_html(&h, &lat);
        assert_eq!(h2.matches("cj-flush").count(), 2, "幂等: {h2}");
        let (c, _) = wash_html("<html><body><h1>T</h1><p>x</p></body></html>", &cjk);
        assert!(!c.contains("text-indent:0"), "中文不做首段不缩进: {c}");
    }

    #[test]
    fn cjk_br_book_paragraphized_and_fullwidth_indent_stripped() {
        let cjk = WashOpts { lang: LangMode::Cjk, ..Default::default() };
        // 《人骨拼圖》形态：一章一个 div，h3 + 双 br + 每段全角空格开头、单 br 分段，无 <p>
        let src = "<html><head></head><body class=\"calibre\">\n<div class=\"calibre1\">\n<h3 class=\"calibre3\">14</h3><br class=\"calibre2\"/><br class=\"calibre2\"/>　　這間辦公室高居在曼哈頓下城高處。<br class=\"calibre2\"/>　　「對不起？長官？」<br class=\"calibre2\"/>　　嚴格說來，她不能算是。<br class=\"calibre2\"/></div></body></html>";
        let (h, _) = wash_html(src, &cjk);
        assert!(h.contains("<div class=\"calibre1\">\n<h3 class=\"calibre3\">14</h3>"), "块级标签原样: {h}");
        assert!(h.contains("<p>這間辦公室高居在曼哈頓下城高處。</p><p>「對不起？長官？」</p><p>嚴格說來，她不能算是。</p></div>"), "按 br 段落化且剥全角空格: {h}");
        assert!(!h.contains("<br") && !h.contains('　'), "br 与全角空格都不剩: {h}");
        // 有 <p> 的书：不动 br，但剥段首全角空格/nbsp
        let (h2, _) = wash_html("<html><body><p>　　第一段。</p><p>&#160;&#160;第二段。<br/>换行</p></body></html>", &cjk);
        assert!(h2.contains("<p>第一段。</p><p>第二段。<br/>换行</p>"), "{h2}");
        // 拉丁书不做段落化
        let lat = WashOpts { lang: LangMode::Latin, ..Default::default() };
        let (h3, _) = wash_html("<html><body><div>line one<br/>line two<br/>line three<br/>line four<br/>five</div></body></html>", &lat);
        assert!(h3.contains("line one<br/>line two"), "{h3}");
    }

    #[test]
    fn latin_flush_after_bold_heading_scene_break_and_chapter_start() {
        // 《Tell Me Your Dreams》形态：章名=加粗段落（非 <h>），场景切换=段末双 <br/>，无空段
        let lat = WashOpts { lang: LangMode::Latin, ..Default::default() };
        let src = r#"<html><body><div><p class="calibre_"><a href="x.html#1"><span class="bold"><span class="underline">Chapter Three</span></span></a></p><p class="calibre_"><span class="bold">I</span>N another place, at another time, Alette Peters could have been a successful artist.</p><p class="calibre_">Her father’s voice was blue.</p><p class="calibre_">The sound of running water was gray.<br class="calibre3"/><br class="calibre3"/></p><p class="calibre_">Alette Peters was twenty years old.</p><p class="calibre_">She could be plain-looking.</p><p class="calibre_">* * *</p><p class="calibre_">After the break.</p><p class="calibre_">Still after.</p></div></body></html>"#;
        let (h, _) = wash_html(src, &lat);
        assert_eq!(h.matches("cj-flush").count(), 3, "章首正文 + 双br 后 + * * * 后各一段: {h}");
        assert!(h.contains(r#"<div class="cj-flush"><span class="bold">I</span>N another"#), "章首正文顶格＝换成只带 cj-flush 的 div（章名段本身不算）: {h}");
        assert!(h.contains(r#"<div class="cj-flush">Alette Peters was twenty"#), "双 br 后顶格: {h}");
        assert!(h.contains(r#"<div class="cj-flush">After the break.</div>"#), "* * * 后顶格: {h}");
        assert!(h.contains(r#"<p class="calibre_">Her father"#) && h.contains(r#"<p class="calibre_">Still after"#), "普通续段仍是 p: {h}");
        assert!(h.contains(r#"<p class="calibre_"><a href="x.html#1">"#), "章名段自己不动: {h}");
        // 拿空段当段距的书（空段 > 20%）：空段不算场景分隔
        let spaced = r#"<html><body><p>One.</p><p></p><p>Two.</p><p></p><p>Three.</p><p></p><p>Four.</p></body></html>"#;
        let (h3, _) = wash_html(spaced, &lat);
        assert_eq!(h3.matches("cj-flush").count(), 1, "只有章首一段顶格: {h3}");
        let (h4, _) = wash_html(&h, &lat);
        assert_eq!(h4.matches("cj-flush").count(), 3, "幂等（cj-flush div 当段落参与计数，后一段不被误顶格）: {h4}");
        assert!(h4.contains(r#"<p class="calibre_">Her father"#), "重洗后续段仍不顶格: {h4}");
    }

    #[test]
    fn detect_script() {
        let cjk = vec![e("c.xhtml", "<html><body><p>这是一本中文书籍需要两字缩进的测试内容足够多的汉字</p></body></html>")];
        assert_eq!(detect_dominant_script(&cjk), LangMode::Cjk);
        let en = vec![e("c.xhtml", "<html><body><p>This is an English book with plenty of latin letters here indeed</p></body></html>")];
        assert_eq!(detect_dominant_script(&en), LangMode::Latin);
    }

    #[test]
    fn auto_toc_from_h3_and_deep_nesting() {
        // 只用 h3 当章标题：旧 h1/h2 正则会漏，现在应生成目录
        let mut v = vec![
            e("content.opf", r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#),
            e("c1.xhtml", "<html><body><h3>章一</h3><p>a</p><h3>章二</h3></body></html>"),
        ];
        assert_eq!(wash_entries(&mut v, &WashOpts::default()).unwrap().toc_generated, 2, "h3 也进目录");
        // 多级嵌套 h1>h2>h3>h1
        let items = vec![(1u8, "A".into(), "c.xhtml".into(), "a".into()), (2, "B".into(), "c.xhtml".into(), "b".into()), (3, "C".into(), "c.xhtml".into(), "c".into()), (1, "D".into(), "c.xhtml".into(), "d".into())];
        let nav = build_nav(&items, "");
        assert!(nav.contains(r#"<li><a href="c.xhtml#a">A</a><ol><li><a href="c.xhtml#b">B</a><ol><li><a href="c.xhtml#c">C</a></li></ol></li></ol></li><li><a href="c.xhtml#d">D</a></li></ol>"#), "{nav}");
        let ncx = build_ncx(&items, "", "T", "cj-wash");
        assert!(ncx.contains(r#"<navPoint id="np1" playOrder="1"><navLabel><text>A</text></navLabel><content src="c.xhtml#a"/><navPoint id="np2""#), "{ncx}");
        assert!(ncx.contains(r#"</navPoint></navPoint></navPoint><navPoint id="np4""#), "C 收 3 层再开 D: {ncx}");
    }

    #[test]
    fn split_numbered_title_splits_section_number_not_page_number() {
        assert_eq!(split_numbered_title("第一章 1"), Some(("第一章".into(), "1".into())));
        assert_eq!(split_numbered_title("第一章　1"), Some(("第一章".into(), "1".into())), "全角空格分隔也要认");
        assert_eq!(split_numbered_title("第一章 三"), Some(("第一章".into(), "三".into())), "中文数字编号");
        assert_eq!(split_numbered_title("第一章 237"), None, "三位数以上大概率是印刷页码残留，不拆");
        assert_eq!(split_numbered_title("第一章"), None, "没有编号尾巴不拆");
        assert_eq!(split_numbered_title("1984"), None, "整体是数字不是「标题+编号」结构");
        assert_eq!(split_numbered_title("第一章 0"), None, "0 不是有效小节编号");
    }

    #[test]
    fn auto_toc_splits_numbered_titles_into_nested_entries() {
        let mut v = vec![
            e("content.opf", r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/></spine></package>"#),
            e("c1.xhtml", "<html><body><h1>第一章 1</h1><p>a</p><h1>后记</h1></body></html>"),
        ];
        wash_entries(&mut v, &WashOpts::default()).unwrap();
        let nav = s(&v, "nav.xhtml");
        assert!(
            nav.contains(r#"<li><a href="c1.xhtml#cj-toc-1">第一章</a><ol><li><a href="c1.xhtml#cj-toc-1">1</a></li></ol></li><li><a href="c1.xhtml#cj-toc-2">后记</a></li>"#),
            "「第一章 1」拆成父子两级、都指向同一锚点；「后记」没有编号尾巴不拆: {nav}"
        );
    }

    #[test]
    fn auto_toc_fallback_when_no_headings_at_all() {
        // 全书没有 h1–h6，退化到按 spine 文件生成目录（取正文首段文本当标题）
        let mut v = vec![
            e("content.opf", r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#),
            e("c1.xhtml", "<html><body><p>从前有座山，山里有座庙。</p></body></html>"),
            e("c2.xhtml", "<html><body><p>庙里有个老和尚在讲故事。</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.toc_generated, 2, "无标题也要生成兜底目录");
        let nav = s(&v, "nav.xhtml");
        assert!(nav.contains(r#"<a href="c1.xhtml">从前有座山，山里有座庙。</a>"#), "无锚点、直接指文件本身: {nav}");
        assert!(nav.contains(r#"<a href="c2.xhtml">庙里有个老和尚在讲故事。</a>"#), "{nav}");
        let ncx = s(&v, "toc.ncx");
        assert!(ncx.contains(r#"content src="c1.xhtml"/"#), "ncx 同样不带 # : {ncx}");
    }

    #[test]
    fn auto_toc_fallback_for_mostly_imageonly_pages_is_page_ranges_not_body_n() {
        // 多数页是纯图片(无可提取文本)——疑似漫画/画册，不该灌一堆"正文 N"；但所有书都要有目录（2026-09-20 用户要求），
        // 所以改为按页分段的"第 N–M 页"（3 页 → 1 段）。
        let mut v = vec![
            e("content.opf", r#"<package version="3.0"><metadata><dc:title>书</dc:title></metadata><manifest><item id="c1" href="c1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="c2.xhtml" media-type="application/xhtml+xml"/><item id="c3" href="c3.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c2"/><itemref idref="c3"/></spine></package>"#),
            e("c1.xhtml", r#"<html><body><img src="p1.jpg"/></body></html>"#),
            e("c2.xhtml", r#"<html><body><img src="p2.jpg"/></body></html>"#),
            e("c3.xhtml", "<html><body><p>唯一一页有字。</p></body></html>"),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.toc_generated, 1, "多数纯图片页 → 按页分段目录");
        let nav = s(&v, "nav.xhtml");
        assert!(nav.contains("第 1–3 页") && !nav.contains("正文 "), "{nav}");
    }

    // ---- 无效引用清理 ----

    fn dead_refs(entries: &mut [Entry]) -> usize {
        let mut rep = WashReport::default();
        drop_dead_refs(entries, &mut rep);
        rep.dead_refs_removed
    }

    #[test]
    fn dead_img_removed_but_live_external_and_alt_kept() {
        let html = concat!(
            "<html><body><p>字</p>",
            r#"<img src="../Images/gone.jpg" alt="cover"/>"#,       // 缺失、alt=cover（我们自己写的）→ 删
            r#"<img src="../Images/ok.jpg"/>"#,                       // 存在 → 留
            r#"<img src="../Images/OK2.JPG"/>"#,                      // 仅大小写不同 → 留
            r#"<img src="../Images/%E5%9B%BE.png"/>"#,                // 百分号编码的存在文件 → 留
            r#"<img src="http://x/y.jpg"/><img src="data:image/png;base64,AA=="/>"#, // 外部 → 留
            r#"<img src="../Images/lost.jpg" alt="示意图：断桥"/>"#,   // 缺失但有真替代文字 → 留
            r#"<img src='../Images/gone2.png'>"#,                     // 单引号缺失无 alt → 删
            "</body></html>"
        );
        let mut v = vec![
            e("OEBPS/Text/a.xhtml", html),
            e("OEBPS/Images/ok.jpg", "x"),
            e("OEBPS/Images/ok2.jpg", "x"),
            e("OEBPS/Images/图.png", "x"),
        ];
        assert_eq!(dead_refs(&mut v), 2);
        let out = s(&v, "OEBPS/Text/a.xhtml");
        assert!(!out.contains("gone.jpg") && !out.contains("gone2.png"), "{out}");
        for keep in ["ok.jpg", "OK2.JPG", "%E5%9B%BE.png", "http://x/y.jpg", "data:image/png", "lost.jpg"] {
            assert!(out.contains(keep), "应保留 {keep}: {out}");
        }
        assert!(out.contains("<p>字</p>"), "文字不动");
        assert_eq!(dead_refs(&mut v), 0, "幂等");
    }

    #[test]
    fn dead_font_face_removed_in_css_and_style_block() {
        let css = concat!(
            r#"@font-face{font-family:"ht";src:url("../Fonts/ht.ttf")}"#,                    // 缺失 → 删
            r#"@font-face{font-family:"live";src:url(../Fonts/live.ttf)}"#,                    // 存在 → 留
            r#"@font-face{font-family:"duokan";src:url("res:/sdcard/DuoKan/Resource/Font/a.ttf")}"#, // 设备路径 → 删
            r#"@font-face{font-family:"mix";src:local("Songti"),url(../Fonts/none.ttf) format("truetype"),url(res:///sdcard/x.ttf)}"#, // 有 local → 留 local，剔死 url
            r#"@font-face{font-family:"web";src:url(https://f.example/x.woff2)}"#,             // 外部 → 留
            "p{color:#333}"
        );
        let html = format!(r#"<html><head><style type="text/css">{css}</style></head><body><p>字</p></body></html>"#);
        let mut v = vec![
            e("OEBPS/Styles/s.css", css),
            e("OEBPS/Text/a.xhtml", &html),
            e("OEBPS/Fonts/live.ttf", "x"),
        ];
        assert_eq!(dead_refs(&mut v), 6, "css 文件 3 条（2 整条删 + mix 剔 url）+ style 块 3 条");
        for name in ["OEBPS/Styles/s.css", "OEBPS/Text/a.xhtml"] {
            let out = s(&v, name);
            assert!(!out.contains("ht.ttf") && !out.contains("DuoKan") && !out.contains("none.ttf") && !out.contains("sdcard"), "{name}: {out}");
            assert!(out.contains(r#"src:local("Songti")}"#), "mix 只剩 local，逗号/format 清干净: {name}: {out}");
            assert!(out.contains("live.ttf") && out.contains("local(") && out.contains("f.example") && out.contains("p{color:#333}"), "{name}: {out}");
        }
    }

    #[test]
    fn wash_entries_reports_dead_refs_and_never_changes_text() {
        let text = "第一章 正文文字不能变。";
        let html = format!(r#"<html><head><title>t</title></head><body><h1>第一章</h1><p>{text}</p><img src="../Images/gone.jpg" alt="cover"/></body></html>"#);
        let mut v = vec![
            e("OEBPS/content.opf", r#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>t</dc:title></metadata><manifest><item id="a" href="Text/a.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="a"/></spine></package>"#),
            e("OEBPS/Text/a.xhtml", &html),
        ];
        let rep = wash_entries(&mut v, &WashOpts::default()).unwrap();
        assert_eq!(rep.dead_refs_removed, 1);
        let out = s(&v, "OEBPS/Text/a.xhtml");
        assert!(out.contains(text) && !out.contains("gone.jpg"), "{out}");
    }

    /// `is_empty_page` 去正则重写后与原实现（正则取 body → 小写查媒体标签 → 正则去标签 → 替换实体 → trim）逐例对拍。
    #[test]
    fn is_empty_page_matches_old_regex_impl() {
        fn old(html: &str) -> bool {
            let body = Regex::new(r#"(?is)<body\b[^>]*>(.*?)</body>"#).unwrap();
            let tag = Regex::new(r#"(?s)<[^>]*>"#).unwrap();
            let inner = body.captures(html).map(|c| c[1].to_string()).unwrap_or_default();
            let low = inner.to_ascii_lowercase();
            if low.contains("<img") || low.contains("<svg") || low.contains("<image") || low.contains("<video") || low.contains("<audio") {
                return false;
            }
            let text = tag.replace_all(&inner, "");
            let text = text.replace("&nbsp;", " ").replace("&#160;", " ").replace('\u{a0}', " ");
            text.trim().is_empty()
        }
        let bodies = [
            "", " ", "\n\t", "<p></p>", "<p> &nbsp; </p>", "&#160;\u{a0}", "<div class=\"mbppagebreak\"></div>", "x", "<p>字</p>",
            "&nb<i>sp;", "&nbsp", "&amp;", "<IMG src=a>", "<p><Svg/></p>", "<image/>", "<video>", "<audio>", "a < b", "<p", "<!-- c -->",
            "<br/>\u{3000}", "&#160;x", "< >", "<<>>", "&&nbsp;",
        ];
        let wraps = [
            |b: &str| format!("<html><body>{b}</body></html>"),
            |b: &str| format!("<HTML><BODY class=\"x\">{b}</BODY></HTML>"),
            |b: &str| format!("<html><body>{b}</body><body><p>第二段</p></body></html>"),
            |b: &str| format!("<html><bodyx><body>{b}</body></html>"),
            |b: &str| format!("<html><body>{b}"),
            |b: &str| b.to_string(),
        ];
        for b in bodies {
            for w in wraps {
                let html = w(b);
                assert_eq!(is_empty_page(&html), old(&html), "{html:?}");
            }
        }
    }

    /// manifest 项/属性解析（`parse_opf`、封面声明、占位封面探测共用）：属性顺序任意、`=` 两边带空格、
    /// 缺 id 的项不算、`data-id` 这类后缀同名的属性不能串。
    #[test]
    fn manifest_items_and_tag_attr() {
        let opf = r#"<manifest><item data-id="no" href="a%20b.xhtml" id="c1" media-type="application/xhtml+xml"/>
<item id = "img" properties="cover-image" href="i.jpg" media-type="image/jpeg"></item><item href="noid.css"/><itemref idref="c1"/></manifest>"#;
        let items = manifest_items(opf);
        assert_eq!(items.len(), 2, "缺 id 的项和 itemref 都不算");
        assert_eq!((items[0].id, items[0].href, items[0].media_type, items[0].properties), ("c1", "a%20b.xhtml", "application/xhtml+xml", ""));
        assert_eq!((items[1].id, items[1].properties), ("img", "cover-image"));
        assert!(items[1].tag.starts_with("<item id = ") && items[1].tag.ends_with('>'));
        assert_eq!(tag_attr(r#"<rootfile full-path="OEBPS/x.opf" media-type="y"/>"#, "FULL-PATH"), Some("OEBPS/x.opf"));
        assert_eq!(tag_attr(r#"<a data-id="1"/>"#, "id"), None);
    }
