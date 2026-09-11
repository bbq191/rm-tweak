//! EPUB 漫画超限投原生时按 NCX 第一层结构拆分——2026-09-18 用户真机反馈驱动（真机测《火影忍者》
//! 281MB 撞 xochitl `/upload` 上限，之前的规则是"大部头漫画默认不投原生"，用户要求"一旦发现超限
//! 就全部拆"，改成按卷切成能塞进上传上限的若干份，各自独立投递，不再全有全无）。
//! **只对 EPUB 格式漫画做**（用户明确：CBZ 走 host `comic_gray.py`/`cbz_to_pdf` 那条完全独立的
//! Python 管线，这次不碰）。
//!
//! 拆分依据是书自带的 `toc.ncx`——真机《火影忍者》验证过这类 Calibre 转出的合集漫画会在 NCX 里
//! 老老实实标好每一卷的起始页（`<navLabel>火影忍者（卷八）</navLabel><content src="text/part0001.html.../>`），
//! 不用自己猜结构。**规则：整书超预算 → 只按 NCX 第一层节点切一刀，各自成一份**（不再往更深层级
//! 递归——2026-09-19 用户拍板简化，见 `plan_splits` 注释里的完整理由）；某一份切完自己还超预算，
//! 直接标 `fits=false` 放弃（不投原生，调用方据此提示用户"哪一卷没能投上"，原书完整字节仍在
//! 母版库/KOReader，不会为了硬塞进预算而损内容）。

use crate::epub::{assemble, Book, BookMeta, Chapter, Resource};
use crate::wash::{dir_of, is_html, parse_opf, posix_norm, resolve, Entry};
use regex::Regex;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::OnceLock;

fn navpoint_event_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?s)(<navPoint\b)|(</navPoint>)|<navLabel>\s*<text>([^<]*)</text>\s*</navLabel>|<content\s+src="([^"]*)""#).unwrap()
    })
}

/// 线性扫 `toc.ncx`，展平成 `(depth, title, target)` 序列（不建真正的树——跟本 crate 一贯的
/// "扁平+depth"写法一致，如 `wash.rs::dense_ranks`）。NCX 规范保证 `<navLabel>` 和 `<content>`
/// 总是先于自己的子 `<navPoint>` 出现，扫描时按"刚看到 content 就用当前 depth/title 落地一条"
/// 处理即可，不用等子节点扫完。
fn parse_ncx_flat(ncx_text: &str) -> Vec<(usize, String, String)> {
    let mut depth = 0usize;
    let mut cur_title = String::new();
    let mut out = Vec::new();
    for c in navpoint_event_re().captures_iter(ncx_text) {
        if c.get(1).is_some() {
            depth += 1;
        } else if c.get(2).is_some() {
            depth = depth.saturating_sub(1);
        } else if let Some(t) = c.get(3) {
            cur_title = t.as_str().trim().to_string();
        } else if let Some(s) = c.get(4) {
            out.push((depth, std::mem::take(&mut cur_title), s.as_str().to_string()));
        }
    }
    out
}

/// 一页 (x)html 里引用的图片，解析成 zip 内绝对路径（相对该页自身目录解析，去重按出现顺序）。
fn imgs_referenced(html: &str, page_dir: &str) -> Vec<String> {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r#"(?i)<img\b[^>]*\bsrc="([^"]+)""#).unwrap());
    let mut seen = std::collections::HashSet::new();
    re.captures_iter(html)
        .map(|c| posix_norm(&resolve(page_dir, &c[1])))
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// `[start,end)` 这段 spine（含它引用的图片）的字节总量估算——page 自身 + 各自引用的图（按路径去重，
/// 防止极端情况下多页共用同一张图被重复计入）。不追求字节级精确，够判断"塞不塞得进预算"就行。
/// 每条 entry 的"体积"由 `size_of` 决定而不是死取 `data.len()`——流式路径
/// （`deliver_split_streaming`）图片条目是空占位（真实字节留到真正要组包那一刻才读），真实体积从
/// zip 目录里查表拿，不用解压就知道；内存路径（`plan_splits`）直接传 `|e| e.data.len() as u64`
/// 当 `size_of`，两条路径共用同一份体积计算逻辑。
fn range_bytes_sized(entries: &[Entry], spine: &[String], start: usize, end: usize, size_of: &dyn Fn(&Entry) -> u64) -> u64 {
    let mut total = 0u64;
    let mut counted_imgs = std::collections::HashSet::new();
    for p in &spine[start..end] {
        let Some(e) = entries.iter().find(|e| &e.name == p) else { continue };
        total += size_of(e);
        if !is_html(p) {
            continue;
        }
        let Ok(html) = std::str::from_utf8(&e.data) else { continue };
        for img in imgs_referenced(html, dir_of(p)) {
            if counted_imgs.insert(img.clone()) {
                if let Some(ie) = entries.iter().find(|e| e.name == img) {
                    total += size_of(ie);
                }
            }
        }
    }
    total
}

/// 拆分方案的一条：能塞进预算的一份（`fits=true`，调用方据此打包投递）或拆到叶子仍超限被放弃的
/// 一份（`fits=false`，调用方据此提示用户"哪一卷没能投原生"，原书完整字节仍在母版库/KOReader）。
pub struct PlannedPiece {
    pub title: String,
    pub start: usize,
    pub end: usize,
    pub fits: bool,
}

/// 整本书是否超预算、要不要拆的入口。返回 `None`＝整本已经在预算内，调用方按"不用拆，原样整本投"
/// 处理（跟改动前行为完全一致）。拆不出方案（没有 `toc.ncx`/NCX 目标对不上 spine）时报错，调用方
/// 退回"超限直接拒绝"这条改动前就有的老路径，不是新引入的失败模式。
///
/// **只按 NCX 第一层切，不再往更深层级递归**（2026-09-19 用户拍板简化：某一份第一层切完还超预算
/// 就直接标 `fits=false` 放弃，不再往更深一层找）。原来设计成"某一份还超就用它自己更深一层 NCX
/// 节点接着切"，真机《镖人》11 卷 173 章坐实这套递归边界计算有 bug——任何带子节点的顶层条目都会
/// 拿到错误的 end 边界，`range_bytes` 传进 `spine[start..end]` 直接 panic（`range end index
/// 18446744073709551615 out of range for slice of length 173`，book-serve 进程被摔炸、投递
/// 卡死在 pending 永不恢复；《火影忍者》当初验证这条逻辑没暴露，因为它没有更深层级可递归）。
/// 合集漫画每卷体积本来就远低于上传上限（镖人 11 卷均摊 ~71MB、火影 7 卷均摊 ~40MB，budget 通常
/// 150MB 量级），"单卷本身仍超预算"是真实存在但少见的边界情况，为它保留一整套树形递归边界计算的
/// 复杂度（以及随之而来的这类 bug）不值得——简化成只切第一层，代码不再需要建树/算边界，那类 bug
/// 从设计上就不存在了。
pub fn plan_splits(entries: &[Entry], budget: u64) -> Result<Option<Vec<PlannedPiece>>, String> {
    let Some(opf) = parse_opf(entries) else { return Err("解不出 OPF/spine，没法按结构拆".into()) };
    let size_of = |e: &Entry| e.data.len() as u64;
    plan_pieces_sized(entries, &opf, budget, &size_of)
}

/// `plan_splits` 的实现，"体积怎么算"抽成 `size_of` 参数——流式路径（`deliver_split_streaming`）
/// 图片条目是空占位，真实体积从 zip 目录查表拿，不解压也能规划。
fn plan_pieces_sized(entries: &[Entry], opf: &crate::wash::Opf, budget: u64, size_of: &dyn Fn(&Entry) -> u64) -> Result<Option<Vec<PlannedPiece>>, String> {
    let total = range_bytes_sized(entries, &opf.spine, 0, opf.spine.len(), size_of);
    if total <= budget {
        return Ok(None);
    }
    if let Some(top) = ncx_top_level(entries, opf) {
        let mut out = Vec::new();
        for (i, (title, start)) in top.iter().enumerate() {
            let start = *start;
            let end = top.get(i + 1).map(|(_, s)| *s).unwrap_or(opf.spine.len());
            let size = range_bytes_sized(entries, &opf.spine, start, end, size_of);
            if size <= budget {
                out.push(PlannedPiece { title: title.clone(), start, end, fits: true });
                continue;
            }
            // 这一卷本身切完还超预算（真机《镖人》11 卷坐实：11 卷均摊都远低于上传上限，但恰好
            // 有一卷单独 ~96MB，超过 xochitl 自己 ~100MB 的硬上限——用户实测坐实、curl 直传复现的
            // HTTP 413——不是猜出来的边界情况）：复用"没有 toc.ncx"那条退路的按页贪心切法，再切
            // 一层，不为这种情况另建一套树形递归（那正是旧写法真机 panic 的根源）。切出 1 份说明
            // 单页本身已经超预算，切不动，原样保留、如实标 `fits=false`。
            let sub = fixed_page_chunks_range_sized(entries, &opf.spine, start, end, budget, size_of);
            if sub.len() <= 1 {
                out.push(PlannedPiece { title: title.clone(), start, end, fits: false });
            } else {
                let n = sub.len();
                for (j, piece) in sub.into_iter().enumerate() {
                    out.push(PlannedPiece { title: format!("{title}（{}/{n}）", j + 1), start: piece.start, end: piece.end, fits: piece.fits });
                }
            }
        }
        return Ok(Some(out));
    }
    // 没有可用 toc.ncx（书压根没目录，或目录目标一个都对不上 spine）——退化成按页数切：贪心累加
    // 每页体积，快超预算就切一刀，不依赖任何书本身的结构信息，任何超限漫画都能切出方案，不再是
    // "没目录就整本拒绝"。
    Ok(Some(fixed_page_chunks_range_sized(entries, &opf.spine, 0, opf.spine.len(), budget, size_of)))
}

/// NCX 第一层 `(标题, spine 起始下标)` 列表；没有 `toc.ncx`、entry 缺失、或全部目标都对不上 spine
/// （拿不到任何有效切点）时返回 `None`，调用方据此退化到 `fixed_page_chunks`。
fn ncx_top_level(entries: &[Entry], opf: &crate::wash::Opf) -> Option<Vec<(String, usize)>> {
    let ncx_path = opf.ncx.as_ref()?;
    let ncx_entry = entries.iter().find(|e| &e.name == ncx_path)?;
    let ncx_text = String::from_utf8_lossy(&ncx_entry.data);
    let ncx_dir = dir_of(ncx_path);
    let flat = parse_ncx_flat(&ncx_text);
    let resolved: Vec<(usize, String, usize)> = flat
        .into_iter()
        .filter_map(|(depth, title, target)| {
            let no_frag = target.split('#').next().unwrap_or(&target);
            let abs = posix_norm(&resolve(ncx_dir, no_frag));
            opf.spine.iter().position(|p| p == &abs).map(|idx| (depth, title, idx))
        })
        .collect();
    if resolved.is_empty() {
        return None;
    }
    let min_depth = resolved.iter().map(|n| n.0).min().unwrap();
    Some(resolved.into_iter().filter(|n| n.0 == min_depth).map(|(_, t, s)| (t, s)).collect())
}

/// 没有可用目录结构时的退路：贪心按页累加体积，快超预算（加上这一页会超）就切一刀开新的一份；
/// 单页本身已经超预算（罕见，如一张巨图）没法再细分，该份单独成篇、`fits=false` 如实标出。
fn fixed_page_chunks_range_sized(entries: &[Entry], spine: &[String], range_start: usize, range_end: usize, budget: u64, size_of: &dyn Fn(&Entry) -> u64) -> Vec<PlannedPiece> {
    let mut out = Vec::new();
    let (mut start, mut end, mut acc) = (range_start, range_start, 0u64);
    for i in range_start..range_end {
        let page_size = range_bytes_sized(entries, spine, i, i + 1, size_of);
        if acc > 0 && acc + page_size > budget {
            let size = range_bytes_sized(entries, spine, start, end, size_of);
            out.push(PlannedPiece { title: format!("第 {}-{} 页", start + 1, end), start, end, fits: size <= budget });
            start = i;
            acc = 0;
        }
        acc += page_size;
        end = i + 1;
    }
    let size = range_bytes_sized(entries, spine, start, end, size_of);
    out.push(PlannedPiece { title: format!("第 {}-{} 页", start + 1, end), start, end, fits: size <= budget });
    out
}

/// 按规划出的一段 spine range 组一份独立 EPUB：range 内每页各自的图片重新收进
/// `images/NNNN.{ext}`（去重、路径全新分配，不依赖原书目录结构），页面本身简化成
/// "一张图占一页"的最小 body（原页面的 CSS/装饰 wrapper 对纯图片漫画页没有实质意义，不带过去，
/// 避免连带原书内联 style/字体锁这类已经被 `optimize_epub_with` 处理过的东西节外生枝）。
/// 只接 `entries`（不接 `spine`）——`Opf`/`parse_opf` 是 `pub(crate)`，跨 crate（book-serve）调
/// 不到，`spine` 在这里重新解一遍（跟 `plan_splits` 内部解的那次逻辑相同、成本可忽略）。
pub fn build_piece(entries: &[Entry], start: usize, end: usize, title: &str, book_id_suffix: &str) -> Result<Vec<u8>, String> {
    let opf = parse_opf(entries).ok_or("解不出 OPF/spine")?;
    let spine = &opf.spine;
    let mut chapters = Vec::new();
    let mut resources = Vec::new();
    let mut remap: HashMap<String, String> = HashMap::new();
    for p in &spine[start..end] {
        let Some(e) = entries.iter().find(|e| &e.name == p) else { continue };
        if !is_html(p) {
            continue;
        }
        let Ok(html) = std::str::from_utf8(&e.data) else { continue };
        let mut body = String::new();
        for img in imgs_referenced(html, dir_of(p)) {
            let new_path = match remap.get(&img) {
                Some(np) => np.clone(),
                None => {
                    let Some(ie) = entries.iter().find(|e| e.name == img) else { continue };
                    let ext = img.rsplit('.').next().unwrap_or("jpg").to_ascii_lowercase();
                    let media = if ext == "png" { "image/png" } else { "image/jpeg" };
                    let np = format!("images/{:04}.{ext}", resources.len() + 1);
                    resources.push(Resource { path: np.clone(), media_type: media.into(), bytes: ie.data.clone() });
                    remap.insert(img.clone(), np.clone());
                    np
                }
            };
            body.push_str(&format!(r#"<div style="text-align:center"><img src="{new_path}"/></div>"#));
        }
        if !body.is_empty() {
            chapters.push(Chapter { title: String::new(), html_body: body, level: 1 });
        }
    }
    if chapters.is_empty() {
        return Err(format!("《{title}》这一段没有找到可用页面，跳过"));
    }
    let mut book = Book {
        meta: BookMeta {
            book_id: format!("cangjie-comic-split-{book_id_suffix}"),
            title: title.to_string(),
            author: String::new(),
            language: "zh".into(),
            publisher: String::new(),
            cover: None,
            cover_ext: "jpg".into(),
            cover_media_type: "image/jpeg".into(),
        },
        chapters,
        resources,
    };
    let bytes = assemble(&mut book)?;
    repack_with_comic_css(bytes)
}

/// `assemble()`吐出来的页面没有任何 CSS——真机拿真实拆出来的一卷在原生阅读器打开量过
/// （`<uuid>.pdf` pymupdf 测页面 303×538pt，图片实际只占 (17.8,35.5)-(284.8,447.9)，
/// 上下左右都空出一圈，底部尤其空出 90pt），根因是 xochitl 默认文档边距没被清零、`<img>`
/// 没有撑满容器的样式——KOReader 对比之下是真正贴边满屏。补一段外链 `comic.css`（**只用裸元素
/// 选择器**，xochitl CSS 解析器脆，见书架白皮书 §03y 七条实测规则）：`body{margin:0;padding:0}`
/// 清零默认边距，`img{width:100%;height:auto}` 让图片撑满可用宽度。每章头部插入 `<link>`
/// （xochitl 只认外链 css，不认内联 `<style>`，同一条规则）。
fn repack_with_comic_css(bytes: Vec<u8>) -> Result<Vec<u8>, String> {
    let mut entries = crate::check::read_entries(&bytes)?;
    const CSS_PATH: &str = "OEBPS/comic.css";
    const CSS: &str = "body{margin:0;padding:0;}\nimg{width:100%;height:auto;}\n";
    entries.push(Entry { name: CSS_PATH.into(), data: CSS.as_bytes().to_vec() });
    for e in entries.iter_mut() {
        if e.name.starts_with("OEBPS/chap_") && e.name.ends_with(".xhtml") {
            if let Ok(html) = std::str::from_utf8(&e.data) {
                let linked = html.replacen("</head>", "<link rel=\"stylesheet\" type=\"text/css\" href=\"comic.css\"/></head>", 1);
                e.data = linked.into_bytes();
            }
        } else if e.name == "OEBPS/content.opf" {
            if let Ok(opf) = std::str::from_utf8(&e.data) {
                let patched = opf.replacen("</manifest>", "<item id=\"comic-css\" href=\"comic.css\" media-type=\"text/css\"/></manifest>", 1);
                e.data = patched.into_bytes();
            }
        }
    }
    let mut out = Vec::new();
    {
        let cursor = std::io::Cursor::new(&mut out);
        let mut z = zip::ZipWriter::new(cursor);
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        // mimetype 必须首个、STORED（EPUB 规范）。
        if let Some(i) = entries.iter().position(|e| e.name == "mimetype") {
            let m = entries.remove(i);
            z.start_file("mimetype", stored).map_err(|e| e.to_string())?;
            z.write_all(&m.data).map_err(|e| e.to_string())?;
        }
        for e in &entries {
            z.start_file(&e.name, stored).map_err(|e| e.to_string())?;
            z.write_all(&e.data).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    Ok(out)
}

/// 结果：成功投递的份 / 拆到底仍超限或组包失败没能投的份（标题+原因）。
pub struct StreamSplitOutcome {
    pub delivered: Vec<String>,
    pub failed: Vec<String>,
}

/// `plan_splits`+`build_piece` 的流式版：**不整本解压进内存**——真机 785MB《镖人（11 卷）》坐实
/// 内存版会把 book-serve 逼近系统内存上限（跟今早 §03ba 修的 `optimize_epub_file_streaming` 是
/// 同一类风险，只是这条落库拆分路径当时没顺带改）。
///
/// 分两阶段：阶段一只读小文件（`content.opf`/`toc.ncx`/全部 html 页面文本）进内存，图片条目留空
/// 占位——`comic_detect::is_comic` 判定只看 html 里的 `<img>` 计数和正文字数，`plan_pieces_sized`
/// 规划各份边界只需要"体积够不够预算"，而图片的真实体积 zip 目录里直接有（`ZipFile::size()`，
/// **不用解压就知道**），靠 `sizes` 这张表把两者接起来。阶段二逐份处理：轮到某一份，才把它范围内
/// 真正引用到的图片从源文件按需读回真实字节、组包、上传，传完立刻丢，不留给下一份——峰值内存只有
/// "一份的体积"（受 budget 钳制，通常 ≤150MB），不会随全书体积/卷数线性涨。
///
/// `upload_piece(文件名, 字节, 第几份, 预算内共几份) -> Result<(), String>` 由调用方提供（book-serve
/// 侧接 xochitl 上传），本函数不关心具体怎么投、要不要等 xochitl 消化完才传下一份，只管规划+组包+
/// 逐份调用、把这个决定完全交给调用方（真机《镖人（11 卷）》坐实连续紧挨着传会把 xochitl 冲垮——
/// `Connection reset by peer`/`Broken pipe`，但"该等多久"跟 xochitl 端渲染+缩略图+建索引的耗时
/// 挂钩、不是固定数字，猜时间不可靠，见 book-serve `try_deliver_split` 改成等渲染真正完成再放行
/// 下一份，不在这里瞎猜）。第 3/4 个参数是 1-based 进度（2026-09-19 补，给调用方画进度条用）——
/// "预算内共几份"只数拆到底仍超限、注定不投的那几份**之外**的份数，见调用方 `sidecar::
/// StepProgress` 的文档注释。
/// 返回 `None`＝不适用这条路径（整本已经在预算内，或压根不是漫画），调用方按"不用拆，走原来的
/// 整本流程"处理；`Err`＝解不出 OPF/spine 这类致命问题，调用方退回"超限直接拒绝"老路径。
pub fn deliver_split_streaming(
    path: &std::path::Path,
    budget: u64,
    mut upload_piece: impl FnMut(&str, &[u8], usize, usize) -> Result<(), String>,
) -> Result<Option<StreamSplitOutcome>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开母版库文件失败: {e}"))?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| format!("解 EPUB(非 zip?): {e}"))?;

    // 名字 → 真实解压大小（zip 目录直读，逐条 by_index 只取 size()，不解压 data）。
    let mut sizes: HashMap<String, u64> = HashMap::with_capacity(zip.len());
    // 阶段一：非图片条目整份读；图片条目占位（真实字节留到阶段二按需读）。
    let mut entries: Vec<Entry> = Vec::with_capacity(zip.len());
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| format!("读 EPUB 条目 {i}: {e}"))?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        sizes.insert(name.clone(), f.size());
        let data = if crate::imgopt::is_downscalable(&name) {
            Vec::new()
        } else {
            let mut d = Vec::with_capacity(f.size() as usize);
            f.read_to_end(&mut d).map_err(|e| e.to_string())?;
            d
        };
        entries.push(Entry { name, data });
    }
    drop(zip); // 阶段一读完关掉，阶段二按需重开——避免整个函数生命周期内都占着文件句柄/内部缓冲。

    if !crate::comic_detect::is_comic(&entries) {
        return Ok(None); // 不是漫画，这条路径不适用，调用方退回原来的整本流程
    }
    let Some(opf) = parse_opf(&entries) else { return Err("解不出 OPF/spine，没法按结构拆".into()) };
    let size_of = |e: &Entry| -> u64 {
        if e.data.is_empty() { sizes.get(&e.name).copied().unwrap_or(0) } else { e.data.len() as u64 }
    };
    let Some(pieces) = plan_pieces_sized(&entries, &opf, budget, &size_of)? else { return Ok(None) };

    // 阶段二：重开源文件（阶段一那个 zip 已经 drop 掉），逐份按需读图片、组包、上传、丢。
    let file2 = std::fs::File::open(path).map_err(|e| format!("重开母版库文件失败: {e}"))?;
    let mut zip2 = zip::ZipArchive::new(std::io::BufReader::new(file2)).map_err(|e| e.to_string())?;

    let total_fitting = pieces.iter().filter(|p| p.fits).count();
    let mut idx = 0usize;
    let mut delivered = Vec::new();
    let mut failed = Vec::new();
    for piece in &pieces {
        if !piece.fits {
            failed.push(format!("{}（拆到底仍超限，未投）", piece.title));
            continue;
        }
        idx += 1;
        // 只克隆这一份用得到的骨架（entries 里 html/opf/ncx 已经是真实字节，图片仍是占位，克隆便宜）。
        let mut piece_entries = entries.clone();
        for p in &opf.spine[piece.start..piece.end] {
            if !is_html(p) {
                continue;
            }
            let Some(html_entry) = entries.iter().find(|e| &e.name == p) else { continue };
            let Ok(html) = std::str::from_utf8(&html_entry.data) else { continue };
            for img in imgs_referenced(html, dir_of(p)) {
                let Some(pe) = piece_entries.iter_mut().find(|e| e.name == img) else { continue };
                if !pe.data.is_empty() {
                    continue; // 已经读过（同一张图被这一份里多页共用）
                }
                if let Ok(mut f) = zip2.by_name(&img) {
                    let mut buf = Vec::with_capacity(f.size() as usize);
                    if f.read_to_end(&mut buf).is_ok() {
                        pe.data = buf;
                    }
                }
            }
        }
        match build_piece(&piece_entries, piece.start, piece.end, &piece.title, &piece.title) {
            Ok(bytes) => {
                let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("book");
                let piece_name = format!("{stem} - {}.epub", piece.title);
                match upload_piece(&piece_name, &bytes, idx, total_fitting) {
                    Ok(()) => delivered.push(piece.title.clone()),
                    Err(e) => failed.push(format!("{}（上传失败：{e}）", piece.title)),
                }
            }
            Err(e) => failed.push(format!("{}（组包失败：{e}）", piece.title)),
        }
        // piece_entries/bytes 出循环体作用域即释放——下一份开始前，这一份占的内存已经收回。
    }
    Ok(Some(StreamSplitOutcome { delivered, failed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(name: &str, data: &[u8]) -> Entry {
        Entry { name: name.into(), data: data.to_vec() }
    }
    fn html_e(name: &str, img_rel: &str) -> Entry {
        e(name, format!(r#"<html><body><img src="{img_rel}"/></body></html>"#).as_bytes())
    }

    /// 造一本 N 卷合集漫画：每卷若干页，NCX 标好每卷起始页，跟真机《火影忍者》结构一致
    /// （每页一个 `text/pNNNN.html` + 一张 `images/NNNN.jpg`）。
    fn make_multivol(pages_per_vol: &[usize], page_bytes: usize) -> Vec<Entry> {
        let mut entries = Vec::new();
        let mut manifest = String::new();
        let mut spine = String::new();
        let mut navpoints = String::new();
        let mut page_no = 0usize;
        for (vi, &n) in pages_per_vol.iter().enumerate() {
            let vol_start_page = page_no;
            for _ in 0..n {
                entries.push(html_e(&format!("text/p{page_no:04}.html"), &format!("../images/{page_no:04}.jpg")));
                entries.push(e(&format!("images/{page_no:04}.jpg"), &vec![7u8; page_bytes]));
                manifest += &format!(
                    r#"<item id="h{page_no}" href="text/p{page_no:04}.html" media-type="application/xhtml+xml"/><item id="i{page_no}" href="images/{page_no:04}.jpg" media-type="image/jpeg"/>"#
                );
                spine += &format!(r#"<itemref idref="h{page_no}"/>"#);
                page_no += 1;
            }
            navpoints += &format!(
                r#"<navPoint id="nv{vi}"><navLabel><text>卷{vi}</text></navLabel><content src="text/p{vol_start_page:04}.html"/></navPoint>"#
            );
        }
        entries.push(e(
            "content.opf",
            format!(r#"<package version="3.0"><metadata><dc:title>t</dc:title></metadata><manifest>{manifest}<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx">{spine}</spine></package>"#).as_bytes(),
        ));
        entries.push(e("toc.ncx", format!(r#"<ncx><navMap>{navpoints}</navMap></ncx>"#).as_bytes()));
        entries.push(e(
            "META-INF/container.xml",
            br#"<container><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#,
        ));
        entries
    }

    /// 同 `make_multivol`，但每卷的 navPoint 内再嵌一层"每页一条"的子 navPoint（跟真机《镖人》
    /// 卷→章两层结构一致），用来验证"只按第一层切、不管更深层级嵌套导航点"这条规则。
    fn make_multivol_nested(pages_per_vol: &[usize], page_bytes: usize) -> Vec<Entry> {
        let mut entries = Vec::new();
        let mut manifest = String::new();
        let mut spine = String::new();
        let mut navpoints = String::new();
        let mut page_no = 0usize;
        for (vi, &n) in pages_per_vol.iter().enumerate() {
            let vol_start_page = page_no;
            let mut children = String::new();
            for pi in 0..n {
                entries.push(html_e(&format!("text/p{page_no:04}.html"), &format!("../images/{page_no:04}.jpg")));
                entries.push(e(&format!("images/{page_no:04}.jpg"), &vec![7u8; page_bytes]));
                manifest += &format!(
                    r#"<item id="h{page_no}" href="text/p{page_no:04}.html" media-type="application/xhtml+xml"/><item id="i{page_no}" href="images/{page_no:04}.jpg" media-type="image/jpeg"/>"#
                );
                spine += &format!(r#"<itemref idref="h{page_no}"/>"#);
                children += &format!(
                    r#"<navPoint id="nv{vi}c{pi}"><navLabel><text>卷{vi}章{pi}</text></navLabel><content src="text/p{page_no:04}.html"/></navPoint>"#
                );
                page_no += 1;
            }
            navpoints += &format!(
                r#"<navPoint id="nv{vi}"><navLabel><text>卷{vi}</text></navLabel><content src="text/p{vol_start_page:04}.html"/>{children}</navPoint>"#
            );
        }
        entries.push(e(
            "content.opf",
            format!(r#"<package version="3.0"><metadata><dc:title>t</dc:title></metadata><manifest>{manifest}<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/></manifest><spine toc="ncx">{spine}</spine></package>"#).as_bytes(),
        ));
        entries.push(e("toc.ncx", format!(r#"<ncx><navMap>{navpoints}</navMap></ncx>"#).as_bytes()));
        entries.push(e(
            "META-INF/container.xml",
            br#"<container><rootfiles><rootfile full-path="content.opf"/></rootfiles></container>"#,
        ));
        entries
    }

    #[test]
    fn whole_book_within_budget_returns_none() {
        let entries = make_multivol(&[2, 2, 2], 100);
        assert!(plan_splits(&entries, 10_000).unwrap().is_none());
    }

    #[test]
    fn splits_by_top_level_ncx_when_oversized() {
        // 每卷 3 页 * 1000 字节 ≈ 3000+，整书 3 卷 ≈ 9000+，预算给 4000 只够单卷
        let entries = make_multivol(&[3, 3, 3], 1000);
        let pieces = plan_splits(&entries, 4000).unwrap().expect("应该要拆");
        assert_eq!(pieces.len(), 3, "三卷应该各自成一份");
        assert!(pieces.iter().all(|p| p.fits), "每卷单独都在预算内");
        assert_eq!(pieces[0].title, "卷0");
        assert_eq!(pieces[1].start, 3, "第二卷从第 3 页开始");
        assert_eq!(pieces[2].end, 9, "第三卷（也是最后一卷）end 钳到 spine 总长");
    }

    #[test]
    fn oversized_top_level_piece_gets_sub_split_by_page() {
        // 真机回归（2026-09-19，《镖人》第8卷 ~96MB 撞上 xochitl 自己 ~100MB 的硬上限，curl 直传
        // 复现 HTTP 413）：某一卷本身切完还超预算，不再是直接放弃——退化成按页贪心再切一层（复用
        // `fixed_page_chunks_range_sized`），标题带 `（N/M）`后缀区分。
        let entries = make_multivol(&[1, 5, 1], 2000);
        let pieces = plan_splits(&entries, 3000).unwrap().expect("应该要拆");
        assert_eq!(pieces.len(), 7, "卷0(1) + 卷1 拆成 5 份 + 卷2(1) = 7");
        assert!(pieces.iter().all(|p| p.fits), "拆开后每份都应该在预算内");
        assert_eq!(pieces[0].title, "卷0");
        assert_eq!(pieces[1].title, "卷1（1/5）");
        assert_eq!(pieces[5].title, "卷1（5/5）");
        assert_eq!(pieces[6].title, "卷2");
        // 覆盖完整、首尾相接
        assert_eq!(pieces[0].start, 0);
        assert_eq!(pieces.last().unwrap().end, 7);
        for w in pieces.windows(2) {
            assert_eq!(w[0].end, w[1].start, "相邻两份首尾必须相接");
        }
    }

    #[test]
    fn oversized_top_level_piece_with_single_giant_page_stays_unfit() {
        // 单页本身已经超预算（如一张巨图）——切不动（sub-split 切出 1 份等于没切），原样保留
        // 原标题、如实标 fits=false，不产生 (N/M) 子份。
        let entries = make_multivol(&[1, 1, 1], 2000);
        let pieces = plan_splits(&entries, 1500).unwrap().expect("应该要拆");
        assert_eq!(pieces.len(), 3, "每卷都只有一页，切不动，维持三份");
        assert!(pieces.iter().all(|p| !p.fits));
        assert_eq!(pieces[1].title, "卷1", "切不动时标题不该被加 (N/M) 后缀");
    }

    #[test]
    fn nested_ncx_only_splits_by_top_level_no_panic() {
        // 真机回归（2026-09-19，《镖人》11 卷 173 章）：NCX 带两层嵌套（卷→章）时，旧的递归拆分
        // 实现会在算边界时 panic（range end index 是 usize::MAX，book-serve 进程被摔炸）。简化成
        // 只切第一层后，带不带更深层级的嵌套导航点都不影响结果——多出来的子层级条目原样忽略。
        let entries = make_multivol_nested(&[2, 2], 1000);
        let pieces = plan_splits(&entries, 4000).unwrap().expect("应该要拆");
        assert_eq!(pieces.len(), 2, "只按顶层（卷）切，忽略卷内的章级嵌套导航点");
        assert_eq!(pieces[0].title, "卷0");
        assert_eq!(pieces[0].start, 0);
        assert_eq!(pieces[0].end, 2, "第一卷 end = 第二卷 start");
        assert_eq!(pieces[1].end, 4, "最后一卷 end 钳到 spine 总长");
        assert!(pieces.iter().all(|p| p.fits));
    }

    #[test]
    fn no_ncx_falls_back_to_fixed_page_chunks() {
        // 书压根没有 toc.ncx——不再是"整本拒绝"，退化成按页数贪心切。
        let entries = make_multivol(&[6], 1000); // 单卷 6 页，制造一份没有分卷意义的扁平书
        let mut v: Vec<Entry> = entries.into_iter().filter(|e| e.name != "toc.ncx").collect();
        // 去掉 manifest 里指向 toc.ncx 的 item，模拟"这本书真的没有目录"（不是目录文件缺失但manifest还残留引用的半吊子状态）。
        if let Some(opf) = v.iter_mut().find(|e| e.name == "content.opf") {
            let t = String::from_utf8_lossy(&opf.data).replace(r#"<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#, "");
            opf.data = t.into_bytes();
        }
        let pieces = plan_splits(&v, 2500).unwrap().expect("超预算应该要拆");
        assert!(pieces.len() >= 2, "6 页 * 1000B 超过 2500 预算应该切成至少两份: {}", pieces.len());
        assert!(pieces.iter().all(|p| p.fits), "每份都应该在预算内");
        // 覆盖完整：首尾相接，没有漏页也没有重叠
        assert_eq!(pieces[0].start, 0);
        assert_eq!(pieces.last().unwrap().end, 6);
        for w in pieces.windows(2) {
            assert_eq!(w[0].end, w[1].start, "相邻两份首尾必须相接");
        }
    }

    #[test]
    fn single_oversized_page_marked_not_fits_in_fallback() {
        // 单页本身就超预算（如一张巨图）——没法再细分，该份单独成篇、如实标 fits=false。
        let entries = make_multivol(&[3], 5000);
        let mut v: Vec<Entry> = entries.into_iter().filter(|e| e.name != "toc.ncx").collect();
        if let Some(opf) = v.iter_mut().find(|e| e.name == "content.opf") {
            let t = String::from_utf8_lossy(&opf.data).replace(r#"<item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>"#, "");
            opf.data = t.into_bytes();
        }
        let pieces = plan_splits(&v, 2000).unwrap().expect("应该要拆");
        assert_eq!(pieces.len(), 3, "每页单独超预算，一页一份");
        assert!(pieces.iter().all(|p| !p.fits), "每份都仍超预算，如实标出");
    }

    /// 手动验证用：拿真机真实下载的《火影忍者》测——不进 CI（本地文件路径依赖），
    /// `cargo test --lib comic_split::tests::against_real_naruto_book -- --ignored --nocapture` 手跑。
    #[test]
    #[ignore]
    fn against_real_naruto_book() {
        let path = "/tmp/naruto-source.epub";
        let bytes = std::fs::read(path).expect("先手动 scp 真机文件到这个路径");
        let entries = crate::check::read_entries(&bytes).unwrap();
        let budget = 150 * 1024 * 1024; // 跟 book-serve 缺省 native_upload_limit_mb 一致
        let pieces = plan_splits(&entries, budget).unwrap().expect("281MB 应该超预算触发拆分");
        println!("拆出 {} 份:", pieces.len());
        let opf = parse_opf(&entries).unwrap();
        for p in &pieces {
            let sz = range_bytes_sized(&entries, &opf.spine, p.start, p.end, &|e| e.data.len() as u64);
            println!("  {} [{},{}) {} MB fits={}", p.title, p.start, p.end, sz / 1024 / 1024, p.fits);
        }
        assert!(pieces.iter().all(|p| p.fits), "真机这本书每卷体积应该都在 150MB 预算内");
        // 实际组一份出来，确认真的是合法 EPUB——挑真有内容的一卷（卷八），不是只有一页的封面份。
        let vol8 = pieces.iter().find(|p| p.title.contains("卷八")).unwrap();
        let out = build_piece(&entries, vol8.start, vol8.end, &vol8.title, "naruto-vol8").unwrap();
        std::fs::write("/tmp/naruto-vol8-test.epub", &out).unwrap();
        println!("组出{}（{} 页）{} 字节，写到 /tmp/naruto-vol8-test.epub 供人工核验", vol8.title, vol8.end - vol8.start, out.len());
    }

    #[test]
    fn build_piece_produces_valid_epub_with_remapped_images() {
        let entries = make_multivol(&[2], 50);
        let bytes = build_piece(&entries, 0, 2, "卷0", "t0").unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert!(zip.by_name("OEBPS/images/0001.jpg").is_ok(), "第一页图片应该被收进新资源目录");
        assert!(zip.by_name("OEBPS/images/0002.jpg").is_ok());
        use std::io::Read;
        let mut c1 = String::new();
        zip.by_name("OEBPS/chap_0001.xhtml").unwrap().read_to_string(&mut c1).unwrap();
        assert!(c1.contains(r#"src="images/0001.jpg""#), "{c1}");
    }

    #[test]
    fn build_piece_injects_external_comic_css_link_and_manifest_entry() {
        // 真机拿拆出来的一卷在原生阅读器打开、量 <uuid>.pdf 坐实：assemble() 吐出来的页面没有清零
        // 默认文档边距，图片也没有撑满容器样式，导致页面四周（尤其底部）空出一大圈——跟 KOReader
        // 贴边满屏的效果不一致。补一段外链 comic.css 治本，这条测试钉住"外链+manifest 都要有"
        // （xochitl 只认外链 css，漏了 manifest 声明也可能不生效，两处都不能省，见书架白皮书 §03y）。
        let entries = make_multivol(&[2], 50);
        let bytes = build_piece(&entries, 0, 2, "卷0", "t0").unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        use std::io::Read;
        let mut css = String::new();
        zip.by_name("OEBPS/comic.css").expect("应该有外链 comic.css").read_to_string(&mut css).unwrap();
        assert!(css.contains("body{margin:0;padding:0;}") && css.contains("img{width:100%;height:auto;}"), "{css}");
        let mut c1 = String::new();
        zip.by_name("OEBPS/chap_0001.xhtml").unwrap().read_to_string(&mut c1).unwrap();
        assert!(c1.contains(r#"<link rel="stylesheet" type="text/css" href="comic.css"/>"#), "章节头部应该链外链 css: {c1}");
        let mut opf = String::new();
        zip.by_name("OEBPS/content.opf").unwrap().read_to_string(&mut opf).unwrap();
        assert!(opf.contains(r#"<item id="comic-css" href="comic.css" media-type="text/css"/>"#), "manifest 也要声明这个资源: {opf}");
    }

    fn write_epub_zip(entries: &[Entry], path: &std::path::Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut z = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default();
        for e in entries {
            z.start_file(&e.name, opts).unwrap();
            z.write_all(&e.data).unwrap();
        }
        z.finish().unwrap();
    }

    #[test]
    fn streaming_matches_top_level_split_and_uploads_valid_pieces() {
        // 真机回归（2026-09-19，《镖人》OOM 风险）：`deliver_split_streaming` 不整本解压进内存，
        // 结果应该跟内存版 `plan_splits`（`splits_by_top_level_ncx_when_oversized`）一致——三卷各
        // 成一份，图片是真实字节（不是阶段一的空占位）。
        // 每卷 8 页（三卷共 24 张图，过 comic_detect::MIN_IMAGES=20 门槛，才会判定成漫画）。
        let entries = make_multivol(&[8, 8, 8], 1000);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_epub_zip(&entries, &path);

        let mut uploaded: Vec<(String, Vec<u8>)> = Vec::new();
        let mut progresses: Vec<(usize, usize)> = Vec::new();
        let outcome = deliver_split_streaming(&path, 9000, |name, bytes, idx, total| {
            uploaded.push((name.to_string(), bytes.to_vec()));
            progresses.push((idx, total));
            Ok(())
        })
        .unwrap()
        .expect("超预算应该要拆");

        assert_eq!(outcome.delivered.len(), 3, "三卷各自成一份");
        assert!(outcome.failed.is_empty(), "{:?}", outcome.failed);
        assert_eq!(uploaded.len(), 3);
        assert_eq!(progresses, vec![(1, 3), (2, 3), (3, 3)], "进度按上传顺序 1-based 递增，分母是预算内总份数");
        for (name, bytes) in &uploaded {
            let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
            let mut img = Vec::new();
            zip.by_name("OEBPS/images/0001.jpg").expect("每份都应该至少有一张图").read_to_end(&mut img).unwrap();
            assert_eq!(img, vec![7u8; 1000], "图片字节应该是真实内容，不是阶段一的空占位: {name}");
        }
    }

    #[test]
    fn streaming_within_budget_returns_none() {
        let entries = make_multivol(&[11, 11], 100); // 22 张图，过 MIN_IMAGES 门槛
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_epub_zip(&entries, &path);
        let outcome = deliver_split_streaming(&path, 10_000, |_, _, _, _| Ok(())).unwrap();
        assert!(outcome.is_none(), "预算够就不该拆，调用方按整本投处理");
    }

    #[test]
    fn streaming_handles_nested_ncx_without_panic() {
        // 对应 `nested_ncx_only_splits_by_top_level_no_panic`，流式路径走的是同一份
        // `plan_pieces_sized`，同样不该被两层嵌套 NCX 带崩。
        let entries = make_multivol_nested(&[11, 11], 1000); // 22 张图，过 MIN_IMAGES 门槛
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("book.epub");
        write_epub_zip(&entries, &path);
        let outcome = deliver_split_streaming(&path, 15_000, |_, _, _, _| Ok(())).unwrap().expect("应该要拆");
        assert_eq!(outcome.delivered.len(), 2);
        assert!(outcome.failed.is_empty());
    }
}
