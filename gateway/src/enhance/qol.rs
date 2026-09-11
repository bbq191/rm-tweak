//! `reading-qol.json` 读写（cangjie-ime 设置页共享配置，`~/.local/share/cangjie-ime/reading-qol.json`）。
//!
//! **全量写回铁律**（旧系统增强白皮书 §08「全量防覆盖」，那份已移出仓库；现行说明见 enhance 白皮书 §02）：`langhook`（C hook）+ 原生「设置」App 的
//! QML 子页都要求写这个文件时带上全部已知键——只写自己关心的那个会把别的开关（★全局待办/单词笔记/
//! 手写识别配置…）冲掉。这里不照抄 QML 那种手写全部字段的方式：[`patch`] 把整份文件当成不透明的
//! `serde_json::Map` 读进来，只覆盖调用方明确要改的键，其余原样写回——不知道、不关心的键天然不会丢，
//! 也不怕将来别处新增字段时这边漏改。
use rmsvc_core::paths::Paths;
use serde_json::{Map, Value};
use std::path::PathBuf;

fn path(paths: &Paths) -> PathBuf {
    paths.home().join(".local/share/cangjie-ime/reading-qol.json")
}

fn load(paths: &Paths) -> Map<String, Value> {
    std::fs::read_to_string(path(paths))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// 只 patch 传入的键，其余原样透传写回。
pub fn patch(paths: &Paths, changes: Map<String, Value>) -> Result<(), String> {
    // 进程内串行化读-改-写：网页连点几个开关会并发进来多个 PUT，各自 load→改→写会互相覆盖对方刚改的键。
    // （跨进程——QML/C hook 也写这个文件——靠"全量写回"约定，锁不到，见头注。）
    static PATCH_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = rmsvc_core::sync::lock(&PATCH_LOCK);
    let mut map = load(paths);
    map.extend(changes);
    let p = path(paths);
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec_pretty(&Value::Object(map)).map_err(|e| e.to_string())?;
    rmsvc_core::fs::write_atomic(&p, &bytes).map_err(|e| e.to_string())
}

/// `reading-qol.json` 的一次快照：`/api/enhance/status` 一次请求要读六七个开关，读一次文件、各开关从同一份
/// 快照取（此前每个开关各自读一遍整份文件）。
pub struct Qol(Map<String, Value>);

impl Qol {
    pub fn load(paths: &Paths) -> Qol {
        Qol(load(paths))
    }

    fn flag(&self, key: &str, default: bool) -> bool {
        self.0.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    /// CJK 荧光笔精确吸附开关（`hlSnapCjk`，langhook C hook 消费）。缺省视为开——跟 QML 侧 `c.hlSnapCjk !== false`
    /// 同一条缺省规则（`xovi-extensions/reading-qol/settings-reading-enhance.qmd`）。
    pub fn hl_snap_cjk(&self) -> bool {
        self.flag("hlSnapCjk", true)
    }

    /// CJK 手写笔迹优化——纯网页层派生开关，不是 `reading-qol.json` 里单独存在的字段：
    /// `hwStrokeNibMinRatio < 1.0` 视为已开（`enhance/handwriting-stroke/src/hw_stroke.c` 里 `1.0`
    /// 是两个效果〔笔尖角度模型+提按速度代理〕全部关闭的 fail-safe 默认值，真机验证过 `0.6` 是效果
    /// 不错的强度）。只读 `hwStrokeNibMinRatio` 一个键就够判断开关态——网页层写入时两个 min_ratio
    /// 字段永远同步写（见 [`super::set_qol`]），不会出现只改了一个的情况。
    pub fn hw_stroke_enabled(&self) -> bool {
        self.0.get("hwStrokeNibMinRatio").and_then(Value::as_f64).map(|v| v < 1.0).unwrap_or(false)
    }

    /// 「导入 md 文档」开关（`notesImportMdEnabled`），控制笔记 tab「导入」子标签是否显示。跟
    /// `hl_snap_cjk` 缺省开不同，这个缺省关——新功能第一次上线，不想让用户点开笔记 tab 就撞见一个
    /// 半成品，得手动去「管理→实验室」打开才看得到。
    pub fn notes_import_md_enabled(&self) -> bool {
        self.flag("notesImportMdEnabled", false)
    }

    /// 「漫画页边距最小化」开关（`comicMinMargin`，2026-09-21）：**仅对漫画 EPUB**（以图为主，允许有文字页），控制 book-serve 优化时漫画页补白到哪种页框、
    /// 「加入 xochitl」后是否登记"首次打开时把阅读器页边距设为 1"（xochitl 里的 qmd 代理执行）。book-serve 只读这个键
    /// （`comic_margins.rs::enabled`），跟「导入 md」一样缺省关——新功能第一次上线，得手动去「管理→实验室」打开。
    pub fn comic_min_margin(&self) -> bool {
        self.flag("comicMinMargin", false)
    }

    /// 「单击翻页」（`tapPageTurn`）与「日漫翻页规则」（`rtlPageTurn`）两个开关（2026-09-24）：xochitl 阅读器里的
    /// `reader-page-turn.qmd` 每次打开书时读这两个键（不轮询），切换后下次打开书生效。缺省都关 = xochitl 原生行为。
    pub fn tap_page_turn(&self) -> bool {
        self.flag("tapPageTurn", false)
    }
    pub fn rtl_page_turn(&self) -> bool {
        self.flag("rtlPageTurn", false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn tmp_paths() -> (tempfile::TempDir, Paths) {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let mut env = HashMap::new();
        env.insert("HOME".to_string(), h);
        let paths = Paths::resolve(move |k| env.get(k).cloned());
        (t, paths)
    }

    #[test]
    fn hl_snap_cjk_defaults_true_when_missing() {
        let (_t, paths) = tmp_paths();
        assert!(Qol::load(&paths).hl_snap_cjk(), "文件不存在时缺省视为开，跟 QML 侧一致");
    }

    #[test]
    fn patch_preserves_unknown_keys() {
        let (_t, paths) = tmp_paths();
        // 模拟 QML 已经写过一份、带上跟这个面板无关的开关
        let mut seed = Map::new();
        seed.insert("starTodoEnabled".into(), Value::Bool(true));
        seed.insert("cardhwEnabled".into(), Value::Bool(true));
        seed.insert("hlSnapCjk".into(), Value::Bool(true));
        patch(&paths, seed).unwrap();

        let mut change = Map::new();
        change.insert("hlSnapCjk".into(), Value::Bool(false));
        patch(&paths, change).unwrap();

        assert!(!Qol::load(&paths).hl_snap_cjk(), "patch 的键要生效");
        let full = load(&paths);
        assert_eq!(full["starTodoEnabled"], Value::Bool(true), "没碰过的键不能被冲掉");
        assert_eq!(full["cardhwEnabled"], Value::Bool(true), "没碰过的键不能被冲掉");
    }

    #[test]
    fn hw_stroke_enabled_defaults_false_when_missing() {
        let (_t, paths) = tmp_paths();
        assert!(!Qol::load(&paths).hw_stroke_enabled(), "文件不存在/字段缺失时缺省视为关（C 侧同一条 fail-safe 规则）");
    }

    #[test]
    fn hw_stroke_enabled_true_when_ratio_below_one() {
        let (_t, paths) = tmp_paths();
        let mut seed = Map::new();
        seed.insert("hwStrokeNibMinRatio".into(), serde_json::json!(0.6));
        patch(&paths, seed).unwrap();
        assert!(Qol::load(&paths).hw_stroke_enabled(), "ratio < 1.0 视为已开");
    }

    #[test]
    fn notes_import_md_enabled_defaults_false() {
        let (_t, paths) = tmp_paths();
        assert!(!Qol::load(&paths).notes_import_md_enabled(), "新功能第一次上线，缺省关，不是缺省开");
    }

    #[test]
    fn comic_min_margin_defaults_false_and_follows_patch() {
        let (_t, paths) = tmp_paths();
        assert!(!Qol::load(&paths).comic_min_margin(), "缺省关");
        let mut on = Map::new();
        on.insert("comicMinMargin".into(), Value::Bool(true));
        patch(&paths, on).unwrap();
        assert!(Qol::load(&paths).comic_min_margin());
    }
}
