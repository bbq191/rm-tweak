//! 单页 UI（手机/电脑浏览器打开 `https://<设备IP>/`，2026-09-10 起绑标准 443 端口，不用带端口号）。固定 tab「传书」（母版库总入口）+「管理」，中间的服务 tab
//! 按 `/api/services` 注册表动态生成（xochitl 字体 = font-serve、KOReader = koreader-serve、壁纸 = wallpaper-serve）。
//! 上传逐文件一请求（每本独立成败、独立进度条），所有上传口共用一个 `uploader` + 服务端同形回执（`asset::receipt`）；
//! 格式白名单由 [`page`] 从 `rmsvc_core::formats` 注入（`__EXTS__`），网页 accept / 选中即拦与服务端上传门同源。
//! 页面源码在 `services/gateway/ui/`（index.html 骨架 + style.css + app.js + auth.css），编译期 `include_str!` 进二进制：
//! 网页仍是单文件零外链，但 JS/CSS 是真文件——编辑器/`node --check`（CI）直接检查，改样式不用在 Rust 原始字符串里找。
use std::sync::OnceLock;

const INDEX_HTML: &str = include_str!("../ui/index.html");
const STYLE_CSS: &str = include_str!("../ui/style.css");
const APP_JS: &str = include_str!("../ui/app.js");
const AUTH_CSS: &str = include_str!("../ui/auth.css");

/// i18n 语言包（2026-09-09 起，只覆盖主界面外壳 + 顶层导航，登录/改密码页与各模块正文暂不迁移——
/// 见 `ui/locales/` 目录说明与白皮书对应记录）。继续走 `include_str!` 编译进二进制，不破坏"单文件
/// 零外链"部署（不用改 build/deploy/install 脚本，语言包新增/改词只是改这两个 JSON 再重新编译）。
const LOCALE_ZH_CN: &str = include_str!("../ui/locales/zh-CN.json");
const LOCALE_EN_US: &str = include_str!("../ui/locales/en-US.json");

/// 按语言码取语言包 JSON；不认识的语言码一律落中文（不是空白页面）。
pub fn locale_json(lang: &str) -> &'static str {
    match lang {
        "en-US" | "en" => LOCALE_EN_US,
        _ => LOCALE_ZH_CN,
    }
}

/// 渲染主页：骨架 + 样式 + 脚本拼成单文件，再把格式白名单注入（进程内只算一次）。
pub fn page() -> &'static str {
    static PAGE: OnceLock<String> = OnceLock::new();
    PAGE.get_or_init(|| {
        use rmsvc_core::formats::{BOOK_EXTS, DICT_EXTS, FONT_EXTS, IMAGE_EXTS, NATIVE_EXTS};
        // "convertible" 档（azw3/mobi/azw/prc/fb2/txt）2026-09-17 随 EPUB 线架构调整退役；"仅
        // KOReader" 档（cbz/cbr/djvu/html/htm/rtf/doc/docx/chm/xps）2026-09-18 用户明确要求一并
        // 退役——母版库只收 EPUB/PDF，`BOOK_EXTS == NATIVE_EXTS`，不再需要 `koOnly` 字段区分两档。
        let exts = serde_json::json!({"book": BOOK_EXTS, "native": NATIVE_EXTS, "font": FONT_EXTS, "dict": DICT_EXTS, "image": IMAGE_EXTS});
        INDEX_HTML.replace("__STYLE__", STYLE_CSS).replace("__SCRIPT__", APP_JS).replace("__EXTS__", &exts.to_string())
    })
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// 登录页：只要密码，无用户名。`error` 空=无提示。
pub fn login_page(error: &str, next: &str) -> String {
    format!(r#"<!doctype html><html lang="zh"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>秘密花园 · 登录</title><style>{AUTH_CSS}</style></head><body>
<form method="post" action="/login" autocomplete="on"><h1>秘密花园</h1>
<label for="pw">密码</label><input id="pw" name="password" type="password" autofocus required autocomplete="current-password">
<input type="hidden" name="next" value="{next}"><div class="err">{err}</div><button type="submit">登录</button>
<p class="small">首次使用密码为 <code>shelf</code>，登录后必须改。<br>浏览器提示"不安全"是自签证书所致：<a href="/ca.crt">下载 CA 证书</a> 装进手机/电脑信任库一次即不再提示。</p></form></body></html>"#, next = esc(next), err = esc(error))
}

/// 改密码页：`forced`=首登必改（不给"返回"）。
pub fn password_page(error: &str, forced: bool) -> String {
    // 2026-09-09 审计修：密码最小长度原来在这里硬编码了两处 `minlength="6"` + 提示文案里的"6"，
    // 跟 `config::MIN_PASSWORD_LEN`（服务端真正校验用的那个）各写各的——真改了那个常量，这里
    // 三处不会跟着变，会出现"服务端要求 N 位，网页却只拦到 6 位就放行提交"的体验错配（不是安全
    // 漏洞，服务端仍是最终裁决者，纯粹 UX 一致性问题）。改成从常量插值，单一事实源。
    let n = crate::config::MIN_PASSWORD_LEN;
    let hint = if forced { format!("首次登录：请先设置新密码（至少 {n} 位，不能是默认密码）。") } else { format!("至少 {n} 位。改完其它已登录设备需重新登录。") };
    let back = if forced { "" } else { r#"<p class="small"><a href="/">返回秘密花园</a></p>"# };
    format!(r#"<!doctype html><html lang="zh"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>秘密花园 · 改密码</title><style>{AUTH_CSS}</style></head><body>
<form method="post" action="/password"><h1>设置密码</h1><p class="small" style="margin-top:0">{hint}</p>
<label for="cur">当前密码</label><input id="cur" name="current" type="password" required autocomplete="current-password">
<label for="new">新密码</label><input id="new" name="new" type="password" required minlength="{n}" autocomplete="new-password">
<label for="cf">再输一次</label><input id="cf" name="confirm" type="password" required minlength="{n}" autocomplete="new-password">
<div class="err">{err}</div><button type="submit">保存</button>{back}</form></body></html>"#, err = esc(error))
}

#[cfg(test)]
mod tests {
    #[test]
    fn page_injects_format_whitelists_once() {
        let p = super::page();
        assert!(!p.contains("__EXTS__"), "占位应被替换");
        assert!(p.contains(r#""book":["epub","pdf"]"#) && p.contains(r#""font":["ttf""#) && p.contains(r#""dict":["ifo""#) && p.contains(r#""image":["jpg""#));
        assert!(p.contains(r#""native":["epub","pdf"]"#), "格式说明注入");
        assert!(!p.contains(r#""convertible""#), "convertible 档已随 EPUB 线架构调整退役");
        assert!(!p.contains(r#""koOnly""#), "仅 KOReader 档已随 2026-09-18 格式收窄退役");
        assert!(std::ptr::eq(p, super::page()), "OnceLock 只渲染一次");
        assert!(!p.contains("__STYLE__") && !p.contains("__SCRIPT__") && p.contains("<style>") && p.contains("</script></body></html>"), "骨架三段拼接完整");
        assert!(super::APP_JS.contains("__EXTS__") && !super::APP_JS.contains("__STYLE__"), "白名单占位在 app.js");
    }
    #[test]
    fn locale_files_have_identical_key_sets() {
        // 语言包 key 漂移是这套 i18n 最容易悄悄坏掉的地方：某个语言加了新 key、另一个忘了加，
        // 前端查表查不到会静默显示 undefined（比显示错误语言更容易被漏看）——用一条离线测试钉住
        // "两份文件 key 集合完全一致"，比指望人工记得同步两份 JSON 可靠。
        let zh: serde_json::Value = serde_json::from_str(super::LOCALE_ZH_CN).unwrap();
        let en: serde_json::Value = serde_json::from_str(super::LOCALE_EN_US).unwrap();
        let keys = |v: &serde_json::Value| -> std::collections::BTreeSet<String> { v.as_object().unwrap().keys().cloned().collect() };
        assert_eq!(keys(&zh), keys(&en), "两份语言包的 key 集合必须完全一致");
        assert!(!keys(&zh).is_empty());
    }

    #[test]
    fn locale_json_falls_back_to_chinese_for_unknown_lang() {
        assert_eq!(super::locale_json("fr-FR"), super::LOCALE_ZH_CN);
        assert_eq!(super::locale_json("en-US"), super::LOCALE_EN_US);
        assert_eq!(super::locale_json("zh-CN"), super::LOCALE_ZH_CN);
    }

    #[test]
    fn password_page_minlength_matches_config_constant() {
        // 2026-09-09 审计修：改密码页的 minlength/提示文案该跟 config::MIN_PASSWORD_LEN 联动，
        // 不是各写各的硬编码——常量改了，这里必须跟着变，否则回归会漏掉这条一致性。
        let n = crate::config::MIN_PASSWORD_LEN;
        let p = super::password_page("", false);
        assert!(p.contains(&format!("minlength=\"{n}\"")), "页面里的 minlength 应该等于当前常量: {p}");
        assert!(p.contains(&format!("至少 {n} 位")), "提示文案也该带上当前常量: {p}");
    }
}
