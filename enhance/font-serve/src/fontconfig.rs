//! shelf 自管的 `~/.config/fontconfig/fonts.conf` 生成（纯字符串拼装，无 IO，便于单测）。
//! 把界面/书籍的中文回退动态指向**当前已装**的中文字体（覆盖率降序）。关键：全部用 **append + binding=weak**——
//! 所以阅读器 `setFontName(你选的字体)` 永远排在最前、真正生效（修 bug1「上传不生效」），只有你选的字体缺的那个字
//! 才字形级回退到兜底中文字体（修 bug2「书里方框」，只要装了任意中文字体就不豆腐）。generic sans/serif/mono
//! 也指向它们（界面/笔记）。

/// shelf 生成的 fontconfig 首行标记（据此判断是否可安全重写）。
pub const FC_MARK: &str = "shelf font-serve 自动生成";

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

/// `cjk_keys`：中文回退字体的 fontconfig 家族名，**已按覆盖率降序**；`embolden`：对每个回退字体加 embolden
/// （墨水屏细笔画补偿）；`backup_path`：原配置备份位置（只写进注释）。
pub fn render(cjk_keys: &[&str], embolden: bool, backup_path: &std::path::Path) -> String {
    let mut x = String::new();
    x.push_str("<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"fonts.dtd\">\n");
    x.push_str(&format!("<!-- {FC_MARK}：随已装中文字体自动更新，请勿手改（改动会被覆盖）。\n     原有配置已备份到 {}。全部 weak 绑定：阅读器里选的字体优先，缺字才回退。 -->\n", backup_path.display()));
    x.push_str("<fontconfig>\n");
    if cjk_keys.is_empty() {
        x.push_str("  <!-- 当前没有已装的中文字体（覆盖率≥8%）；无回退可设。装一个中文字体即自动生效。 -->\n");
    } else {
        let names: Vec<String> = cjk_keys.iter().map(|k| esc(k)).collect();
        let prefer_block: String = names.iter().map(|n| format!("      <family>{n}</family>\n")).collect();
        for generic in ["sans-serif", "serif", "monospace"] {
            x.push_str(&format!("  <alias binding=\"weak\">\n    <family>{generic}</family>\n    <prefer>\n{prefer_block}    </prefer>\n  </alias>\n"));
        }
        // 中文文本 + 兜底：prepend weak（排在用户所选字体之后）
        // prepend 是逐条插到最前，故按覆盖率**升序**写、最高覆盖率最后 prepend → 落在最前。
        let rev: Vec<&String> = names.iter().rev().collect();
        x.push_str("  <match target=\"pattern\">\n    <test name=\"lang\" compare=\"contains\"><string>zh</string></test>\n");
        for n in &rev {
            x.push_str(&format!("    <edit name=\"family\" mode=\"prepend\" binding=\"weak\"><string>{n}</string></edit>\n"));
        }
        x.push_str("  </match>\n  <match target=\"pattern\">\n");
        for n in &rev {
            x.push_str(&format!("    <edit name=\"family\" mode=\"prepend\" binding=\"weak\"><string>{n}</string></edit>\n"));
        }
        x.push_str("  </match>\n");
        // 可选：对每个中文回退字体加 embolden（墨水屏细笔画补偿，对标旧中文化套件）。
        if embolden {
            for n in &names {
                x.push_str(&format!("  <match target=\"font\">\n    <test name=\"family\" compare=\"eq\"><string>{n}</string></test>\n    <edit name=\"embolden\" mode=\"assign\"><bool>true</bool></edit>\n  </match>\n"));
            }
        }
    }
    x.push_str("</fontconfig>\n");
    x
}

/// 抽出 fonts.conf 里所有 `<family>…</family>` 的文本（粗解析，够判断「某家族被引用」）。
pub fn referenced_families(xml: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<family>") {
        let after = &rest[i + 8..];
        if let Some(j) = after.find("</family>") {
            v.push(after[..j].trim().to_string());
            rest = &after[j..];
        } else {
            break;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn empty_chain_has_no_alias_and_says_so() {
        let x = render(&[], true, Path::new("/bak"));
        assert!(x.contains("没有已装的中文字体") && !x.contains("<alias") && !x.contains("embolden"));
        assert!(x.contains(FC_MARK) && x.contains("/bak"));
    }

    #[test]
    fn chain_order_prepend_reversed_and_escaped() {
        let x = render(&["A&B", "C<D>"], false, Path::new("/b"));
        // 家族名转义
        assert!(x.contains("A&amp;B") && x.contains("C&lt;D&gt;") && !x.contains("A&B<"));
        // generic 别名按降序（A 在 C 前）
        let alias = &x[x.find("<alias").unwrap()..x.find("<match").unwrap()];
        assert!(alias.find("A&amp;B").unwrap() < alias.find("C&lt;D&gt;").unwrap());
        // prepend 段按升序（C 先写、A 最后 prepend → 落在最前）
        let m = &x[x.find("<match").unwrap()..];
        assert!(m.find("C&lt;D&gt;").unwrap() < m.find("A&amp;B").unwrap());
        assert!(!x.contains("strong") && !x.contains("embolden"));
        // 良构：两段 pattern match，各有全部 prepend
        assert_eq!(x.matches("<match target=\"pattern\">").count(), 2);
        assert_eq!(x.matches("mode=\"prepend\" binding=\"weak\"").count(), 4);
    }

    #[test]
    fn embolden_adds_one_font_match_per_family() {
        let x = render(&["A", "B"], true, Path::new("/b"));
        assert_eq!(x.matches("<match target=\"font\">").count(), 2);
        assert_eq!(x.matches("<edit name=\"embolden\"").count(), 2);
    }

    #[test]
    fn referenced_families_extracts_all_and_tolerates_truncation() {
        let x = "<a><family> One </family><b><family>Two</family><family>broken";
        assert_eq!(referenced_families(x), vec!["One", "Two"]);
        assert!(referenced_families("").is_empty());
        // 自己生成的配置：generic 名也会被提取（既有行为：判 fontconfigRef 只看字体 key 相等，不受影响）
        let g = render(&["Han"], false, Path::new("/b"));
        assert!(referenced_families(&g).contains(&"Han".to_string()));
    }
}
