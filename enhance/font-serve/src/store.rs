//! 字体仓库（`AssetStore` 实现）= `$XDG_DATA_HOME/fonts/`（fontconfig 用户字体目录）的管理器。
//! 目录里**所有**字体一视同仁、按 fontconfig 家族名归组（一个家族多字重文件=一项），都可删——没有"内建/系统"之分
//! （2026-09-03 用户纠正：那是对旧中文化套件 scp 字体的路径耦合）。唯一的提示：家族被
//! `~/.config/fontconfig/fonts.conf` 引用（界面 CJK 回退）的标 `fontconfigRef`，删前 UI 提醒但不拦。
//! 上传→落目录→`fc-cache -f`→重建 `$XDG_DATA_HOME/shelf/fonts.json`（字体菜单 qmd 读）。**只管原生阅读器**：
//! KOReader 的字体由 koreader-serve 单独管（用户 2026-09-03 定：两边各自装、不同时装填）。
use serde::{Deserialize, Serialize};
use rmsvc_core::asset::{AssetItem, AssetStore};
use rmsvc_core::formats::{self, FONT_EXTS};
use rmsvc_core::paths::Paths;
use rmsvc_core::ttf;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 覆盖率 ≥ 此值才算"中文字体"、才进回退链（滤掉纯拉丁字体，避免拉丁字体当中文兜底）。
pub const CJK_MIN_PCT: u8 = 8;
/// 覆盖率 < 此值 = 低覆盖美术/子集字体，上传时警告（正文会缺字）。
pub const CJK_LOW_PCT: u8 = 80;
/// shelf 生成的 fontconfig 首行标记（据此判断是否可安全重写）。
const FC_MARK: &str = "shelf font-serve 自动生成";

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Names {
    pub cn: String,
    pub tw: String,
    pub en: String,
}

/// fonts.json 里的一项 = 一个字体家族。
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct FontEntry {
    /// fontconfig 首家族名（`epub.setFontName(key)` 用）。
    pub key: String,
    /// 该家族的全部文件（多字重）。
    pub files: Vec<String>,
    pub names: Names,
    /// 中文基本区覆盖率（0..=100）。0=非中文字体。用于回退排序 + 低覆盖警告。
    #[serde(default)]
    pub cjk_pct: u8,
    /// 被 fontconfig 用户配置引用（界面中文回退用），删前提醒。
    #[serde(default)]
    pub fontconfig_ref: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct FontConfig {
    /// 对中文回退字体加 `embolden`（墨水屏细笔画/低对比补偿，对标旧中文化套件）。
    /// **默认开**（2026-09-04 用户目测更清楚）；显式写 false 则关。
    pub embolden_cjk_fallback: bool,
}

impl Default for FontConfig {
    fn default() -> Self {
        FontConfig { embolden_cjk_fallback: true }
    }
}

impl FontConfig {
    pub fn load(paths: &Paths) -> FontConfig {
        rmsvc_core::config::load_or_seed(&paths.service_config("font"))
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct FontsJson {
    version: u32,
    fonts: Vec<FontEntry>,
}

pub struct FontStore {
    /// 中文回退加粗（墨水屏补偿）。运行时可切（PUT /config），用 AtomicBool 免锁。
    embolden: std::sync::atomic::AtomicBool,
    config_path: PathBuf,
    fonts_dir: PathBuf,
    json_path: PathBuf,
    fontconfig_conf: PathBuf,
    /// 测试可关：不真跑 fc-cache/fc-scan。
    pub side_effects: bool,
}

/// 一个文件的家族信息：(首家族名 = key, 全部家族名列表)。
fn split_families(s: &str) -> Vec<String> {
    s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect()
}

impl FontStore {
    pub fn new(paths: &Paths, cfg: FontConfig) -> FontStore {
        FontStore {
            embolden: std::sync::atomic::AtomicBool::new(cfg.embolden_cjk_fallback),
            config_path: paths.service_config("font"),
            fonts_dir: paths.user_fonts_dir(),
            json_path: paths.data_dir().join("fonts.json"),
            fontconfig_conf: paths.config_root().join("fontconfig/fonts.conf"),
            side_effects: true,
        }
    }

    pub fn embolden(&self) -> bool {
        self.embolden.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 切换中文回退加粗：存配置 + 重写 fontconfig（fontconfig 实时生效，翻书即见，无需重启 xochitl）。
    pub fn set_embolden(&self, on: bool) -> Result<(), String> {
        self.embolden.store(on, std::sync::atomic::Ordering::Relaxed);
        rmsvc_core::config::save(&self.config_path, &FontConfig { embolden_cjk_fallback: on }, None)?;
        self.write_fontconfig(&self.entries())?;
        Ok(())
    }
    pub fn fonts_dir(&self) -> &Path {
        &self.fonts_dir
    }
    pub fn json_path(&self) -> &Path {
        &self.json_path
    }

    /// 文件的家族名列表：fc-scan 优先（带本地化名，如 "LXGW WenKai,霞鹜文楷"），否则自解析 name 表。
    fn families_of(&self, path: &Path) -> Vec<String> {
        if self.side_effects {
            if let Ok(o) = std::process::Command::new("fc-scan").args(["--format", "%{family}"]).arg(path).output() {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if o.status.success() && !s.is_empty() {
                    return split_families(&s);
                }
            }
        }
        std::fs::read(path).ok().and_then(|b| ttf::family_name(&b)).map(|f| vec![f]).unwrap_or_default()
    }

    fn fontconfig_families(&self) -> Vec<String> {
        let Ok(t) = std::fs::read_to_string(&self.fontconfig_conf) else { return vec![] };
        let mut v = Vec::new();
        let mut rest = t.as_str();
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

    /// 扫目录 → 按首家族名归组 → 条目（key 排序）。
    pub fn scan(&self) -> Vec<FontEntry> {
        let refs = self.fontconfig_families();
        let mut groups: BTreeMap<String, FontEntry> = BTreeMap::new();
        let Ok(rd) = std::fs::read_dir(&self.fonts_dir) else { return vec![] };
        let mut files: Vec<String> = rd.flatten().filter_map(|e| e.file_name().to_str().map(|s| s.to_string())).filter(|n| !n.starts_with('.') && formats::has_ext(n, FONT_EXTS)).collect();
        files.sort();
        for f in files {
            let path = self.fonts_dir.join(&f);
            let fams = self.families_of(&path);
            let pct = std::fs::read(&path).ok().and_then(|b| ttf::han_coverage_pct(&b)).unwrap_or(0);
            let key = match fams.first() {
                Some(k) => k.clone(),
                None => f.rsplit_once('.').map(|(s, _)| s.to_string()).unwrap_or_else(|| f.clone()),
            };
            // 本地化显示名：取第一个含非 ASCII 的家族名，没有就用 key
            let local = fams.iter().find(|s| !s.is_ascii()).cloned().unwrap_or_else(|| key.clone());
            let e = groups.entry(key.clone()).or_insert_with(|| FontEntry {
                key: key.clone(),
                files: vec![],
                names: Names { cn: local.clone(), tw: local.clone(), en: key.clone() },
                cjk_pct: 0,
                fontconfig_ref: refs.iter().any(|r| r == &key || fams.contains(r)),
            });
            e.cjk_pct = e.cjk_pct.max(pct);
            e.files.push(f);
        }
        groups.into_values().collect()
    }

    /// 重建 fonts.json（qmd 消费）。
    pub fn write_index(&self) -> Result<Vec<FontEntry>, String> {
        let fonts = self.scan();
        rmsvc_core::config::save(&self.json_path, &FontsJson { version: 1, fonts: fonts.clone() }, None)?;
        if let Err(e) = self.write_fontconfig(&fonts) {
            eprintln!("[font-serve] 写 fontconfig 回退失败: {e}");
        }
        Ok(fonts)
    }

    /// 读缓存的 fonts.json（列表接口用，避免每次 fc-scan）。缺则重建。
    pub fn entries(&self) -> Vec<FontEntry> {
        std::fs::read_to_string(&self.json_path).ok().and_then(|t| serde_json::from_str::<FontsJson>(&t).ok()).map(|j| j.fonts).unwrap_or_else(|| self.write_index().unwrap_or_default())
    }

    fn fc_cache(&self) {
        if self.side_effects {
            let _ = std::process::Command::new("fc-cache").arg("-f").status();
        }
    }

    /// 中文字体（覆盖率 ≥ CJK_MIN_PCT），按覆盖率降序——回退链首选覆盖最全的。
    fn cjk_fallback_order(fonts: &[FontEntry]) -> Vec<&FontEntry> {
        let mut v: Vec<&FontEntry> = fonts.iter().filter(|e| e.cjk_pct >= CJK_MIN_PCT).collect();
        v.sort_by(|a, b| b.cjk_pct.cmp(&a.cjk_pct).then(a.key.cmp(&b.key)));
        v
    }

    /// 生成 shelf 自管的 `~/.config/fontconfig/fonts.conf`：把界面/书籍的中文回退动态指向**当前已装**的
    /// 中文字体（覆盖率降序）。关键：全部用 **append + binding=weak**——所以阅读器 `setFontName(你选的字体)`
    /// 永远排在最前、真正生效（修 bug1「上传不生效」），只有你选的字体缺的那个字才字形级回退到兜底中文字体
    /// （修 bug2「书里方框」，只要装了任意中文字体就不豆腐）。generic sans/serif/mono 也指向它们（界面/笔记）。
    /// 首次接管前，把已存在的非 shelf 配置备份到 `~/.config/shelf/fontconfig-fonts.conf.pre-shelf.bak`。
    pub fn write_fontconfig(&self, fonts: &[FontEntry]) -> Result<(), String> {
        let cjk = Self::cjk_fallback_order(fonts);
        let dir = self.fontconfig_conf.parent().ok_or("fontconfig 路径无父目录")?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        // 备份既有的非 shelf 配置（只备份一次）
        if let Ok(existing) = std::fs::read_to_string(&self.fontconfig_conf) {
            if !existing.contains(FC_MARK) {
                let bak = self.config_root_backup();
                if !bak.exists() {
                    if let Some(p) = bak.parent() {
                        let _ = std::fs::create_dir_all(p);
                    }
                    let _ = std::fs::write(&bak, &existing);
                }
            }
        }
        let esc = |s: &str| s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
        let mut x = String::new();
        x.push_str("<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"fonts.dtd\">\n");
        x.push_str(&format!("<!-- {FC_MARK}：随已装中文字体自动更新，请勿手改（改动会被覆盖）。\n     原有配置已备份到 {}。全部 weak 绑定：阅读器里选的字体优先，缺字才回退。 -->\n", self.config_root_backup().display()));
        x.push_str("<fontconfig>\n");
        if cjk.is_empty() {
            x.push_str("  <!-- 当前没有已装的中文字体（覆盖率≥8%）；无回退可设。装一个中文字体即自动生效。 -->\n");
        } else {
            let names: Vec<String> = cjk.iter().map(|e| esc(&e.key)).collect();
            let prefer_block: String = names.iter().map(|n| format!("      <family>{n}</family>\n")).collect();
            for generic in ["sans-serif", "serif", "monospace"] {
                x.push_str(&format!("  <alias binding=\"weak\">\n    <family>{generic}</family>\n    <prefer>\n{prefer_block}    </prefer>\n  </alias>\n"));
            }
            // 中文文本 + 兜底：append weak（排在用户所选字体之后）
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
            if self.embolden() {
                for n in &names {
                    x.push_str(&format!("  <match target=\"font\">\n    <test name=\"family\" compare=\"eq\"><string>{n}</string></test>\n    <edit name=\"embolden\" mode=\"assign\"><bool>true</bool></edit>\n  </match>\n"));
                }
            }
        }
        x.push_str("</fontconfig>\n");
        let tmp = self.fontconfig_conf.with_extension("conf.shelf.tmp");
        std::fs::write(&tmp, x).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.fontconfig_conf).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// ~/.config/shelf/fontconfig-fonts.conf.pre-shelf.bak（首次接管前的原配置备份）。
    fn config_root_backup(&self) -> PathBuf {
        self.fontconfig_conf.parent().and_then(|p| p.parent()).map(|c| c.join("shelf/fontconfig-fonts.conf.pre-shelf.bak")).unwrap_or_else(|| PathBuf::from("fontconfig-fonts.conf.pre-shelf.bak"))
    }

    /// 当前中文回退链（覆盖率降序的字体 key）——回执/状态展示用。
    pub fn cjk_fallback_keys(&self) -> Vec<String> {
        Self::cjk_fallback_order(&self.entries()).into_iter().map(|e| e.key.clone()).collect()
    }

    /// 删整个家族（全部文件）。
    pub fn remove_family(&self, key: &str) -> Result<Vec<String>, String> {
        let Some(e) = self.entries().into_iter().find(|e| e.key == key) else { return Err("没有这个字体家族".into()) };
        for f in &e.files {
            std::fs::remove_file(self.fonts_dir.join(f)).map_err(|err| format!("删 {f} 失败: {err}"))?;
        }
        self.fc_cache();
        self.write_index()?;
        Ok(e.files)
    }
}

impl AssetStore for FontStore {
    fn kind(&self) -> &'static str {
        "font"
    }
    fn allowed_ext(&self) -> &'static [&'static str] {
        FONT_EXTS
    }
    fn validate(&self, _name: &str, staged: &Path) -> Result<(), String> {
        let mut head = [0u8; 4];
        std::io::Read::read_exact(&mut std::fs::File::open(staged).map_err(|e| e.to_string())?, &mut head).map_err(|_| "文件太小".to_string())?;
        if !ttf::is_font(&head) {
            return Err("不是 TrueType/OpenType 字体文件".into());
        }
        Ok(())
    }
    fn install(&self, name: &str, staged: &Path) -> Result<AssetItem, String> {
        std::fs::create_dir_all(&self.fonts_dir).map_err(|e| e.to_string())?;
        let dest = self.fonts_dir.join(name);
        std::fs::copy(staged, &dest).map_err(|e| format!("写入字体目录失败: {e}"))?;
        let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        self.fc_cache();
        let fonts = self.write_index()?;
        let entry = fonts.iter().find(|e| e.files.iter().any(|f| f == name)).cloned();
        let pct = entry.as_ref().map(|e| e.cjk_pct).unwrap_or(0);
        let is_cjk = pct >= CJK_MIN_PCT;
        let mut warn = String::new();
        if is_cjk && pct < CJK_LOW_PCT {
            warn = format!("中文覆盖率仅 {pct}%（正文会有生僻字缺字/方框，适合做标题或点缀，不建议当正文主字体）");
        } else if !is_cjk && pct > 0 {
            warn = format!("中文覆盖率仅 {pct}%，不作中文回退");
        }
        let extra = serde_json::json!({
            "family": entry.as_ref().map(|e| e.key.clone()).unwrap_or_default(),
            "names": entry.as_ref().map(|e| e.names.clone()),
            "cjkPct": pct, "isCjk": is_cjk,
            "fallback": is_cjk, "warn": warn,
        });
        Ok(AssetItem { name: name.into(), bytes, extra })
    }
    fn success_message(&self, _requested: &str, item: &AssetItem) -> String {
        match item.extra.get("family").and_then(|f| f.as_str()).filter(|f| !f.is_empty()) {
            Some(f) => format!("已安装（家族 {f}）"),
            None => "已安装".into(),
        }
    }
    fn list(&self) -> Vec<AssetItem> {
        self.entries()
            .into_iter()
            .map(|e| {
                let bytes: u64 = e.files.iter().map(|f| std::fs::metadata(self.fonts_dir.join(f)).map(|m| m.len()).unwrap_or(0)).sum();
                AssetItem { name: e.key.clone(), bytes, extra: serde_json::json!({"family": e.key, "files": e.files, "names": e.names, "cjkPct": e.cjk_pct, "fontconfigRef": e.fontconfig_ref}) }
            })
            .collect()
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.remove_family(name).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmsvc_core::asset::AssetUploadFlow;

    fn setup() -> (tempfile::TempDir, Paths, FontStore) {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        paths.ensure().unwrap();
        let mut s = FontStore::new(&paths, FontConfig::default());
        s.side_effects = false;
        (t, paths, s)
    }

    fn body(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut b = Vec::new();
        for (f, d) in files {
            b.extend_from_slice(format!("--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{f}\"\r\n\r\n").as_bytes());
            b.extend_from_slice(d);
            b.extend_from_slice(b"\r\n");
        }
        b.extend_from_slice(b"--B--\r\n");
        b
    }

    #[test]
    fn all_fonts_uniform_grouped_by_family_and_deletable() {
        let (_t, paths, store) = setup();
        std::fs::create_dir_all(store.fonts_dir()).unwrap();
        // 预先存在的（旧套件 scp 进来的）字体也一视同仁；无 name 表 → 家族=文件名去扩展名
        std::fs::write(store.fonts_dir().join("Old-Regular.ttf"), b"\x00\x01\x00\x00").unwrap();
        // fontconfig 引用它 → 标 fontconfigRef
        std::fs::create_dir_all(store.fontconfig_conf.parent().unwrap()).unwrap();
        std::fs::write(&store.fontconfig_conf, "<fontconfig><alias><family>Old-Regular</family></alias></fontconfig>").unwrap();
        let font = b"OTTO\x00\x00\x00\x00rest";
        let out = AssetUploadFlow::new(&paths).run(&store, &body(&[("New.otf", font), ("junk.ttf", b"PK\x03\x04junk")])[..], "B").unwrap();
        assert!(out[0].ok && out[0].item.as_ref().unwrap().extra["family"] == "New");
        assert_eq!(out[1].message, "不是 TrueType/OpenType 字体文件");
        let list = store.list();
        assert_eq!(list.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), vec!["New", "Old-Regular"]);
        assert_eq!(list[1].extra["fontconfigRef"], true);
        assert_eq!(list[0].extra["fontconfigRef"], false);
        let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(store.json_path()).unwrap()).unwrap();
        assert_eq!(j["fonts"].as_array().unwrap().len(), 2);
        assert!(j["fonts"][0].get("source").is_none(), "不再有 source/内建概念");
        // 旧字体也可删
        assert_eq!(store.remove_family("Old-Regular").unwrap(), vec!["Old-Regular.ttf"]);
        assert_eq!(store.list().len(), 1);
        assert!(store.remove("nope").is_err());
    }


    fn entry(key: &str, pct: u8) -> FontEntry {
        FontEntry { key: key.into(), files: vec![format!("{key}.ttf")], names: Names { cn: key.into(), tw: key.into(), en: key.into() }, cjk_pct: pct, fontconfig_ref: false }
    }

    #[test]
    fn fontconfig_weak_fallback_ordered_by_coverage_and_backs_up() {
        let (_t, _paths, store) = setup();
        std::fs::create_dir_all(store.fontconfig_conf.parent().unwrap()).unwrap();
        // 首次接管前已有非 shelf 配置 → 应备份
        std::fs::write(&store.fontconfig_conf, "<fontconfig><!-- chinese-ime 旧配置 --></fontconfig>").unwrap();
        let fonts = vec![entry("Art Font", 35), entry("Big CJK", 99), entry("Latin Only", 0), entry("Mid CJK", 90)];
        store.write_fontconfig(&fonts).unwrap();
        let out = std::fs::read_to_string(&store.fontconfig_conf).unwrap();
        assert!(out.contains(FC_MARK), "带 shelf 标记");
        assert!(std::fs::read_to_string(store.config_root_backup()).unwrap().contains("chinese-ime"), "原配置已备份");
        // 全 weak，无 strong
        assert!(!out.contains("strong"), "不得有 strong 绑定（否则盖过用户选择）: {out}");
        assert!(out.contains(r#"binding="weak""#));
        // 覆盖率降序：Big CJK(99) 在 Mid CJK(90) 之前，二者在 Art Font(35) 之前；Latin(0) 与 <8% 不入链
        let ib = out.find("Big CJK").unwrap();
        let im = out.find("Mid CJK").unwrap();
        let ia = out.find("Art Font").unwrap();
        assert!(ib < im && im < ia, "按覆盖率降序: {out}");
        assert!(!out.contains("Latin Only"), "非中文字体不入回退链");
        // 幂等 + 二次不再覆盖备份
        std::fs::write(store.config_root_backup(), "SENTINEL").unwrap();
        store.write_fontconfig(&fonts).unwrap();
        assert_eq!(std::fs::read_to_string(store.config_root_backup()).unwrap(), "SENTINEL", "已备份则不再覆盖");
        // 无中文字体 → 空回退但不报错
        store.write_fontconfig(&[entry("Latin", 0)]).unwrap();
        assert!(std::fs::read_to_string(&store.fontconfig_conf).unwrap().contains("没有已装的中文字体"));
        // embolden 机制：关则无 <match target=font>，开则每个回退字体一条（不依赖默认值）
        store.embolden.store(false, std::sync::atomic::Ordering::Relaxed);
        store.write_fontconfig(&fonts).unwrap();
        assert!(!std::fs::read_to_string(&store.fontconfig_conf).unwrap().contains("embolden"), "关则无");
        store.embolden.store(true, std::sync::atomic::Ordering::Relaxed);
        store.write_fontconfig(&fonts).unwrap();
        let e = std::fs::read_to_string(&store.fontconfig_conf).unwrap();
        assert!(e.contains("<match target=\"font\">") && e.contains("<edit name=\"embolden\""), "开 embolden 应加 match: {e}");
    }
    #[test]
    fn embolden_defaults_on() {
        assert!(FontConfig::default().embolden_cjk_fallback, "默认开");
    }
    #[test]
    fn family_splitting_and_localized_name() {
        assert_eq!(split_families("LXGW WenKai,霞鹜文楷"), vec!["LXGW WenKai", "霞鹜文楷"]);
        assert_eq!(split_families("KF Readerly"), vec!["KF Readerly"]);
    }
}
