//! 伪 DRM：`encryption.xml` 只加密字体/样式/脚本的"伪加密"剥掉，真加密（加密了正文等）报错。
use super::*;

/// 伪 DRM 允许加密的扩展名（= strip_pseudo_drm.py SAFE_EXTS）。
pub const PSEUDO_DRM_SAFE_EXTS: &[&str] = &[".css", ".ttf", ".otf", ".woff", ".woff2", ".js"];
// ───────────────────────── 1. 伪 DRM ─────────────────────────

/// encryption.xml 里的加密目标（zip 内路径）。
/// `META-INF/encryption.xml` 里 `<CipherReference URI="…">` 的匹配（质量门 `check` 与清洗层共用）。
pub(crate) fn cipher_reference_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"CipherReference\s+URI="([^"]+)""#).unwrap())
}

pub fn encrypted_targets(entries: &[Entry]) -> Option<Vec<String>> {
    let enc = entries.iter().find(|e| e.name == "META-INF/encryption.xml")?;
    let t = String::from_utf8_lossy(&enc.data);
    Some(cipher_reference_re().captures_iter(&t).map(|c| posix_norm(&percent_decode(&c[1]))).filter(|u| !u.starts_with('#')).collect())
}

/// 真 DRM 判据：加密了非样式/字体/脚本的文件。返回违规项。
pub fn real_drm_items(targets: &[String]) -> Vec<String> {
    targets.iter().filter(|t| { let l = t.to_ascii_lowercase(); !PSEUDO_DRM_SAFE_EXTS.iter().any(|e| l.ends_with(e)) }).cloned().collect()
}

pub(super) fn strip_pseudo_drm(entries: &mut Vec<Entry>, rep: &mut WashReport) -> Result<(), String> {
    let Some(targets) = encrypted_targets(entries) else { return Ok(()) };
    let bad = real_drm_items(&targets);
    if !bad.is_empty() {
        return Err(format!("加密 EPUB（真 DRM，加密了 {} 等 {} 项），xochitl/KOReader 都读不了", bad.iter().take(3).cloned().collect::<Vec<_>>().join("、"), bad.len()));
    }
    let drop: HashSet<String> = targets.iter().cloned().chain(std::iter::once("META-INF/encryption.xml".to_string())).collect();
    if let Some(oi) = find_opf(entries) {
        let opf_dir = dir_of(&entries[oi].name).to_string();
        let mut text = String::from_utf8_lossy(&entries[oi].data).into_owned();
        for t in &targets {
            let rel = relative_to(&opf_dir, t);
            let re = Regex::new(&format!(r#"<item\b[^>]*\bhref="{}"[^>]*/>\s*"#, regex::escape(&rel))).unwrap();
            text = re.replace_all(&text, "").into_owned();
        }
        entries[oi].data = text.into_bytes();
    }
    entries.retain(|e| !drop.contains(&e.name));
    rep.pseudo_drm_stripped = targets;
    Ok(())
}
