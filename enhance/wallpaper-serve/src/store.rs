//! 壁纸仓库（`AssetStore` 实现）+ 轮换状态。真机结论（2026-09-03 bind 时代 → 2026-09-06 原生键时代）：
//! - 休眠屏由 xochitl.conf `SleepScreenPath` 指向本仓库的 `current.png`（native.rs）；xochitl **每次休眠重读该文件**，
//!   满屏 PreserveAspectFit、插画卡自动隐藏 → 换图零重启即时生效，不写 `/usr`、不 bind-mount；
//! - 换图**原地覆盖 current.png**（truncate 写、保 inode，路径与 inode 都不变）；
//! - 竖屏物理尺寸 954×1696，上传即缩放入池。
//! 路径全走 XDG：池 `$XDG_DATA_HOME/shelf/wallpapers/pool/`、`current.png` 同级；状态 `$XDG_STATE_HOME/shelf/wallpaper-state.json`。
use image::imageops::FilterType;
use image::{GenericImageView, ImageFormat, RgbaImage};
use serde::{Deserialize, Serialize};
use rmsvc_core::asset::{AssetItem, AssetStore};
use rmsvc_core::formats::IMAGE_EXTS;
use rmsvc_core::fs::plain_name;
use rmsvc_core::paths::Paths;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 竖屏物理尺寸：954×1696 @264PPI。2026-09-11 从 shelf/crates/bookconv::imgopt 的同名常量复制
/// 过来（剥离移植，不再路径依赖 bookconv——那是书处理业务 crate，不适合当 enhance/ 的依赖）；
/// 跟 shelf 那边如果哪天屏幕规格变了，两处要分别改。
pub const W: u32 = 954;
pub const H: u32 = 1696;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    #[default]
    Sequential,
    Random,
    Fixed,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    #[default]
    Cover,
    Contain,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Default)]
#[serde(default, rename_all = "camelCase")]
pub struct WpState {
    pub mode: Mode,
    pub current: Option<String>,
}

pub struct WallpaperStore {
    pool: PathBuf,
    current: PathBuf,
    state_file: PathBuf,
    pub fit: Fit,
}

impl WallpaperStore {
    pub fn new(paths: &Paths) -> WallpaperStore {
        let base = paths.data_dir().join("wallpapers");
        WallpaperStore { pool: base.join("pool"), current: base.join("current.png"), state_file: paths.state_dir().join("wallpaper-state.json"), fit: Fit::Cover }
    }
    pub fn pool(&self) -> &Path {
        &self.pool
    }
    pub fn current_path(&self) -> &Path {
        &self.current
    }
    pub fn ensure(&self) -> Result<(), String> {
        std::fs::create_dir_all(&self.pool).map_err(|e| e.to_string())?;
        if let Some(p) = self.state_file.parent() {
            std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    pub fn state(&self) -> WpState {
        rmsvc_core::config::load_or_default(&self.state_file)
    }
    pub fn save_state(&self, st: &WpState) -> Result<(), String> {
        rmsvc_core::config::save(&self.state_file, st, None)
    }
    pub fn set_mode(&self, mode: Mode) -> Result<(), String> {
        let mut st = self.state();
        st.mode = mode;
        self.save_state(&st)
    }

    pub fn names(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.pool)
            .map(|rd| rd.flatten().filter(|e| e.path().extension().map(|x| x == "png").unwrap_or(false)).map(|e| e.file_name().to_string_lossy().to_string()).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// 池里的图按名读字节（预览用）。
    pub fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        std::fs::read(self.pool.join(plain_name(name)?)).map_err(|_| "池里没有这张图".to_string())
    }

    /// 原地覆盖 current.png（truncate 写、保 inode；xochitl 每次休眠按 SleepScreenPath 重读）。
    pub fn activate(&self, name: &str) -> Result<(), String> {
        let src = self.pool.join(plain_name(name)?);
        let data = std::fs::read(&src).map_err(|_| "池里没有这张图".to_string())?;
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&self.current).map_err(|e| e.to_string())?;
        f.write_all(&data).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        let mut st = self.state();
        st.current = Some(name.to_string());
        self.save_state(&st)
    }

    /// 按 mode 选下一张并激活；返回激活的名字（fixed/空池 → None）。
    pub fn roll(&self) -> Result<Option<String>, String> {
        let st = self.state();
        let names = self.names();
        if names.is_empty() || st.mode == Mode::Fixed {
            return Ok(None);
        }
        let next = match st.mode {
            Mode::Sequential => {
                let i = st.current.as_ref().and_then(|c| names.iter().position(|n| n == c)).map(|i| (i + 1) % names.len()).unwrap_or(0);
                names[i].clone()
            }
            Mode::Random => {
                let seed = rmsvc_core::clock::now_nanos() as usize;
                let mut i = seed % names.len();
                if names.len() > 1 && st.current.as_deref() == Some(names[i].as_str()) {
                    i = (i + 1) % names.len();
                }
                names[i].clone()
            }
            Mode::Fixed => unreachable!(),
        };
        self.activate(&next)?;
        Ok(Some(next))
    }
}

/// 任意尺寸 → 竖屏 954×1696 RGBA PNG 字节。cover=等比放大后居中裁；contain=等比缩进画布、黑边补齐。
pub fn fit_to_screen(src: &[u8], fit: Fit) -> Result<Vec<u8>, String> {
    let img = image::load_from_memory(src).map_err(|e| format!("解码失败: {e}"))?;
    let (w, h) = img.dimensions();
    let (sw, sh) = (W as f64, H as f64);
    let scale = match fit {
        Fit::Cover => (sw / w as f64).max(sh / h as f64),
        Fit::Contain => (sw / w as f64).min(sh / h as f64),
    };
    let nw = ((w as f64 * scale).round() as u32).max(1);
    let nh = ((h as f64 * scale).round() as u32).max(1);
    let resized = img.resize_exact(nw, nh, FilterType::Lanczos3).to_rgba8();
    let mut canvas = RgbaImage::from_pixel(W, H, image::Rgba([0, 0, 0, 255]));
    let ox = (nw as i64 - W as i64) / 2;
    let oy = (nh as i64 - H as i64) / 2;
    for y in 0..H {
        for x in 0..W {
            let sx = x as i64 + ox;
            let sy = y as i64 + oy;
            if sx >= 0 && sy >= 0 && (sx as u32) < nw && (sy as u32) < nh {
                canvas.put_pixel(x, y, *resized.get_pixel(sx as u32, sy as u32));
            }
        }
    }
    let mut out = std::io::Cursor::new(Vec::new());
    canvas.write_to(&mut out, ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

impl AssetStore for WallpaperStore {
    fn kind(&self) -> &'static str {
        "wallpaper"
    }
    fn allowed_ext(&self) -> &'static [&'static str] {
        IMAGE_EXTS
    }
    fn validate(&self, _name: &str, staged: &Path) -> Result<(), String> {
        let md = std::fs::metadata(staged).map_err(|e| e.to_string())?;
        if md.len() > 15 * 1024 * 1024 {
            return Err("图片超过 15MB（设备内存有限，请先缩小）".into());
        }
        let mut head = [0u8; 4];
        std::io::Read::read_exact(&mut std::fs::File::open(staged).map_err(|e| e.to_string())?, &mut head).map_err(|_| "文件太小".to_string())?;
        if !(head.starts_with(&[0xFF, 0xD8]) || head.starts_with(b"\x89PNG")) {
            return Err("只收 JPEG/PNG".into());
        }
        Ok(())
    }
    fn install(&self, name: &str, staged: &Path) -> Result<AssetItem, String> {
        let src = std::fs::read(staged).map_err(|e| e.to_string())?;
        let png = fit_to_screen(&src, self.fit)?;
        let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
        let out_name = format!("{stem}.png");
        let dest = self.pool.join(&out_name);
        std::fs::write(&dest, &png).map_err(|e| e.to_string())?;
        Ok(AssetItem { name: out_name, bytes: png.len() as u64, extra: serde_json::json!({"width": W, "height": H}) })
    }
    fn success_message(&self, _requested: &str, _item: &AssetItem) -> String {
        format!("已入池（缩放到 {W}×{H}）")
    }
    fn list(&self) -> Vec<AssetItem> {
        self.names().into_iter().map(|n| AssetItem { name: n.clone(), bytes: std::fs::metadata(self.pool.join(&n)).map(|m| m.len()).unwrap_or(0), extra: serde_json::json!({"current": self.state().current.as_deref() == Some(n.as_str())}) }).collect()
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        let n = plain_name(name)?;
        if self.state().current.as_deref() == Some(n) {
            return Err("正在使用的壁纸不能删，先换一张".into());
        }
        std::fs::remove_file(self.pool.join(n)).map_err(|e| format!("删除失败: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = RgbaImage::from_fn(w, h, |x, y| image::Rgba([(x % 256) as u8, (y % 256) as u8, 128, 255]));
        let mut c = std::io::Cursor::new(Vec::new());
        img.write_to(&mut c, ImageFormat::Png).unwrap();
        c.into_inner()
    }

    fn store() -> (tempfile::TempDir, WallpaperStore) {
        let t = tempfile::tempdir().unwrap();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" { Some(h.clone()) } else { None });
        let s = WallpaperStore::new(&paths);
        s.ensure().unwrap();
        (t, s)
    }

    #[test]
    fn fit_produces_screen_size_both_modes() {
        for (w, h) in [(4000, 3000), (300, 900), (954, 1696)] {
            for fit in [Fit::Cover, Fit::Contain] {
                let out = fit_to_screen(&png(w, h), fit).unwrap();
                let img = image::load_from_memory(&out).unwrap();
                assert_eq!(img.dimensions(), (W, H), "{w}x{h} {fit:?}");
            }
        }
        assert!(fit_to_screen(b"nope", Fit::Cover).is_err());
    }

    #[test]
    fn activate_keeps_inode_and_roll_cycles() {
        let (_t, s) = store();
        for n in ["a.png", "b.png", "c.png"] {
            std::fs::write(s.pool().join(n), png(10, 10)).unwrap();
        }
        s.activate("a.png").unwrap();
        let ino = std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(s.current_path()).unwrap());
        assert_eq!(s.roll().unwrap().as_deref(), Some("b.png"));
        assert_eq!(s.roll().unwrap().as_deref(), Some("c.png"));
        assert_eq!(s.roll().unwrap().as_deref(), Some("a.png"));
        assert_eq!(std::os::unix::fs::MetadataExt::ino(&std::fs::metadata(s.current_path()).unwrap()), ino, "原地覆盖必须保 inode");
        s.set_mode(Mode::Fixed).unwrap();
        assert_eq!(s.roll().unwrap(), None);
        s.set_mode(Mode::Random).unwrap();
        assert!(s.roll().unwrap().is_some());
        assert!(s.remove(&s.state().current.clone().unwrap()).is_err());
    }

    #[test]
    fn upload_flow_installs_resized_png() {
        let (t, s) = store();
        let h = t.path().to_str().unwrap().to_string();
        let paths = Paths::resolve(move |k| if k == "HOME" || k == "XDG_RUNTIME_DIR" { Some(h.clone()) } else { None });
        paths.ensure().unwrap();
        let mut body = Vec::new();
        body.extend_from_slice(b"--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"beach.jpg\"\r\n\r\n");
        body.extend_from_slice(&png(600, 400)); // 扩展名 jpg、内容 png：按魔数收
        body.extend_from_slice(b"\r\n--B\r\nContent-Disposition: form-data; name=\"file\"; filename=\"t.png\"\r\n\r\nnotimage\r\n--B--\r\n");
        let out = rmsvc_core::asset::AssetUploadFlow::new(&paths).run(&s, &body[..], "B").unwrap();
        assert!(out[0].ok && out[0].item.as_ref().unwrap().name == "beach.png");
        assert_eq!(out[1].message, "只收 JPEG/PNG");
        assert_eq!(s.names(), vec!["beach.png"]);
    }
}
