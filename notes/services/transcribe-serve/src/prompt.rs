//! 提示词（纯函数）。要点来自手写 OCR de-risk 的经验：只要转写、不要发挥；勾画原文当上下文能救人名/术语；
//! 行首符号要保留（样式判定靠它）；认不出的字占位而不是编。
//! 2026-09-07 真机坐实一条兜不住的坑（白皮书 §03g）：模型会把手写的汉字数字"一/二/三"转写成阿拉伯数字
//! "1/2/3"——结构认对了，字认错了；"中英文、数字、标点照原样"这句不够管用，加一条专门点名的规则。
const DEFAULT: &str = "这是 reMarkable 墨水屏上一片手写笔记的裁图（黑色手写，分辨率不高，可能有杂线）。请把手写内容逐字转写成文本：\n\
- 只输出转写出来的文字本身，不要解释、不要标题、不要加引号；\n\
- 保留原有换行；行首的符号（- 、• 、1. 、□ 等）原样保留在行首；\n\
- 中英文、数字、标点照原样，不要改写、不要补全；\n\
- 手写的汉字数字（一二三四五六七八九十等）必须原样转写成汉字，绝不能改写成阿拉伯数字（写的是「一」就输出「一」，不是「1」）；\n\
- 辨认不出的字用「？」占位；\n\
- 图里没有文字就输出空。";

/// 组提示词：`custom` 非空则替换内置正文；勾画原文（截 300 字）作为参考语境追加。
pub fn build(custom: &str, quote: Option<&str>) -> String {
    let mut p = if custom.trim().is_empty() { DEFAULT.to_string() } else { custom.trim().to_string() };
    if let Some(q) = quote.map(str::trim).filter(|q| !q.is_empty()) {
        let q: String = q.chars().take(300).collect();
        p.push_str("\n\n这片手写写在书页里被荧光笔勾出的这段原文旁边，转写时可参考其中的人名、术语，但不要把原文抄进来：「");
        p.push_str(&q);
        p.push('」');
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn quote_appended_and_custom_replaces() {
        let p = build("", Some("  梭罗在瓦尔登湖  "));
        assert!(p.starts_with("这是 reMarkable") && p.ends_with("「梭罗在瓦尔登湖」"));
        let p = build("只写字", None);
        assert_eq!(p, "只写字");
        let long: String = "字".repeat(400);
        let p = build("", Some(&long));
        let quoted = &p[p.rfind('「').unwrap()..];
        assert_eq!(quoted.chars().filter(|c| *c == '字').count(), 300, "引文截 300 字");
    }
}
