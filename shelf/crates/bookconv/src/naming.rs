//! EPUB 书名规范化：`书名 - 卷/部/上/下`（用户 2026-09-20 拍板，文字书和漫画一律如此）。
//!
//! 下载站文件名常带一长串元数据（Anna's Archive：`书名 -- 作者 -- 丛书, 年份 -- 出版社 -- <hash> --
//! Anna's Archive`），母版库里既难读、手机上还折成好几行。规则：
//! 1. 取第一个 ` -- ` 之前的段（没有就取整名）；去掉尾部 `[完]`/`（完结）` 这类完结标记。
//! 2. 末尾认卷标记就把它整理成 `书名 - 标记`；**没有卷标记的原样保留**（`疯探-空城` 的"空城"是
//!    副标题不是卷，不该被改成别的样子）。
//! 3. 标记写法统一成**数字在前**（用户 2026-09-20 拍板）：`卷02`→`02卷`、`第二卷`→`二卷`、`第2部`→`2部`、
//!    `Vol.3`→`3卷`；`上`/`中`/`下`（及 `上册` 等）原样保留。只整理结构，不改数字本身（位数、汉字数字、
//!    繁简都不动）。`第N话/回/季/篇` 不是卷标记，不处理。
//!
//! 幂等：规范化过的名字再规范化不变。

use regex::Regex;
use std::sync::OnceLock;

const NUM: &str = r"[0-9０-９一二三四五六七八九十百零〇两]+";

fn marker_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // 标记：第N卷/部/册/集/话/回/季/篇 | 卷N | N卷 | 上/中/下(+册/部/卷/篇) | Vol.N
        let marker = format!(
            r"(?:第\s*{NUM}\s*[卷部册集话回季篇]|[卷部册集]\s*{NUM}|{NUM}\s*[卷部册集]|[上中下](?:[册部卷篇])?|[Vv][Oo][Ll]\.?\s*[0-9]+)"
        );
        // 三种连接：括号包裹 | 分隔符/空白 | 无分隔紧贴（仅限带"卷/部/册"字样的标记，避免误伤"世界上"）
        Regex::new(&format!(
            r"^(?P<t>.+?)\s*(?:[（(]\s*(?P<m1>{marker})\s*[)）]|(?:[-–—_·:：]\s*|\s+)(?P<m2>{marker})|(?P<m3>第\s*{NUM}\s*[卷部册集]|[卷部册]\s*{NUM}|[Vv][Oo][Ll]\.?\s*[0-9]+))\s*$"
        ))
        .unwrap()
    })
}

fn tail_tag_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"\s*[\[（(【]\s*(?:完结版|完结|完|全|终)\s*[\]）)】]\s*$").unwrap())
}

/// 把卷标记写成统一的数字在前形式：`卷N`/`第N卷`/`VolN`→`N卷`，`部N`/`第N部` 等→`N部`…；`N卷` 已是目标形式；
/// 其余（上/中/下、第N话…）去内部空白后原样。
fn normalize_marker(m: &str) -> String {
    let m = m.trim();
    let flat: String = m.split_whitespace().collect();
    static RES: OnceLock<[Regex; 3]> = OnceLock::new();
    let [unit_first, di_first, vol] = RES.get_or_init(|| {
        [
            Regex::new(&format!(r"^(?P<u>[卷部册集])(?P<n>{NUM})$")).unwrap(),
            Regex::new(&format!(r"^第(?P<n>{NUM})(?P<u>[卷部册集])$")).unwrap(),
            Regex::new(r"^[Vv][Oo][Ll]\.?(?P<n>[0-9]+)$").unwrap(),
        ]
    });
    if let Some(c) = unit_first.captures(&flat).or_else(|| di_first.captures(&flat)) {
        return format!("{}{}", &c["n"], &c["u"]);
    }
    if let Some(c) = vol.captures(&flat) {
        return format!("{}卷", &c["n"]);
    }
    flat
}

/// 规范化书名（不含扩展名）。无法识别时原样返回（去首尾空白）。
pub fn canonical_book_name(stem: &str) -> String {
    let stem = stem.trim();
    let first = stem.split(" -- ").next().unwrap_or(stem).trim();
    if first.is_empty() {
        return stem.to_string();
    }
    let first = tail_tag_re().replace(first, "");
    let first = first.trim();
    if let Some(c) = marker_re().captures(first) {
        let m = c.name("m1").or(c.name("m2")).or(c.name("m3")).map(|m| m.as_str()).unwrap_or("");
        let title = c["t"].trim().trim_end_matches(['-', '–', '—', '_', '·', ':', '：']).trim();
        if !title.is_empty() && !m.is_empty() {
            return format!("{title} - {}", normalize_marker(m));
        }
    }
    first.to_string()
}

/// 这个名字（不含扩展名）里是否有可识别的卷标记。优化时只在有卷标记的书上把 EPUB 自己的 `dc:title` 改成
/// 规范名（避免把 `abc123.epub` 这种无意义文件名覆盖掉书里本来正确的书名）。
pub fn has_volume_marker(stem: &str) -> bool {
    let first = stem.trim().split(" -- ").next().unwrap_or("").trim();
    let first = tail_tag_re().replace(first, "");
    marker_re().is_match(first.trim())
}

/// 带扩展名的文件名版本：`x -- y.epub` → `x.epub`。扩展名原样保留。
pub fn canonical_file_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !ext.is_empty() && ext.len() <= 5 => format!("{}.{ext}", canonical_book_name(stem)),
        _ => canonical_book_name(name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_library_names() {
        // 全部取自用户母版库的真实文件名（Anna's Archive 风格）。
        let cases = [
            ("鏢人 - 卷02 -- 許先哲 -- 鏢人 - 卷02, 2022 -- Mox_moe -- df4a0842d2e764f650f494dee3153596 -- Anna’s Archive", "鏢人 - 02卷"),
            ("亂馬1⁄2 典藏版 - 19卷 -- 高橋留美子 -- 19, 2019 -- 尖端 -- 03220cf1a8939e8ef2b2253cddd33408 -- Anna's Archive", "亂馬1⁄2 典藏版 - 19卷"),
            ("亂馬1⁄2 典藏版 - 20卷 [完] -- 高橋留美子 -- 20, 2024 -- 尖端 -- c998de4f -- Anna’s Archive", "亂馬1⁄2 典藏版 - 20卷"),
            ("火影忍者 - 08卷 -- 岸本斉史 -- 8 -- 集英社 -- abc -- Anna’s Archive", "火影忍者 - 08卷"),
            ("镖人(卷二) -- 许先哲", "镖人 - 二卷"),
            ("疯探-空城", "疯探-空城"),
        ];
        for (input, want) in cases {
            assert_eq!(canonical_book_name(input), want, "输入: {input}");
        }
    }

    #[test]
    fn marker_forms() {
        let cases = [
            ("雪人 上册", "雪人 - 上册"),
            ("雪人 - 下", "雪人 - 下"),
            ("三体 - 第二部", "三体 - 二部"),
            ("三体 第2部", "三体 - 2部"),
            ("某书Vol.3", "某书 - 3卷"),
            ("某书 Vol 12", "某书 - 12卷"),
            ("某书 - 3册", "某书 - 3册"),
            ("某书 - 册3", "某书 - 3册"),
            ("某书 - 卷02", "某书 - 02卷"),
            ("某书卷五", "某书 - 五卷"),
            ("某书 第十二卷", "某书 - 十二卷"),
            ("某书（完结）", "某书"),
        ];
        for (input, want) in cases {
            assert_eq!(canonical_book_name(input), want, "输入: {input}");
        }
    }

    #[test]
    fn no_marker_stays_as_is_and_no_false_positive() {
        for s in ["世界上", "文章标题", "红楼梦", "Some English Title", "  带空格  "] {
            assert_eq!(canonical_book_name(s), s.trim(), "不该被改: {s}");
        }
    }

    #[test]
    fn idempotent() {
        for s in ["鏢人 - 02卷", "亂馬1⁄2 典藏版 - 19卷", "雪人 - 上册", "三体 - 二部", "疯探-空城"] {
            assert_eq!(canonical_book_name(&canonical_book_name(s)), canonical_book_name(s));
            assert_eq!(canonical_book_name(s), s);
        }
    }

    #[test]
    fn file_name_keeps_extension_and_handles_dots() {
        assert_eq!(canonical_file_name("鏢人 - 卷02 -- 許先哲 -- Anna’s Archive.epub"), "鏢人 - 02卷.epub");
        assert_eq!(canonical_file_name("plain.epub"), "plain.epub");
        assert_eq!(canonical_file_name("no_ext"), "no_ext");
        // 书名里带点（英文缩写）不是扩展名分隔时也不崩：末段超过 5 字符视为书名的一部分。
        assert_eq!(canonical_file_name("Dr. Who Long Title"), "Dr. Who Long Title");
    }

    #[test]
    fn has_volume_marker_only_for_real_markers() {
        assert!(has_volume_marker("鏢人 - 卷02 -- 許先哲"));
        assert!(has_volume_marker("雪人 - 上册"));
        assert!(!has_volume_marker("abc123"));
        assert!(!has_volume_marker("疯探-空城"));
    }

    #[test]
    fn degenerate_input_never_returns_empty() {
        assert_eq!(canonical_book_name(" -- 作者"), "-- 作者");
        assert_eq!(canonical_book_name(""), "");
    }
}
