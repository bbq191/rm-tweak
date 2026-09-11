//! 按书的阅读方向（2026-09-25）：母版库里每本 EPUB 可设"自动 / 从右往左 / 从左往右"。
//!
//! 痛点：calibre 转出的日漫 OPF 多半没写 `page-progression-direction="rtl"`，xochitl 里的日漫翻页规则
//! （`reader-page-turn.qmd` → `GET /reading-direction/{uuid}`）就认不出来，此前只能手改 `rtl-overrides.json`。
//! 现在：
//! - 设置存边车 `direction`（跟着书走：改名一起挪、删书一起删）；
//! - 「优化」时写进 OPF spine（`bookconv::direction`）。已完整优化过的书只改 OPF、其余条目原样拷贝，不再跑一遍
//!   完整优化（多一代 JPEG 有损，见 `Staging::optimize`）；
//! - 设置与母版文件实际方向不一致 → 列表 `directionStale=true`、`optimized=false`，网页据此提示"要再优化"；
//! - 已加入过 xochitl（边车 `render.uuid`）的书，设置时顺手把那个 uuid 写进/移出手动清单，不必重投即生效；之后
//!   渲染自检认到新 uuid 时也按设置同步（[`Staging::sync_override`]）。
//!
//! **只做手动**：漫画识别看得出"是漫画"，看不出"是日漫"（国漫、美漫从左往右），不自动设。
use super::*;
use bookconv::direction::PageDirection;

/// 边车里存的值 → 方向（`None`＝自动）。
pub(super) fn parse_pref(s: Option<&str>) -> Option<PageDirection> {
    s.and_then(PageDirection::parse)
}

/// 方向 → 列表/接口里的字符串。
pub(super) fn pref_label(p: Option<PageDirection>) -> &'static str {
    p.map_or("auto", PageDirection::as_str)
}

/// 设置与文件里写的方向是否不一致（要再优化才生效）。按 xochitl 的判据只分"是不是 rtl"：设"从左往右"而书里没写
/// 方向，本来就是从左往右，不算不一致。自动恒为一致。
pub(super) fn is_stale(pref: Option<PageDirection>, spine: Option<PageDirection>) -> bool {
    pref.is_some_and(|d| (d == PageDirection::Rtl) != (spine == Some(PageDirection::Rtl)))
}

/// `set_direction` 的结果。
#[derive(Debug, PartialEq)]
pub struct DirectionOutcome {
    /// 母版文件还没按新设置改过（要再点「优化」）。
    pub stale: bool,
    /// 已同步进/出手动清单的落库 uuid（`None`＝这本没加入过 xochitl，或清单本来就对）。
    pub synced: Option<String>,
    /// 同步手动清单失败的原因（不影响设置本身已保存）。
    pub sync_error: Option<String>,
}

impl Staging {
    /// 母版库里这本书当前的方向设置（没有边车 / 没设＝自动）。
    pub(crate) fn direction_pref(&self, path: &Path) -> Option<PageDirection> {
        sidecar::read(path).and_then(|d| parse_pref(d.direction.as_deref()))
    }

    /// 设一本 EPUB 的阅读方向（`None`＝自动）。只写设置、不动书本身；书要等下次「优化」才改。
    pub fn set_direction(&self, name: &str, dir: Option<PageDirection>) -> Result<DirectionOutcome, String> {
        if formats::ext_of(name) != "epub" {
            return Err(format!("《{name}》不是 EPUB，只有 EPUB 能设阅读方向"));
        }
        let p = self.existing(name)?;
        let mut uuid = None;
        sidecar::update(&p, |d| {
            d.direction = dir.map(|x| x.as_str().to_string());
            uuid = d.render.as_ref().map(|r| r.uuid.clone()).filter(|u| !u.is_empty());
        })?;
        let stale = is_stale(dir, bookconv::direction::spine_direction_file(&p));
        let (mut synced, mut sync_error) = (None, None);
        if let Some(u) = uuid {
            match self.sync_override(&u, dir, true) {
                Some(Ok(true)) => synced = Some(u),
                Some(Err(e)) => sync_error = Some(e),
                _ => {}
            }
        }
        Ok(DirectionOutcome { stale, synced, sync_error })
    }

    /// 把已落库副本 `uuid` 按设置同步进手动清单：从右往左＝加入；从左往右＝移出；自动＝`clear_on_auto` 时移出
    /// （用户在网页上主动改回自动），否则不动（渲染自检认到新 uuid 时，自动的书不碰清单——清单里可能是用户手加的）。
    /// 没接清单 → `None`；否则 `Some(是否改了清单)`。
    pub(crate) fn sync_override(&self, uuid: &str, dir: Option<PageDirection>, clear_on_auto: bool) -> Option<Result<bool, String>> {
        let rd = self.reading_direction.as_ref()?;
        let on = match dir {
            Some(PageDirection::Rtl) => true,
            Some(PageDirection::Ltr) => false,
            None if clear_on_auto => false,
            None => return None,
        };
        Some(rd.set_override(uuid, on))
    }

    /// 已完整优化过的书只是方向设置变了：只改 OPF，其余条目压缩数据原样拷贝（不重编码图片），过质量门再替换。
    pub(super) fn rewrite_direction_only(&self, name: &str, p: &Path, dir: PageDirection) -> Result<String, String> {
        let tmp = p.with_file_name(format!(".{name}.optimizing.tmp"));
        bookconv::util::produce_then_replace(&tmp, p, |t| {
            bookconv::direction::rewrite_direction_file(p, t, dir)?;
            let check = bookconv::check::check_epub_file(t).map_err(|e| format!("质量门校验失败: {e}"))?;
            if !check.ok {
                return Err(format!("改方向后的产物未通过质量门，{}", check.errors.join("；")));
            }
            Ok(())
        })?;
        let label = if dir == PageDirection::Rtl { "从右往左" } else { "从左往右" };
        Ok(format!("已把《{name}》的阅读方向改为{label}（书已优化过，只改了 OPF 里的方向标记，图片未重新处理）"))
    }
}
