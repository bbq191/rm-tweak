//! 入库：新书落母版库（字节 / 已落盘暂存文件）与网文抓取。
use super::*;

/// 跨分区入库时的临时文件后缀（点前缀名，列表看不见；进程中途被杀留下的由 `recover_interrupted` 清掉）。
pub(super) const LANDING_TMP_SUFFIX: &str = ".landing.tmp";

pub(super) fn landed_name(p: &Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("book").to_string()
}

impl Staging {
    // ───────────── 入库 ─────────────

    /// 新入库（字节）：原子写，同名加数字前缀不覆盖。返回落地文件名。
    pub fn stage_new(&self, name: &str, bytes: &[u8]) -> Result<String, String> {
        self.stage_bytes(name, bytes, false)
    }

    /// [`Self::stage_new`] 的实现。`hold_busy`＝在挑名落地的**同一个临界区里**给新条目加上忙锁，调用方负责之后
    /// `end_busy`——从书出现在母版库那一刻起删除/改名/优化/落库就会被忙锁拒，不留"已落地、还没上锁"的空窗
    /// （抓网文的同步优化用，2026-09-25）。新名字是临界区里刚挑出来的，通常没人占着；极少数撞上（比如正有人
    /// 把别的书改名成这个名字、还没落地）就如实报忙、不落地。忙锁表只做一次非阻塞查插，在临界区里拿不会死锁。
    fn stage_bytes(&self, name: &str, bytes: &[u8], hold_busy: bool) -> Result<String, String> {
        let _land = self.land_guard();
        let target = unique_path(&self.dir, &canonical_staged_name(plain_name(name)?));
        let landed = landed_name(&target);
        if hold_busy && !self.try_start_busy(&landed) {
            return Err(busy_err(&landed, ""));
        }
        sidecar::remove(&target); // 目标名是全新的，遗留的同名边车一定是旧书的，别让新书继承
        if let Err(e) = write_atomic(&target, bytes) {
            if hold_busy {
                self.end_busy(&landed);
            }
            return Err(format!("写母版库失败: {e}"));
        }
        Ok(landed)
    }

    /// 新入库（已落盘的暂存文件）：同分区 rename 不拷贝（上传 / inbox 追平的大书走这里）。返回落地文件名。
    ///
    /// 跨分区（rename 失败）时先在临界区**外**把字节拷进母版库目录下的点前缀临时文件，再回临界区挑名、同目录 rename 落地
    /// （2026-09-25 第四轮审计）：此前直接往最终名上 `copy`——拷贝期间一本半截的书已经出现在列表里、可被优化/落库/下载，
    /// 拷贝失败还留下这半截；而且整段拷贝（上百 MB）都攥着落名锁，别的入库全被卡住。
    pub fn stage_from_path(&self, name: &str, src: &Path) -> Result<String, String> {
        let canon = canonical_staged_name(plain_name(name)?);
        {
            let _land = self.land_guard();
            let target = unique_path(&self.dir, &canon);
            sidecar::remove(&target);
            if std::fs::rename(src, &target).is_ok() {
                return Ok(landed_name(&target));
            }
        }
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let tmp = self.dir.join(format!(".{}.{}{LANDING_TMP_SUFFIX}", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        if let Err(e) = std::fs::copy(src, &tmp) {
            let _ = std::fs::remove_file(&tmp);
            return Err(format!("写母版库失败: {e}"));
        }
        let landed = {
            let _land = self.land_guard();
            let target = unique_path(&self.dir, &canon);
            sidecar::remove(&target);
            std::fs::rename(&tmp, &target).map(|_| landed_name(&target))
        };
        match landed {
            Ok(n) => {
                let _ = std::fs::remove_file(src);
                Ok(n)
            }
            Err(e) => {
                let _ = std::fs::remove_file(&tmp);
                Err(format!("写母版库失败: {e}"))
            }
        }
    }

    /// 网文抓取（Readability + 白名单）→ 组 EPUB 落母版库。`optimize`＝网页「同步优化」复选框：请求了就紧接着
    /// 跑一遍跟「母版库→优化」按钮同一个 `optimize()`，不用用户再手动点一次——`article.rs` 的属性
    /// 白名单本来就不留 class/style，正文没有任何 CSS，不经 wash 层的边距/段距归零会在 xochitl 上按默认段距
    /// 渲染出大片留空（真机反馈）。`optimize=false` 保留原行为：core 级落库，用户按需再点。同步优化失败不影响
    /// 入库结果（已经抓到的文章不因为这一步失败就整个丢掉），失败原因原样带回给调用方决定怎么措辞。
    ///
    /// 同步优化**占忙锁**（2026-09-25 补）：跟「优化」「落库」「删除」「改名」同一张 `OpRegistry`，优化期间对这本书的
    /// 删除/改名/再优化/落库一律回"正在处理中"，列表里也显示忙。并发/内存闸门在网关那头（`gateway/src/proxy.rs` 的
    /// `FetchArticle`，勾了同步优化才占小档名额）。
    pub fn fetch_article(&self, url: &str, optimize: bool) -> Result<FetchArticleOutcome, String> {
        let (epub, title) = bookconv::article::build_article_epub(url)?;
        let fname = format!("{}.epub", bookconv::util::sanitize_filename(&title, "article"));
        self.land_article(&fname, &epub, title, optimize, |_, _| {})
    }

    /// [`Self::fetch_article`] 抓取之后的部分（落地 + 可选同步优化），拆出来便于离线测（抓取要联网）。
    /// `on_progress` 原样转给 [`Self::optimize`]；生产传空闭包，测试借它在"优化进行中"那一刻检查忙锁。
    pub(super) fn land_article(&self, fname: &str, epub: &[u8], title: String, optimize: bool, on_progress: impl FnMut(usize, usize)) -> Result<FetchArticleOutcome, String> {
        if !optimize {
            let name = self.stage_new(fname, epub)?;
            return Ok(FetchArticleOutcome { name, title, optimized: false, optimize_error: None });
        }
        let name = self.stage_bytes(fname, epub, true)?;
        // 忙锁无论如何都要清：panic 也兜住（同 `spawn_optimize`），不然这本书从此删不掉、优化不了。
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.optimize(&name, on_progress)))
            .unwrap_or_else(|_| Err("优化过程内部异常（已捕获，不影响其他操作）".to_string()));
        self.end_busy(&name);
        let (optimized, optimize_error) = match result {
            Ok(_) => (true, None),
            Err(e) => (false, Some(e)),
        };
        Ok(FetchArticleOutcome { name, title, optimized, optimize_error })
    }
}
