"""`shelf push`：host 洗书 → 落**母版库**（中间层），去向由用户在网页选（xochitl / KOReader）。

统一后 push 不再有去向参数——一律落母版库（`/api/books/staging`）。host 是唯一能"入库时顺带优化"的源：
- 默认：有 Calibre → 洗书（EPUB 深洗 / 杂格式转 EPUB / PDF 结构化重排）→ 落母版库（产物带优化标记）。
- `--no-optimize`：不洗，原样传母版库（用户可在网页按需点优化）。
- `--no-calibre`：**只对本来就是 EPUB 的输入有意义**——跳过 `ebook-convert`（CSS 拍平/series 命名那部分），
  直接调跟网页「母版库→优化」按钮同一个函数的 `epub-optimize` 二进制（清洗+优化都做，不是"只优化不清洗"，
  对应网页「清洗＋优化」档；`--keep-spacing` 同样生效＝对应「清洗但保留段距」档）。不装 Calibre 也能用——
  两条路径唯一共同点只是都在编译产物 `shelf/target/release/epub-optimize`。非 EPUB 输入没法只靠这条路径
  转格式，加了 `--no-calibre` 也一律原样传（等同 `--no-optimize` 的效果，母版库里再按需转/优化）。
- `--to-pdf`：定稿成固定版式 PDF（手写批注用），落母版库。
- **漫画**（AZW3/MOBI/EPUB/PDF 里全是整页图，`comic.is_comic` 自动判，`--comic/--no-comic` 覆盖；CBZ 天然）：不走洗书路，
  出 **CBZ**（原图按页打包，跨页图自动拆分+白边裁切）进母版库，去向 KOReader 漫画模式。大部头**漫画默认不投原生**
  （用户 2026-09-05 定）：xochitl 没有固定页漫画体验，且整本几百 MB 撞 `/upload` 体积上限（《镖人》282MB EPUB /
  188MB PDF 都被 "multipart body is too large" 拒）——**但灰阶 CBZ 估算转 PDF 后还在设备原生上传上限内的小体积
  漫画，会顺带多出一份 PDF 一起落库**，母版库里就多一个「投入原生书库」的选项（不强制分卷，超限的照旧只有
  CBZ，`--no-comic-native` 关掉这条，2026-09-08）。
无 Calibre → 原样传母版库（设备端优化在网页母版库里点）。
规则与网页一致：**所有书只落母版库**，没有绕过母版库直投读器的选项（2026-09-05 用户定）。
"""
from __future__ import annotations

import time
from pathlib import Path

from .. import calibre_bridge as cb
from .. import comic, pdfsplit
from ..receipts import guard_file, upload_each

NAME = "push"
HELP = "投书到母版库（host 有 Calibre 先洗书；漫画自动转 CBZ 给 KOReader）；去向在网页选。难搞的书/PDF 重排用这条"

# host 能洗/转成 EPUB 的源格式（其余原样传母版库）。= Rust `rmsvc_core::formats::HOST_CONVERTIBLE_EXTS` ∪ {epub} − {txt}，
# 网页「格式」提示里"电脑可转"那一档就是它——改一处另一处同步（Python 不链接 Rust crate，只能镜像；
# `tests/test_push.py::test_wash_ext_matches_rust_host_convertible_exts` 跨语言正则核对，改漏了会报）。
# `.txt` 也是电脑可转，但先走 txt_to_epub.py 切章（Calibre 不认中文"第X章"），再进 wash——见 host_prepare。
WASH_EXT = {".epub", ".azw3", ".mobi", ".azw", ".prc", ".fb2"}

WAIT_DEFAULT_SECS = 600
WAIT_PROBE_SECS = 5
UNREACHABLE_HINT = "设备不可达（离 USB 后几秒就自动休眠、关 WiFi）：点亮屏幕或接上 USB 再推，或加 --wait 让 push 等它醒"


def ensure_reachable(transport, wait: int | None, sleep=time.sleep, clock=time.monotonic) -> bool:
    """探活（`GET /health`，不用密码）。不可达：无 `--wait` → 提示后 False；有 → 每 WAIT_PROBE_SECS 秒探一次直到可达/超时/Ctrl-C。
    整批处理前只探一次，避免逐本各报一次"连不上"。2026-09-14 起放在处理任何一本书之前（原来放
    在第一本洗完之后，好处是等设备醒的时间能顺带处理第一本、不白占；这次为了让 `_staging_snapshot`
    的 source 判重能在处理前生效，改成先探活——见 run() 头部注释里记的这个取舍）。"""
    probe = getattr(transport, "reachable", None)
    if probe is None or probe():
        return True
    if wait is None:
        print(f"✗ {UNREACHABLE_HINT}")
        return False
    print(f"… 设备不可达（可能已休眠）：点亮屏幕或接上 USB，每 {WAIT_PROBE_SECS} 秒探一次，最多等 {wait} 秒（Ctrl-C 放弃）", flush=True)
    t0 = clock()
    last_note = t0
    try:
        while clock() - t0 < wait:
            sleep(WAIT_PROBE_SECS)
            if probe():
                print(f"… 设备醒了（等了 {int(clock() - t0)} 秒），继续上传", flush=True)
                return True
            if clock() - last_note >= 30:
                last_note = clock()
                print(f"… 仍在等（还剩 {int(wait - (clock() - t0))} 秒）", flush=True)
    except KeyboardInterrupt:
        print("… 放弃等待")
        return False
    print(f"✗ 等了 {wait} 秒设备还没醒，未上传")
    return False


def add_args(p):
    p.add_argument("files", nargs="+", type=Path)
    p.add_argument("--to-pdf", action="store_true", help="定稿成固定版式 PDF（手写批注用）；默认洗成流式 EPUB")
    p.add_argument("--no-optimize", action="store_true", help="不洗，原样传母版库（网页里可再点优化）")
    p.add_argument("--no-calibre", action="store_true", help="EPUB 只跑 epub-optimize（跳过 ebook-convert，不用装 Calibre）；非 EPUB 原样传")
    p.add_argument("--keep-spacing", action="store_true", help="洗书时保留原书段间距（诗集 / 剧本；对应网页「清洗但保留段距」档位；--no-calibre 下同样生效）")
    p.add_argument("--no-reflow", action="store_true", help="PDF 不重排（原样传）")
    g = p.add_mutually_exclusive_group()
    g.add_argument("--comic", action="store_true", help="强制按漫画处理（出 CBZ，只加入 KOReader；漫画不投原生）")
    g.add_argument("--no-comic", action="store_true", help="不判漫画，按文字书洗")
    g2 = p.add_mutually_exclusive_group()
    g2.add_argument("--eink-gray", dest="eink_gray", action="store_true", default=True, help="漫画省刷新档（缺省开）：CBZ 逐页缩屏盒，黑白页转 16 灰抖动 4-bit PNG（轻波形、翻页明显少闪），彩页保色")
    g2.add_argument("--no-eink-gray", dest="eink_gray", action="store_false", help="漫画不转 16 灰，原图 CBZ")
    p.add_argument("--manga-ltr", action="store_true", help="漫画跨页拆分按从左往右排（缺省从右往左，东亚漫画传统）")
    g3 = p.add_mutually_exclusive_group()
    g3.add_argument("--comic-native", dest="comic_native", action="store_true", default=True, help="漫画灰阶 CBZ 估算转 PDF 后在原生上传上限内就顺带出份 PDF，多一个「投入原生书库」选项（缺省开）")
    g3.add_argument("--no-comic-native", dest="comic_native", action="store_false", help="漫画一律只出 CBZ，不生成投原生用的 PDF")
    p.add_argument("--no-split", action="store_true", help="大 PDF 不分卷")
    p.add_argument("--require-toc", action="store_true", help="洗书体检要求有目录")
    p.add_argument("--skip-check", action="store_true", help="跳过 check_output.py 体检（缺省不过不推）")
    p.add_argument("--dry-run", "-n", action="store_true", help="只打印会怎么做，不动文件、不上传")
    p.add_argument("--wait", nargs="?", const=WAIT_DEFAULT_SECS, type=int, metavar="秒", help=f"设备不可达（离 USB 自动休眠关 WiFi）时每 {WAIT_PROBE_SECS} 秒探一次、等它醒再传（缺省最多 {WAIT_DEFAULT_SECS} 秒）；不加则直接报错不传")


def _gate(out: Path, args) -> None:
    """原生高质量门：host 洗书产物必过 check_output.py（TOC/内链/字体子集/双 id/屏上字号列宽），不过不推。"""
    if getattr(args, "skip_check", False):
        print("  （--skip-check：跳过体检）")
        return
    ok, rep = cb.check(out, require_toc=args.require_toc)
    print(rep)
    if not ok:
        raise cb.CalibreError("体检未通过，未推送（--skip-check 强推）")


def _mb(n: float) -> str:
    return f"{n / 2**20:.1f}MB"


# 灰阶 CBZ → PDF 实测体积膨胀约 1.45×（PNG 页解码后走纯 zlib 压缩，没有 PNG 自己的逐行预测滤波，
# 压缩率不如原 PNG；JPEG 彩页原样直嵌不膨胀，但漫画多数是黑白页）——留点余量按 1.5× 估算，宁可保守
# 少生成几次 PDF，也不要估漏了传上去被设备拒。
NATIVE_PDF_SIZE_FACTOR = 1.5
# 跟 book-serve `BookConfig::default()` 的 `native_upload_limit_mb` 一致；那边改了要手动同步。
NATIVE_LIMIT_FALLBACK_MB = 150


def _staging_snapshot(transport) -> tuple[set[tuple[str, int]] | None, set[tuple[str, int]] | None]:
    """母版库快照，上传/处理前各查一次，返回两层独立的判重集合：
    ① `staged`——已在母版库的**处理后产物** (文件名, 字节数)，用来在批量部分失败后原样重跑时跳过
      已经成功落地的文件——不然会撞上 `rmsvc_core::fs::unique_path`"同名不覆盖"，把已经成功的
      文件在母版库里再落一份 `1_x` 副本（2026-09-13，Reddit 用户报告；见书架白皮书 §04）。
    ② `sources`——每份母版库文件对应的**原始输入身份** (文件名, 字节数)（`sidecar::SourceRef`，
      只有 CLI push 洗书产物才带这个字段，上传时随 `?srcName=&srcBytes=` 记进去），
      2026-09-14 补：用来在**处理之前**（不是处理完的产物之后）就判断"这份原始输入是不是已经
      成功处理过"，真正省掉洗书/重排这类耗时的 host 处理步骤，不只是省上传流量——①判重靠的是
      处理后产物的字节数，洗书/优化会改变体积，跟原始输入的字节数对不上，只能在处理完之后才用
      得上，这正是白皮书记录的"只省上传流量、没省处理时间"那个局限，②是为解决它新加的一层。
    两层都是纯文件名+字节数比较，不做内容 hash（同名同大小但内容真的换了会被误判为"已处理过"
    ——已知取舍，见 ① 的头注，换个文件名就绕过去）。查不到（设备不可达/接口异常）两个都返回
    `None`——不阻断推送，只是这次拿不到这两层保护，照常全部处理+上传。"""
    try:
        items = transport.get("/api/books/staging").get("items") or []
    except Exception:  # noqa: BLE001
        return None, None
    staged = {(it["name"], it["bytes"]) for it in items if "name" in it and "bytes" in it}
    sources = set()
    for it in items:
        src = (it.get("delivered") or {}).get("source") or {}
        if "name" in src and "bytes" in src:
            sources.add((src["name"], src["bytes"]))
    return staged, sources


def _native_limit_bytes(ctx) -> int:
    """查询设备当前原生上传上限（book-serve `/api/books/status` 的 `nativeUploadLimitBytes`）；查不到
    （设备不可达/字段缺）就退回跟 book-serve 缺省值一致的静态兜底——查询失败不阻断推送，只是这次拿不到
    「顺带出 PDF」这个加分项，纯 CBZ 照常传。"""
    try:
        v = ctx.transport.get("/api/books/status").get("nativeUploadLimitBytes")
        if isinstance(v, (int, float)) and v > 0:
            return int(v)
    except Exception:  # noqa: BLE001
        pass
    return NATIVE_LIMIT_FALLBACK_MB * 1024 * 1024


def comic_prepare(path: Path, work: Path, args=None, ctx=None) -> list[Path]:
    """漫画通道：→ CBZ 落母版库，去向 KOReader。缺省再过 16 灰省刷新档（含跨页拆分+白边裁切，用户
    2026-09-06 目视少闪后定默认开；`--no-eink-gray` 关，跨页拆分/白边裁切也跟着不做）；16 灰失败
    （如没 Pillow）退回原图 CBZ 并提示，不挡推送。灰阶 CBZ 估算转 PDF 后仍在设备原生上传上限内，
    就顺带转一份 PDF 一起落库——小体积漫画因此多一个「投入原生书库」的选项，大的照旧只有
    CBZ/KOReader，不强制分卷（`ctx` 缺失/`--no-comic-native` 时跳过这一步，2026-09-08）。"""
    if path.suffix.lower() == ".cbz":
        cbz = path
    else:
        cbz = cb.comic2cbz(path, work / (path.stem + ".cbz"))
        print("  漫画 → CBZ（加入 KOReader）")
    if getattr(args, "eink_gray", True):
        try:
            cbz, st = cb.comic_gray(cbz, work / (path.stem + ".gray.cbz"), rtl=not getattr(args, "manga_ltr", False))
            print(f"  省刷新档：拆跨页 {st.get('split', 0)} 页 / 16 灰 {st['gray']} 页 / 保色 {st['color']} 页；体积 {_mb(st['bytes_in'])} → {_mb(st['bytes_out'])}")
            for f in st.get("flagged", []):
                print(f"  ⚠ {f}")
        except cb.CalibreError as e:
            print(f"  ⚠ 16 灰失败，按原图 CBZ 推（{str(e)[:160]}）")
    out = [cbz]
    if getattr(args, "comic_native", True) and ctx is not None:
        limit = _native_limit_bytes(ctx)
        size = cbz.stat().st_size
        if size * NATIVE_PDF_SIZE_FACTOR <= limit:
            try:
                pdf = cb.cbz_to_pdf(cbz, work / (path.stem + ".pdf"))
                print(f"  体积估算在原生上传上限内（{_mb(limit)}）→ 顺带出一份 PDF（{_mb(pdf.stat().st_size)}），母版库多一个「投入原生书库」的选项")
                out.append(pdf)
            except cb.CalibreError as e:
                print(f"  ⚠ 生成投原生用 PDF 失败，仍保留 CBZ（{str(e)[:160]}）")
        else:
            print(f"  体积估算超原生上传上限（{_mb(limit)}），只出 CBZ（KOReader）——不强制分卷投原生")
    return out


def is_comic(path: Path, args) -> bool:
    if getattr(args, "no_comic", False):
        return False
    return getattr(args, "comic", False) or comic.is_comic(path)


def optimize_only_prepare(path: Path, args, work: Path) -> list[Path]:
    """`--no-calibre` 通道：只对 EPUB 有意义（`plan()` 已经把非 EPUB 挡在 raw），直接调
    `cb.optimize_only`——不经 `ebook-convert`，不需要装 Calibre，产物同样过 `_gate` 体检。"""
    out = cb.optimize_only(path, work / path.name, keep_spacing=getattr(args, "keep_spacing", False))
    print(f"  纯优化（跳过 Calibre）→ {out.name}")
    _gate(out, args)
    return [out]


def txt_prepare(path: Path, work: Path, wenv: dict | None) -> Path:
    """中文 TXT：切章建 EPUB（带两级目录）→ wash（TOC 已有，关 Calibre 自动目录）。返回洗好的 EPUB。"""
    epub, meta = cb.txt_to_epub(path, work)
    note = "" if meta.get("detected", True) else "；没认出「第X章」标题，按 8000 字硬切"
    vols = f"{meta['volumes']} 卷 " if meta.get("volumes") else ""
    print(f"  TXT 切章 → EPUB（{vols}{meta['chapters']} 章，{meta['encoding']}{note}）")
    return cb.wash(epub, work, env={**(wenv or {}), "WASH_AUTOTOC": "0"})


def host_prepare(path: Path, args, work: Path, ctx=None) -> list[Path]:
    """host 洗书：默认 EPUB 深洗 / 杂格式转 EPUB / TXT 切章 / PDF 结构化重排；--to-pdf 定稿 PDF；漫画走 comic_prepare。产物待落母版库。"""
    suf = path.suffix.lower()
    if is_comic(path, args):
        return comic_prepare(path, work, args, ctx)
    wenv = {"WASH_KEEP_PARA_SPACING": "1"} if getattr(args, "keep_spacing", False) else None  # 与网页档位对齐
    if suf == ".txt":
        out = txt_prepare(path, work, wenv)
        if args.to_pdf:
            out = cb.to_pdf(out, work / (path.stem + ".pdf"))
        _gate(out, args)
        return [out]
    if args.to_pdf:
        # 定稿固定版式 PDF（EPUB/杂格式先转 EPUB 再定稿；PDF 裁边）。
        if suf == ".epub" or suf in WASH_EXT:
            src = path if suf == ".epub" else cb.wash(path, work, env=wenv)
            out = cb.to_pdf(src, work / (path.stem + ".pdf"))
        elif suf == ".pdf":
            try:
                out = cb.crop_pdf(path, work / (path.stem + ".crop.pdf"))
            except cb.ScannedPdf as e:
                raise cb.CalibreError(f"扫描型 PDF 不做定稿（{e}）；去掉 --to-pdf 走重排，或用网页投 KOReader") from None
        else:
            return [path]
        _gate(out, args)
        return [out]
    # 默认：PDF 结构化重排 / EPUB·杂格式深洗成流式 EPUB。
    if suf == ".pdf":
        if args.no_reflow:
            return [path]
        reflowed, kind = cb.reflow_pdf(path, work)
        if kind == "epub":
            print(f"  结构化重排 → EPUB（{reflowed.name}）")
            out = cb.wash(reflowed, work, env=wenv)
            _gate(out, args)
            return [out]
        print(f"  扫描件位图重排 → PDF（{reflowed.name}）")
        return [reflowed]
    if suf == ".epub" or suf in WASH_EXT:
        out = cb.wash(path, work, env=wenv)  # wash_epub.sh 泛化收 AZW3/MOBI（内部 ebook-convert），末步叠加 epub-optimize
        _gate(out, args)
        return [out]
    return [path]


ROUTE_LABEL = {"raw": "原样→", "comic": "漫画 CBZ→", "wash": "洗书→", "optimize-only": "纯优化（跳过 Calibre）→"}


def plan(path: Path, args, calibre: bool) -> str:
    """一本书走哪条路：raw（原样）/ comic（→CBZ）/ wash（Calibre 洗书）/ optimize-only（`--no-calibre`，
    只对 EPUB 有意义）。纯函数，便于测试。`--no-calibre` 判在 is_comic 之前——AZW3/EPUB 漫画解包成 CBZ
    本来就要 Calibre，用户明确要求跳过 Calibre 就不该再暗地里用它，漫画分支这时对 EPUB 输入退化成普通
    优化（不拆 CBZ），非 EPUB 输入退化成 raw（母版库里再按需处理）。"""
    if path.suffix.lower() == ".cbz":
        return "comic" if getattr(args, "eink_gray", True) else "raw"  # CBZ 缺省再过 16 灰；--no-eink-gray 原样进母版库
    if args.no_optimize:
        return "raw"
    if getattr(args, "no_calibre", False):
        return "optimize-only" if path.suffix.lower() == ".epub" else "raw"
    if is_comic(path, args) and calibre:
        return "comic"  # AZW3/EPUB 漫画解包成 CBZ 要 Calibre
    return "wash" if calibre else "raw"


def run(args, ctx) -> int:
    calibre = cb.has_calibre()
    rc = 0
    landed = 0
    work = cb.workdir()
    if args.dry_run:
        # dry-run 只打印路由决定，不碰网络（不探活、不查母版库快照）——跟这次新加的两层判重
        # 都用不上：没有 sources 就没法预判"会不会跳过重新处理"，这点信息 dry-run 天生给不了，
        # 维持它原本"只看路由"的语义，不做半吊子的网络访问。
        for path in args.files:
            if not guard_file(path):
                rc = 1
                continue
            route = plan(path, args, calibre)
            print(f"→ {path.name}  {ROUTE_LABEL[route]}母版库")
        return rc
    # 探活 + 两层快照挪到循环最前面、处理任何一本书之前查（2026-09-14 补，见书架白皮书 §04）：
    # 源身份判重（sources，见 _staging_snapshot）要在处理前查才有意义，不能像原来那样等第一本
    # 处理完才探活——不然永远只能在处理完之后才知道"其实不用处理"，白干。
    # 代价：以前"探活放第一本处理完之后"是有意为之（洗书在这之前已做完，--wait 等设备醒的时间
    # 不白占），这次改成先探活、不可达就直接整批不处理，`--wait` 场景下第一本也要等设备醒了才
    # 开始处理，不再有这份"顺带白嫖"的重叠时间——重跑一批大部分已成功的场景（这个功能真正要
    # 省的场景）设备通常已经在线，这个损失不影响它；换来的是命中 source 的书完全不用处理。
    ready = ensure_reachable(ctx.transport, args.wait)  # 不可达时已经自己打印过原因（见 ensure_reachable）
    if not ready:
        print(f"✗ 未处理：{', '.join(p.name for p in args.files)}")
        return 2
    staged, sources = _staging_snapshot(ctx.transport)
    for path in args.files:
        if not guard_file(path):
            rc = 1
            continue
        # 源身份命中：这份原始输入之前已经成功处理过（同名同大小），处理都不用做，直接跳过——
        # 这是这次要补的那一层，跟下面"处理后产物判重"（staged）是两回事，见 _staging_snapshot。
        if sources is not None and (path.name, path.stat().st_size) in sources:
            print(f"✓ {path.name}: 原始文件已处理过（同名同大小），跳过重新处理")
            continue
        route = plan(path, args, calibre)
        print(f"→ {path.name}  {ROUTE_LABEL[route]}母版库")
        try:
            if route == "raw":
                outs = [path]
            elif route == "comic":
                outs = comic_prepare(path, work, args, ctx)
            elif route == "optimize-only":
                outs = optimize_only_prepare(path, args, work)
            else:
                outs = host_prepare(path, args, work, ctx)
        except cb.CalibreError as e:
            print(f"✗ {path.name}: {e}")
            rc = 1
            continue
        final: list[Path] = []
        for o in outs:
            # 漫画通道出的 PDF（体积已经卡过原生上限才生成，见 comic_prepare）绝不分卷——分卷跟这条路径无关，
            # 用户要的是"不分卷、装不下就别投原生"，分卷了还硬投原生正是 §03t 被否决掉的那个方案。
            if route != "comic" and not args.no_split and o.suffix.lower() == ".pdf" and pdfsplit.needs_split(o, ctx.config.split_pdf_mb):
                parts = pdfsplit.split(o, ctx.config.split_pdf_mb, work)
                if len(parts) > 1:
                    print(f"  分卷 {len(parts)} 份（>{ctx.config.split_pdf_mb}MB）")
                final.extend(parts)
            else:
                final.append(o)
        # 部分失败后原样重跑：已经在母版库里的（同名同大小）跳过，不重复上传——不然
        # unique_path 会把它再落一份 1_x（见 _staging_snapshot 头注）。
        to_upload = final
        if staged is not None:
            to_upload = []
            for f in final:
                key = (f.name, f.stat().st_size)
                if key in staged:
                    print(f"✓ {f.name}: 已在母版库（同名同大小），跳过重传")
                else:
                    to_upload.append(f)
        # 只落母版库（原样落，host 已洗则带优化标记）；去向在网页「传书 → 母版库」选。带上
        # ?srcName=&srcBytes=（这份产物的原始输入身份），服务端记进 sidecar，供下次命中 sources。
        if to_upload:
            src_query = {"srcName": path.name, "srcBytes": path.stat().st_size}
            rc |= upload_each(ctx.transport, "/api/books/staging", to_upload, query=lambda i, f: src_query)
            if staged is not None:
                staged.update((f.name, f.stat().st_size) for f in to_upload)
            if sources is not None:
                sources.add((path.name, path.stat().st_size))
        landed += len(final)
    if landed:
        scheme = getattr(ctx.config, "scheme", "https")
        print(f"→ 已入母版库。去 {scheme}://{ctx.config.host}:{ctx.config.port}/ 「传书 → 母版库」点优化 / 选去向（xochitl / KOReader）")
    return rc
