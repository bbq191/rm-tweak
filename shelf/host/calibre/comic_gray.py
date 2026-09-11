"""漫画省刷新档 + 跨页拆分 + 白边裁切：CBZ → CBZ。

**跨页拆分**（2026-09-08）：扫描版漫画常见"两页拼一张扫描图"（宽高比明显 >1 的横图，中间有装订缝）；
原来的管线把这种图当一页整个塞进竖屏，等于变相把内容挤错位置。现在每页先判断像不像跨页（宽高比阈值），
像的话在图片中段找一条内容最少的竖直窄带当装订缝（不是死切正中间，兼容装订缝偏移/扫描略歪），拆成两张
单页，按传统东亚漫画从右往左的阅读顺序排（`rtl=True`，缺省；`--ltr` 关）。找不到干净装订缝、或者拆完
两半还是明显偏宽（比如异常的三联跨页/扉页），就放弃拆分、原图直出，记进 `flagged` 提示人工看一眼——
不弄巧成拙瞎切。

**白边裁切 + 放大**（2026-09-08）：跨页拆完（或本来就是单页）之后，找每张内容包围盒（从四边向内扫，
排除大片纯背景留白），裁掉多余白边。单轴裁掉超过 `MAX_TRIM_FRAC` 就放弃裁那一轴——防止大面积均匀浅色
内容（满页留白分镜、渐变背景）被误判成"白边"整个裁没。裁完的图允许放大（原来的 `fit_screen` 从不放大），
放大倍数封顶 `MAX_UPSCALE` 防止小图被拉花。

省刷新（黑白/偏色页转 16 级灰 + Floyd-Steinberg 抖动 → 4-bit PNG，真彩页保色存 JPEG）：依据墨水屏波形
按内容分档（彩色重 / 256 灰中 / ≤16 灰轻，真机坐实），16 灰是画质与减闪的甜点。**默认开**
（`shelf push --no-eink-gray` 关）：实测《阿拉蕾①》171MB→108MB，用户目视翻页明显少闪后定默认开
（2026-09-06）。灰度/降采样阈值镜像设备端 bookconv `imgopt.rs`（`COLOR_KEEP_CHROMA=0.06`）——两处
同一份，改一处另一处同步。前身：已删的 `einkify_epub.py`（EPUB 内图；`git show 560a8b5^`）。

依赖 Pillow（`uv run --group calibre`），**不引入 numpy**——内容密度剖面靠 PIL 原生 `resize(..., Image.BOX)`
把二值掩码缩成 1 像素厚做逐列/逐行平均，C 实现，够快，见 `_axis_profile`。
用法: python comic_gray.py [--ltr] <in.cbz> <out.cbz> → 末行 JSON
  {out,pages,split,gray,color,bytes_in,bytes_out,flagged:[str,...]}
"""
from __future__ import annotations

import io
import json
import re
import sys
import zipfile
from pathlib import Path

from PIL import Image

LONG_EDGE = 1696  # Move 屏：长边 ≤1696 且短边 ≤954（imgopt.rs 同规则）
SHORT_EDGE = 954
GRAY_LEVELS = 16
COLOR_KEEP_CHROMA = 0.06
IMG_EXTS = (".jpg", ".jpeg", ".png", ".gif", ".webp", ".bmp")
_NUM = re.compile(r"(\d+)")

# 跨页识别：正常竖版单页宽高比约 0.5–0.85；超过这个就可能是跨页扫描图。
SPREAD_RATIO_MIN = 1.05
# 比例夸张到这个程度，就算装订缝没找干净也认定是跨页（保底靠猜的位置拆，好过整张塞进竖屏）。
SPREAD_RATIO_CONFIDENT = 1.3
# 装订缝候选窗口：图片中段 35%–65% 宽度，不假设正中间。
GUTTER_SEARCH_LO = 0.35
GUTTER_SEARCH_HI = 0.65
# 候选列内容密度低于此值才算"干净装订缝"（真找到白色缝隙，不是蒙对的）。
GUTTER_CLEAN_FRAC = 0.01
# 白边裁切：背景阈值（灰度 <245 算内容）、行/列内容占比超过此值才算"有内容"、单轴最多裁掉这么多。
BG_THRESH = 245
CONTENT_FRAC_THRESH = 0.005
MAX_TRIM_FRAC = 0.20
# 裁完放大的倍数上限，防止小图被拉花。
MAX_UPSCALE = 1.5


def natural_key(name: str):
    return [int(t) if t.isdigit() else t.lower() for t in _NUM.split(name)]


def mean_chroma(img: Image.Image) -> float:
    """页面平均色度：RGB 极差均值 /255，每 total/40000 像素采一样本（与 imgopt.rs `mean_chroma` 同法）。灰度图恒 0。"""
    if img.mode in ("L", "LA", "1"):
        return 0.0
    rgb = img.convert("RGB")
    w, h = rgb.size
    total = w * h
    if total == 0:
        return 0.0
    step = max(1, total // 40_000)
    buf = rgb.tobytes()  # RGBRGB…（getdata 在 Pillow 12 已弃用）
    s = n = 0
    for i in range(0, total, step):
        r, g, b = buf[3 * i], buf[3 * i + 1], buf[3 * i + 2]
        s += max(r, g, b) - min(r, g, b)
        n += 1
    return (s / n) / 255.0 if n else 0.0


def _content_mask(img: Image.Image) -> Image.Image:
    """L 图 → 二值掩码：非背景（灰度 <BG_THRESH）=255，背景=0。"""
    return img.convert("L").point(lambda p: 255 if p < BG_THRESH else 0)


def _axis_profile(mask: Image.Image, axis: str) -> list[float]:
    """按轴（'x' 逐列 / 'y' 逐行）算内容密度剖面（0..1）：用 BOX 滤波把掩码缩成 1 像素厚做平均——
    PIL C 实现，不用挨个像素跑 Python 循环（一页几百万像素纯 Python 扫太慢）。"""
    w, h = mask.size
    if axis == "x":
        strip = mask.resize((w, 1), Image.BOX)
        return [strip.getpixel((i, 0)) / 255.0 for i in range(w)]
    strip = mask.resize((1, h), Image.BOX)
    return [strip.getpixel((0, i)) / 255.0 for i in range(h)]


def content_bbox(img: Image.Image) -> tuple[int, int, int, int]:
    """内容包围盒 (x0,y0,x1,y1)。单轴裁掉的比例超过 MAX_TRIM_FRAC 就放弃裁那一轴（原样返回该轴全范围）——
    防止大面积均匀浅色内容（满页留白分镜、渐变背景）被误判成"白边"整个裁没。"""
    w, h = img.size
    mask = _content_mask(img)

    def bounds(profile: list[float], total: int) -> tuple[int, int]:
        idx = [i for i, v in enumerate(profile) if v > CONTENT_FRAC_THRESH]
        if not idx:
            return 0, total
        lo, hi = idx[0], idx[-1] + 1
        if (lo + (total - hi)) / total > MAX_TRIM_FRAC:
            return 0, total
        return lo, hi

    x0, x1 = bounds(_axis_profile(mask, "x"), w)
    y0, y1 = bounds(_axis_profile(mask, "y"), h)
    return x0, y0, x1, y1


def crop_border(img: Image.Image) -> Image.Image:
    """裁掉内容包围盒外的白边，留一点安全边距（避免裁进画格边框线）。整页都是内容（或裁太多被放弃）就原样返回。"""
    x0, y0, x1, y1 = content_bbox(img)
    if (x0, y0, x1, y1) == (0, 0, img.width, img.height):
        return img
    pad = max(2, round(0.01 * min(img.size)))
    x0, y0 = max(0, x0 - pad), max(0, y0 - pad)
    x1, y1 = min(img.width, x1 + pad), min(img.height, y1 + pad)
    return img.crop((x0, y0, x1, y1))


def fit_to_screen(img: Image.Image, allow_upscale: bool = False) -> Image.Image:
    """缩放到屏幕盒内。`allow_upscale`：裁完白边后的图允许放大回填屏幕（封顶 MAX_UPSCALE），
    原来的 `fit_screen` 只降不升——旧调用方式仍保留（`allow_upscale=False`）。"""
    k = min(LONG_EDGE / max(img.size), SHORT_EDGE / min(img.size))
    k = min(k, MAX_UPSCALE) if allow_upscale else min(k, 1.0)
    if k != 1.0:
        img = img.resize((max(1, round(img.width * k)), max(1, round(img.height * k))), Image.LANCZOS)
    return img


def looks_like_spread(img: Image.Image) -> bool:
    w, h = img.size
    return h > 0 and (w / h) > SPREAD_RATIO_MIN


def find_gutter_x(img: Image.Image) -> tuple[int, float]:
    """在图片中段（GUTTER_SEARCH_LO–HI）找一条内容密度最低的竖直窄带当装订缝。返回 (x, 该列内容密度)。"""
    w, _ = img.size
    xs = _axis_profile(_content_mask(img), "x")
    lo, hi = int(w * GUTTER_SEARCH_LO), max(int(w * GUTTER_SEARCH_LO) + 1, int(w * GUTTER_SEARCH_HI))
    window = xs[lo:hi]
    x = lo + min(range(len(window)), key=lambda i: window[i])
    return x, window[x - lo]


def split_spread(img: Image.Image, rtl: bool = True) -> tuple[list[Image.Image], str | None]:
    """跨页图 → 拆成两张单页，按阅读顺序排好（`rtl` 缺省从右往左，东亚漫画传统）。
    返回 (页面列表, 提示)：没拆或拆得有把握时提示是 None；拆得没把握、或判断像跨页但放弃拆分时，
    提示是给用户看的一句话（推荐人工核对），供 `convert()` 汇总进 `flagged`。"""
    w, h = img.size
    if not looks_like_spread(img):
        return [img], None
    x, density = find_gutter_x(img)
    clean = density <= GUTTER_CLEAN_FRAC
    ratio = w / h
    if not clean and ratio < SPREAD_RATIO_CONFIDENT:
        return [img], None  # 比例不算夸张、也没找到干净装订缝——大概率不是跨页，不瞎拆
    left, right = img.crop((0, 0, x, h)), img.crop((x, 0, w, h))

    def still_too_wide(p: Image.Image) -> bool:
        return looks_like_spread(p) and (p.width / p.height) > SPREAD_RATIO_CONFIDENT

    if still_too_wide(left) or still_too_wide(right):
        return [img], f"跨页拆分后仍偏宽（原图 {w}x{h}），已放弃拆分保留原图，建议人工核对是否三联跨页/扉页"
    flag = None if clean else f"装订缝不够干净（猜测切割点 x={x}），已按最空一列拆分，建议核对"
    return ([right, left] if rtl else [left, right]), flag


def to_gray16(img: Image.Image) -> Image.Image:
    """L → 16 级等距灰调色板 + FS 抖动（P 模式）。"""
    pal = Image.new("P", (1, 1))
    steps = [round(i * 255 / (GRAY_LEVELS - 1)) for i in range(GRAY_LEVELS)]
    pal.putpalette(sum(([v, v, v] for v in steps), []) + [0] * (768 - GRAY_LEVELS * 3))
    return img.convert("L").convert("RGB").quantize(palette=pal, dither=Image.FLOYDSTEINBERG)


def _encode(img: Image.Image) -> tuple[bytes, str, str]:
    """裁白边+放大后编码一张页面 → (字节, 扩展名, 'gray'|'color')。"""
    img = fit_to_screen(crop_border(img), allow_upscale=True)
    out = io.BytesIO()
    if mean_chroma(img) >= COLOR_KEEP_CHROMA:
        img.convert("RGB").save(out, "JPEG", quality=85)
        return out.getvalue(), ".jpg", "color"
    to_gray16(img).save(out, "PNG", optimize=True, bits=4)
    return out.getvalue(), ".png", "gray"


def process(data: bytes, rtl: bool = True) -> tuple[list[tuple[bytes, str, str]], str | None]:
    """一页 → 1~2 张输出（跨页会拆开）+ 需要人工核对的提示（没有就 None）。解不开的原样返回，kind='raw'。"""
    try:
        img = Image.open(io.BytesIO(data))
        img.load()
    except Exception:  # noqa: BLE001
        return [(data, "", "raw")], None
    parts, flag = split_spread(img, rtl=rtl)
    return [_encode(p) for p in parts], flag


def convert(src: Path, out: Path, rtl: bool = True) -> dict:
    stats = {"out": str(out), "pages": 0, "split": 0, "gray": 0, "color": 0, "bytes_in": 0, "bytes_out": 0, "flagged": []}
    with zipfile.ZipFile(src) as zin, zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as zout:
        names = sorted((n for n in zin.namelist() if n.lower().endswith(IMG_EXTS) and not n.endswith("/")), key=natural_key)
        suffix = "abcdefgh"
        for n in names:
            raw = zin.read(n)
            parts, flag = process(raw, rtl=rtl)
            stem, _, ext_in = n.rpartition(".")
            stem = stem or n
            for i, (data, ext, kind) in enumerate(parts):
                tag = suffix[i] if len(parts) > 1 else ""
                zout.writestr(stem + tag + (ext or f".{ext_in}"), data)
                stats["bytes_out"] += len(data)
                if kind in ("gray", "color"):
                    stats[kind] += 1
            stats["pages"] += 1
            stats["bytes_in"] += len(raw)
            if len(parts) > 1:
                stats["split"] += 1
            if flag:
                stats["flagged"].append(f"{n}: {flag}")
    if not stats["pages"]:
        raise SystemExit("CBZ 内无图片")
    return stats


def main() -> int:
    args = sys.argv[1:]
    rtl = "--ltr" not in args
    args = [a for a in args if a != "--ltr"]
    if len(args) != 2:
        print("用法: comic_gray.py [--ltr] <in.cbz> <out.cbz>", file=sys.stderr)
        return 2
    print(json.dumps(convert(Path(args[0]), Path(args[1]), rtl=rtl), ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
