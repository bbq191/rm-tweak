//! 流式优化：路径进路径出，峰值内存不随书体积线性涨（构建器 `StreamingOptimize` + 旧签名薄封装）。
use super::*;

/// [`optimize_epub_with`] 的流式版：路径进、路径出，峰值内存不随书体积线性涨。**真机 2026-09-19
/// 坐实**——552MB《镖人》全集走内存版优化，`VmRSS` 几十秒内冲到 1.4GB+，真机系统可用内存探底到
/// ~25MB（book-serve `systemd` 的 `MemoryMax=192M` 没生效——这台设备的 systemd 压根没把 memory
/// 控制器代理进 `system.slice` 子树，这条"软限"从来没真正兜住过），逼近全系统级 OOM（内核会不分
/// 青红皂白挑内存最大的进程杀，可能殃及 xochitl 本体），手动重启服务才止血。
///
/// 内存的大头几乎全是图片字节——漫画书尤其如此，正文 html/css/opf/ncx 这些结构信息本来就很小。
/// 两阶段拆开：**阶段一**只把非图片条目（html/css/opf/ncx/字体等）整份读进内存（本来就小，全书
/// 一起拿着无所谓），图片条目只记名字、字节留空占位——后续 wash 层（空页清理/自动目录/目录分部
/// 重建/dtb:uid 同步）跟漫画识别只看 html 文字内容和 `<img>` 标签*引用*，从来不需要图片真实字节，
/// 占位不影响任何判断。**阶段二**（最终写出）按 `entries` 顺序重新遍历：非图片条目直接用阶段一
/// 已经处理好的字节；图片条目才从源文件按需流式读回这一张的真实字节、处理、立刻写进目标文件，
/// 读完这张就丢，从不会有第二张同时留在内存里。输出直接流式写文件（`ZipWriter` 包 `BufWriter<File>`），
/// 不再攒一份完整产物在内存里。峰值内存量级降到"一张图 + 全书文字部分"，不随书变大线性涨。
///
/// 跟 [`optimize_epub_with`] 共用 [`first_pass_html`]/[`transform_html_chapter`]/
/// [`transform_image_bytes`] 这几个抽出来的变换函数——两条路径的业务逻辑是同一份代码，不会因为
/// "整本内存版"跟"流式版"分叉走样；`optimize_epub_with` 继续保留给测试/CLI 小书场景用（签名不变，
/// 100+ 既有单测零改动），book-serve 真机场景（真书可能上百 MB）改走这条流式路径。
///
/// `on_progress(done, total)`（2026-09-19 补，给调用方画进度条用）：阶段二每写完一个条目回调一次，
/// `total`＝这本书要写出的条目总数（`entries.len()`，含 mimetype 之外的所有文本/图片条目，不含
/// 末尾的 marker）。只在阶段二回调——阶段一（读入+清洗）对文字书通常是毫秒级，真正拖时间的是阶段
/// 二逐张图片的重编码，回调粒度对齐"真正在做的工作"，跟 `comic_split::deliver_split_streaming` 的
/// `upload_piece` 进度粒度同一个道理。**这个回调纯粹是可观测性，不改变内存峰值**——阶段二本来就是
/// 逐条目处理+立刻写文件+立刻丢，回调只是在这个已有的循环里多做一次通知，不持有任何额外数据。
pub fn optimize_epub_file_streaming(input_path: &std::path::Path, output_path: &std::path::Path, opts: &OptimizeOpts, on_progress: impl FnMut(usize, usize)) -> Result<Report, String> {
    optimize_epub_file_streaming_titled(input_path, output_path, opts, None, on_progress)
}

/// 同 [`optimize_epub_file_streaming`]，`title=Some` 时把 OPF 的 `<dc:title>` 改成这个书名——设备上的显示名取
/// EPUB 自己的 `dc:title`，母版库按 `书名 - N卷` 规范命名后，这里让设备显示名与文件名一致（乱马等下载书
/// 的原 `dc:title` 甚至是 "Unknown"）。
pub fn optimize_epub_file_streaming_titled(input_path: &std::path::Path, output_path: &std::path::Path, opts: &OptimizeOpts, title: Option<&str>, on_progress: impl FnMut(usize, usize)) -> Result<Report, String> {
    optimize_epub_file_streaming_ctl(input_path, output_path, opts, title, &|| false, on_progress)
}

/// 用户主动取消时返回的错误文案（调用方按它区分"取消"和"失败"，见 `book-serve` 的 `CANCELLED`）。
pub const CANCELLED_MSG: &str = "已取消";

/// 同 [`optimize_epub_file_streaming_titled`]，多一个 `cancel` 回调：每处理完一个条目检查一次，返回 `true` 就
/// 立刻停手、返回 `Err(`[`CANCELLED_MSG`]`)`（图片 worker 随之退出，调用方负责清掉半成品输出文件）。
/// 用户 2026-09-20 反馈"不能停止某个执行中的优化"——优化在设备上要几分钟，必须能中途停。
pub fn optimize_epub_file_streaming_ctl(input_path: &std::path::Path, output_path: &std::path::Path, opts: &OptimizeOpts, title: Option<&str>, cancel: &dyn Fn() -> bool, on_progress: impl FnMut(usize, usize)) -> Result<Report, String> {
    StreamingOptimize::new(input_path, output_path, opts).title(title).cancel(cancel).run(on_progress)
}

/// 流式优化任务的构建器（2026-09-20 审计：`optimize_epub_file_streaming` → `_titled` → `_ctl` 是靠加参数堆出来的
/// 重载链，下一个需求（如新增回调）又要多一个参数）。必填项走 [`Self::new`]，可选项（改书名、取消检查）链式加，
/// 最后 [`Self::run`]（带进度回调）。旧的三个函数保留为薄封装，签名与行为不变。
pub struct StreamingOptimize<'a> {
    input: &'a std::path::Path,
    output: &'a std::path::Path,
    opts: &'a OptimizeOpts,
    title: Option<&'a str>,
    cancel: Option<&'a dyn Fn() -> bool>,
}

impl<'a> StreamingOptimize<'a> {
    pub fn new(input: &'a std::path::Path, output: &'a std::path::Path, opts: &'a OptimizeOpts) -> StreamingOptimize<'a> {
        StreamingOptimize { input, output, opts, title: None, cancel: None }
    }

    /// `Some` 时把 OPF 的 `<dc:title>` 改成这个书名（见 [`optimize_epub_file_streaming_titled`]）。
    pub fn title(mut self, title: Option<&'a str>) -> Self {
        self.title = title;
        self
    }

    /// 每处理完一个条目检查一次，返回 `true` 就立刻停手、返回 `Err(`[`CANCELLED_MSG`]`)`（见 [`optimize_epub_file_streaming_ctl`]）。
    pub fn cancel(mut self, cancel: &'a dyn Fn() -> bool) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// 执行。`on_progress(done, total)` 语义见 [`optimize_epub_file_streaming`]。
    pub fn run(self, mut on_progress: impl FnMut(usize, usize)) -> Result<Report, String> {
        let StreamingOptimize { input: input_path, output: output_path, opts, title, cancel } = self;
        let never = || false;
        let cancel: &dyn Fn() -> bool = cancel.unwrap_or(&never);
        let in_file = std::fs::File::open(input_path).map_err(|e| format!("打开输入失败: {e}"))?;
        let bytes_before = in_file.metadata().map(|m| m.len() as usize).unwrap_or(0);
        let mut archive = ZipArchive::new(std::io::BufReader::new(in_file)).map_err(|e| format!("解 EPUB(非 zip?): {e}"))?;

        // 阶段一：非图片条目整份读；图片条目占位（真实字节留到阶段二按需流式读）；之后同内存版（`prepare_entries`）。
        let raw = crate::epubzip::read_skeleton(&mut archive)?.entries;
        let Prepared { entries, aside_index, is_comic_book, opf_name, mut rep } = prepare_entries(raw, opts, bytes_before, title)?;
        let comic_frame = opts.comic_frame;

        // 阶段二：流式写出。非图片条目用阶段一已处理好的字节；图片条目现在才从源文件按需读回真实
        // 字节，处理完立刻写文件、立刻丢——峰值只有"当前这一张"，不会随全书图片数量线性涨。
        let out_file = std::fs::File::create(output_path).map_err(|e| format!("建输出文件失败: {e}"))?;
        let mut zw = ZipWriter::new(std::io::BufWriter::new(out_file));
        let (stored, deflated) = (crate::epubzip::stored(), crate::epubzip::deflated());
        // 图片本身已是 JPEG/PNG：deflate 只能再榨一点（实测乱马 6%），用最快档（级别 1）拿大部分收益、少花 CPU。
        let deflated_fast = deflated.compression_level(Some(1));
        let mut xf = EntryXform::new(&aside_index, opts.footnote, title, opts.page_direction, opf_name.as_deref());
        let total_entries = entries.len();
        // 图片并行处理（见 `imgpool`）：主线程按条目顺序读原图字节、提交给 worker、按原顺序取回结果写 zip；
        // 提前提交 `lookahead` 张（读原图字节几乎不花时间，处理才慢），处理与写盘/读盘重叠。结果与逐张顺序处理逐字节相同。
        let workers = crate::imgpool::worker_count();
        let lookahead = workers + 2;
        let image_positions: Vec<usize> = entries.iter().enumerate().filter(|(_, (n, _, ish))| !*ish && crate::imgopt::is_downscalable(n)).map(|(i, _)| i).collect();
        std::thread::scope(|scope| -> Result<(), String> {
            struct ImgJob {
                bytes: Vec<u8>,
                reply: std::sync::mpsc::Sender<Vec<u8>>,
            }
            let (job_tx, job_rx) = std::sync::mpsc::sync_channel::<ImgJob>(lookahead);
            let job_rx = std::sync::Arc::new(std::sync::Mutex::new(job_rx));
            let budget = std::sync::Arc::new(crate::imgpool::PixelBudget::new(crate::imgpool::PIXEL_BUDGET));
            for _ in 0..workers {
                let (rx, budget) = (job_rx.clone(), budget.clone());
                scope.spawn(move || loop {
                    let job = { rx.lock().unwrap_or_else(|e| e.into_inner()).recv() };
                    let Ok(job) = job else { break };
                    // 读图片头也是在解析外部输入：兜住 panic（按读不出尺寸算），不让一张坏图摔掉 worker——worker 全摔掉时
                    // 主线程要么拿到"线程异常退出"，要么（队列已满时）`send` 永远等不到人收。
                    let px = std::panic::catch_unwind(|| crate::imgopt::pixel_count(&job.bytes)).unwrap_or(1_000_000);
                    let _permit = budget.acquire(px);
                    let out = transform_image_bytes(&job.bytes, is_comic_book, comic_frame).unwrap_or(job.bytes);
                    let _ = job.reply.send(out);
                });
            }
            // 接收端只由 worker 持有：万一 worker 全部退出，`job_tx.send` 立刻报错而不是在满队列上永远阻塞。
            drop(job_rx);
            let mut pending: std::collections::VecDeque<std::sync::mpsc::Receiver<Vec<u8>>> = std::collections::VecDeque::new();
            let mut next_submit = 0usize;
            let mut consumed = 0usize; // 已取回的图片数：第 consumed 张图片对应 image_positions[consumed]
            for (i, (name, data, ish)) in entries.iter().enumerate() {
                if cancel() {
                    return Err(CANCELLED_MSG.to_string()); // drop(job_tx) 随作用域结束，worker 退出
                }
                // 补满提前量：读原图字节（archive 支持随时按名字重新 seek 读，跟阶段一是同一个源文件）并提交。
                while pending.len() < lookahead && next_submit < image_positions.len() {
                    let img_name = &entries[image_positions[next_submit]].0;
                    let real_bytes = crate::epubzip::read_by_name(&mut archive, img_name).map_err(|e| format!("重读图片 {img_name} 失败: {e}"))?;
                    let (tx, rx) = std::sync::mpsc::channel();
                    job_tx.send(ImgJob { bytes: real_bytes, reply: tx }).map_err(|_| "图片处理线程已退出".to_string())?;
                    pending.push_back(rx);
                    next_submit += 1;
                }
                let is_image = image_positions.get(consumed) == Some(&i);
                let final_data: std::borrow::Cow<[u8]> = match xf.transform_text(name, data, *ish) {
                    Some(t) => t,
                    None if is_image => {
                        let rx = pending.pop_front().ok_or("图片队列意外为空")?;
                        consumed += 1;
                        std::borrow::Cow::Owned(rx.recv().map_err(|_| format!("图片处理线程异常退出（{name}）"))?)
                    }
                    None => std::borrow::Cow::Borrowed(data.as_slice()),
                };
                // mimetype 与图片（JPEG/PNG 本身已压缩，再 deflate 几乎没收益、白花 CPU）用 Stored；其余 deflate。
                let file_opts = if name == "mimetype" { stored } else if is_image { deflated_fast } else { deflated };
                crate::epubzip::put_entry(&mut zw, name, file_opts, &final_data)?;
                on_progress(i + 1, total_entries);
            }
            drop(job_tx); // 关闭队列，worker 退出，scope 才能 join
            Ok(())
        })?;
        write_tail(&mut zw, &xf.fetched_imgs, opts.wash.is_some())?;
        // `finish()` 只保证写完中央目录，底下 `BufWriter` 自己的缓冲区不一定落盘——显式 flush，
        // 不指望 Drop 的静默兜底（出错会被吞掉）。
        let mut out = zw.finish().map_err(|e| e.to_string())?;
        out.flush().map_err(|e| e.to_string())?;
        // 产物文件大小（跟内存版 out_buf.len() 同语义——压缩后的 zip 体积），直接 stat 落盘文件，比
        // 流式写的时候自己攒一份计数更简单也更准确。
        rep.bytes_after = std::fs::metadata(output_path).map(|m| m.len() as usize).unwrap_or(0);
        Ok(rep)
    }
}
