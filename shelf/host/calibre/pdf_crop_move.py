"""C1b 存量 PDF 书处理：先分流（文字型/扫描型/混合），文字型统一裁边后适配
Move 954×1696 小屏；保留 outline/内链（相对 k2pdfopt 重排的核心优势）。

分流判定内置 pymupdf 实现（借鉴 firecrawl/pdf-inspector 的思路，不引其编译依赖）：
均匀采样页的可抽取文字量分类。扫描型不产出——重排交给设备上 KOReader 的
KOPT（交互式、可逐书调），或 xochitl 内置 Adjust view。

裁边法：逐页内容 bbox（文字块+图块并集）→ 奇偶页分开取 5/95 百分位 →
全书统一裁剪框（奇偶各一）→ set_cropbox。统一框避免逐页裁导致翻页跳动。

裁完按 Move 屏比例 0.5625 **补齐**裁剪框（默认开，--no-pad 关）：xochitl 整页适配显示，
框比例偏离屏幕只会留黑边、不改放大率；补齐后页面恰好铺满屏、手写留白位置全书一致。
补齐只能在 mediabox 内扩，扩不动就按能扩的算（报告里 aspect 能看出来）。
输出带屏幕适配指标（放大率、正文字号屏上 mm、每行字数）——字号 <2.2mm 建议改走
KOReader KOPT 重排而不是硬看。

用法: uv run --group calibre python shelf/host/calibre/pdf_crop_move.py in.pdf [out.pdf]
      --force-crop  混合型/扫描型也硬裁（默认只裁文字型）
      --no-pad      不按屏幕比例补齐裁剪框
退出码: 0=已产出  3=判为扫描型未产出（走 KOReader KOPT）  1=错误
"""

from __future__ import annotations

import json
import os
import sys

import pymupdf

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import move_screen  # noqa: E402

TEXT_PAGE_MIN_CHARS = 100
SAMPLE_MAX = 24
PAD_PT = 6.0


def classify(doc: pymupdf.Document) -> tuple[str, float]:
    n = doc.page_count
    step = max(1, n // SAMPLE_MAX)
    sampled = range(0, n, step)
    text_pages = sum(
        1 for i in sampled if len(doc[i].get_text("text").strip()) >= TEXT_PAGE_MIN_CHARS
    )
    ratio = text_pages / max(1, len(list(sampled)))
    if ratio >= 0.8:
        return "text", ratio
    if ratio <= 0.2:
        return "scanned", ratio
    return "mixed", ratio


def content_bbox(page: pymupdf.Page) -> pymupdf.Rect | None:
    box = pymupdf.Rect()
    for b in page.get_text("blocks"):  # 文字块与图块都在内
        box |= pymupdf.Rect(b[:4])
    return None if box.is_empty else box


def percentile(vals: list[float], pct: float) -> float:
    vals = sorted(vals)
    idx = min(len(vals) - 1, max(0, int(round(pct * (len(vals) - 1)))))
    return vals[idx]


def group_crop(boxes: list[pymupdf.Rect], page_rect: pymupdf.Rect) -> pymupdf.Rect:
    r = pymupdf.Rect(
        percentile([b.x0 for b in boxes], 0.05) - PAD_PT,
        percentile([b.y0 for b in boxes], 0.05) - PAD_PT,
        percentile([b.x1 for b in boxes], 0.95) + PAD_PT,
        percentile([b.y1 for b in boxes], 0.95) + PAD_PT,
    )
    return r & page_rect


def pad_to_aspect(r: pymupdf.Rect, bound: pymupdf.Rect, aspect: float) -> pymupdf.Rect:
    """把 r 居中扩到宽高比 aspect（只扩不缩），再夹进 bound。"""
    w, h = r.width, r.height
    if w / h < aspect:
        dw = h * aspect - w
        out = pymupdf.Rect(r.x0 - dw / 2, r.y0, r.x1 + dw / 2, r.y1)
    else:
        dh = w / aspect - h
        out = pymupdf.Rect(r.x0, r.y0 - dh / 2, r.x1, r.y1 + dh / 2)
    # 越界则整体平移回 bound 内（比单边裁掉更能保住比例）
    if out.x0 < bound.x0:
        out += (bound.x0 - out.x0, 0, bound.x0 - out.x0, 0)
    if out.x1 > bound.x1:
        out += (bound.x1 - out.x1, 0, bound.x1 - out.x1, 0)
    if out.y0 < bound.y0:
        out += (0, bound.y0 - out.y0, 0, bound.y0 - out.y0)
    if out.y1 > bound.y1:
        out += (0, bound.y1 - out.y1, 0, bound.y1 - out.y1)
    return out & bound


def body_font_pt(doc: pymupdf.Document) -> float:
    sizes: dict[float, int] = {}
    n = doc.page_count
    for i in sorted({n // 3, n // 2, (2 * n) // 3}):
        for b in doc[i].get_text("dict")["blocks"]:
            for line in b.get("lines", []):
                for sp in line["spans"]:
                    k = round(sp["size"], 1)
                    sizes[k] = sizes.get(k, 0) + len(sp["text"])
    return max(sizes, key=sizes.get) if sizes else 0.0


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    force = "--force-crop" in sys.argv
    pad = "--no-pad" not in sys.argv
    if not args:
        print(__doc__)
        return 1
    src = args[0]
    out = args[1] if len(args) > 1 else src[:-4] + "-crop.pdf"

    doc = pymupdf.open(src)
    kind, ratio = classify(doc)
    if kind != "text" and not force:
        print(json.dumps({
            "kind": kind, "text_ratio": round(ratio, 2), "cropped": False,
            "advice": "扫描/混合型：设备上用 KOReader 重排(KOPT) 或 xochitl Adjust view；硬裁加 --force-crop",
        }, ensure_ascii=False))
        return 3

    odd: list[pymupdf.Rect] = []
    even: list[pymupdf.Rect] = []
    for i in range(doc.page_count):
        if doc[i].rotation != 0:
            continue
        box = content_bbox(doc[i])
        if box:
            (odd if i % 2 else even).append(box)
    if not odd and not even:
        print(json.dumps({"kind": kind, "cropped": False, "advice": "无可测内容框"},
                         ensure_ascii=False))
        return 1

    crop_even = group_crop(even or odd, doc[0].rect)
    crop_odd = group_crop(odd or even, doc[0].rect)
    content_w = crop_even.width  # 补齐前的正文列宽（算每行字数用）
    if pad:
        crop_even = pad_to_aspect(crop_even, doc[0].rect, move_screen.ASPECT)
        crop_odd = pad_to_aspect(crop_odd, doc[0].rect, move_screen.ASPECT)
    skipped = 0
    for i in range(doc.page_count):
        page = doc[i]
        if page.rotation != 0:
            skipped += 1
            continue
        target = (crop_odd if i % 2 else crop_even) & page.rect
        # 内容 bbox 在 page.rect 坐标系（当前 cropbox、顶左原点）；set_cropbox 要
        # mediabox 顶左原点的未旋转坐标 → 加 cropbox_position 偏移换系。
        p = page.cropbox_position
        shifted = pymupdf.Rect(target.x0 + p.x, target.y0 + p.y,
                               target.x1 + p.x, target.y1 + p.y)
        try:
            page.set_cropbox(shifted)
        except (ValueError, RuntimeError):
            skipped += 1

    doc.save(out, garbage=4, deflate=True)
    before = doc[0].mediabox
    scale = move_screen.fit_scale(crop_even.width, crop_even.height)
    fpt = body_font_pt(doc)
    fpx = fpt * scale
    fmm = move_screen.px_to_mm(fpx)
    report = {
        "kind": kind, "text_ratio": round(ratio, 2), "cropped": True, "out": out,
        "media_pt": [round(before.width), round(before.height)],
        "crop_even_pt": [round(crop_even.width), round(crop_even.height)],
        "crop_odd_pt": [round(crop_odd.width), round(crop_odd.height)],
        "crop_aspect": round(crop_even.width / crop_even.height, 4),
        "screen": {
            "scale_px_per_pt": round(scale, 3),
            "body_font_pt": fpt, "body_font_px": round(fpx, 1), "body_font_mm": round(fmm, 2),
            "cjk_chars_per_line": int(content_w * scale / fpx) if fpx else 0,
        },
        "pages_skipped": skipped,
        "toc_entries": len(doc.get_toc(simple=True)),
    }
    if fpt and fmm < 2.2:
        report["advice"] = "屏上正文 <2.2mm：小屏硬看吃力，建议改走 KOReader KOPT 重排"
    print(json.dumps(report, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
