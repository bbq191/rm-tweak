"""C5 产物体检：Calibre 产物推送设备前的健康检查（挡"7 页书"级事故）。

PDF 检查项：
  1. 页面尺寸宽高比 ≈ 954:1696（Move 竖屏 0.5625，容差 2%；--any-size 跳过，
     裁边产物用）
  2. outline（书签树）条目数 —— 0 条默认告警，--require-toc 时判失败
  3. 内链注解：逐页 get_links()，goto 目标页越界 = 坏链（>0 判失败）
  4. 字体嵌入：CJK 字体须子集嵌入（basefont 形如 ABCDEF+Name）；Type 3 只告警
  5. 屏幕适配指标（按 Move 954×1696@264ppi 换算）：正文字号屏上 px/mm、正文列占页宽比、
     每行汉字数估算——列宽 <65% 或字号 <2.2mm 告警（2026-09-02 定稿流 72pt 隐藏边距
     吃掉半屏就是这条该拦的）

EPUB 检查项（tocfix 思路的最小集）：
  1. 加密检测：META-INF/encryption.xml 存在 = 硬失败（DRM 书，xochitl 读不了）
  2. nav.xhtml/toc.ncx 至少其一，条目 href 指向的文件必须存在（命中率 <80% 失败）
  3. 带 #锚点 的条目，锚点 id 必须在目标文件里（丢失仅告警——xochitl 退化到文件级）

用法: uv run --group calibre python shelf/host/calibre/check_output.py 产物.pdf|.epub [--require-toc] [--any-size]
退出码 0=通过（告警不拦），1=硬失败。输出 JSON 报告。
"""

from __future__ import annotations

import json
import posixpath
import re
import sys
import urllib.parse
import zipfile

import os
import sys as _sys

import pymupdf

_sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import move_screen  # noqa: E402

MOVE_ASPECT = move_screen.ASPECT  # 0.5625
ASPECT_TOL = 0.02
MIN_COLUMN_FRACTION = 0.65
MIN_BODY_MM = 2.2


def screen_metrics(doc: pymupdf.Document) -> dict:
    """采样中段 3 页：正文主字号（按字符数众数）与文字列宽 → 换算到 Move 屏（整页适配）。"""
    n = doc.page_count
    samples = [doc[i] for i in sorted({n // 3, n // 2, (2 * n) // 3}) if 0 <= i < n]
    sizes: dict[float, int] = {}
    col_fracs: list[float] = []
    for page in samples:
        blocks = [b for b in page.get_text("dict")["blocks"] if b.get("lines")]
        if not blocks:
            continue
        for b in blocks:
            for line in b["lines"]:
                for sp in line["spans"]:
                    k = round(sp["size"], 1)
                    sizes[k] = sizes.get(k, 0) + len(sp["text"])
        x0 = min(b["bbox"][0] for b in blocks)
        x1 = max(b["bbox"][2] for b in blocks)
        col_fracs.append((x1 - x0) / page.rect.width)
    if not sizes:
        return {}
    body_pt = max(sizes, key=sizes.get)
    rect = samples[0].rect
    scale = move_screen.fit_scale(rect.width, rect.height)
    body_px = body_pt * scale
    col = max(col_fracs) if col_fracs else 0.0
    return {
        "body_font_pt": body_pt,
        "body_font_px": round(body_px, 1),
        "body_font_mm": round(move_screen.px_to_mm(body_px), 2),
        "text_column_fraction": round(col, 2),
        "cjk_chars_per_line": int(col * rect.width * scale / body_px) if body_px else 0,
        "screen_scale_px_per_pt": round(scale, 3),
    }


def check(path: str, require_toc: bool, any_size: bool = False) -> dict:
    doc = pymupdf.open(path)
    errors: list[str] = []
    warnings: list[str] = []

    if doc.page_count == 0:
        return {"ok": False, "errors": ["零页 PDF"], "warnings": []}

    # 1. 页面尺寸
    rect = doc[0].rect
    aspect = rect.width / rect.height
    if not any_size and abs(aspect - MOVE_ASPECT) > ASPECT_TOL:
        errors.append(
            f"页面宽高比 {aspect:.4f} 偏离 Move 屏 {MOVE_ASPECT:.4f}"
            f"（首页 {rect.width:.0f}x{rect.height:.0f}pt）"
        )

    # 2. outline
    toc = doc.get_toc(simple=True)
    if not toc:
        (errors if require_toc else warnings).append("无 outline/书签（TOC 面板将为空）")

    # 3. 内链
    internal = broken = external = 0
    for page in doc:
        for link in page.get_links():
            kind = link.get("kind")
            if kind == pymupdf.LINK_GOTO:
                internal += 1
                target = link.get("page", -1)
                if not 0 <= target < doc.page_count:
                    broken += 1
            elif kind == pymupdf.LINK_URI:
                external += 1
    if broken:
        errors.append(f"{broken} 条内链目标页越界")

    # 4. 字体嵌入（CJK 字体须子集：basefont 带 6 字母+ 前缀）。
    # Type 3 匿名字体 = CFF 底字体（Noto CJK OTC 等）被 Chromium/Skia 兜底，
    # 换 TTF 字体（如霞鹜新致宋）可得正规 CID TrueType 子集（2026-09-02 实测）。
    # Type 3 = Chromium/Skia 系管线对 CFF 底字体或合成粗体的兜底输出。带
    # ToUnicode 时渲染与文字层都正常（财新周刊整本 Type 3 实证），故一律
    # 告警不拦；自产 PDF 想要正规 CID 子集就 PDF_FONT 选 TTF 字体。
    fonts: dict[str, bool] = {}
    type3_pages: set[int] = set()
    for pno in range(doc.page_count):
        for f in doc.get_page_fonts(pno):
            ftype, basefont = f[2], f[3]
            if ftype == "Type3":
                type3_pages.add(pno)
                continue
            fonts.setdefault(basefont, "+" in basefont)
    type3 = len(type3_pages)
    if type3:
        warnings.append(f"Type 3 字体见于 {type3}/{doc.page_count} 页（自产书换 TTF 字体可消；渲染无碍）")
    cjk = {n: sub for n, sub in fonts.items() if _looks_cjk(n)}
    if not cjk and not type3:
        warnings.append("未发现 CJK 字体（英文书可忽略）")
    for name, subset in cjk.items():
        if not subset:
            errors.append(f"CJK 字体 {name} 未子集嵌入（体积会失控）")

    # 5. 屏幕适配指标
    metrics = screen_metrics(doc)
    if metrics:
        if metrics["text_column_fraction"] < MIN_COLUMN_FRACTION:
            warnings.append(
                f"正文列仅占页宽 {metrics['text_column_fraction']:.0%}（自产 PDF 查 "
                f"--pdf-page-margin-*；未裁存量书走 pdf_crop_move.py；已裁过仍窄=原版面比屏更瘦、"
                f"放大率被页高卡住，只能 KOReader KOPT 重排）"
            )
        if metrics["body_font_mm"] < MIN_BODY_MM:
            warnings.append(
                f"正文字号屏上仅 {metrics['body_font_mm']}mm（<{MIN_BODY_MM}mm 难读；"
                f"扫描/双栏书改走 KOReader KOPT 重排）"
            )

    return {
        "ok": not errors,
        "file": path,
        "pages": doc.page_count,
        "page_pt": [round(rect.width), round(rect.height)],
        "screen": metrics,
        "toc_entries": len(toc),
        "toc_max_depth": max((lvl for lvl, _, _ in toc), default=0),
        "links_internal": internal,
        "links_external": external,
        "links_broken": broken,
        "fonts": {n: ("subset" if s else "FULL") for n, s in fonts.items()},
        "errors": errors,
        "warnings": warnings,
    }


def _looks_cjk(basefont: str) -> bool:
    name = basefont.split("+")[-1].lower()
    return any(
        k in name
        for k in ("cjk", "sourcehan", "noto", "wenkai", "lxgw", "song", "hei", "kai", "ming")
    )


def check_epub(path: str, require_toc: bool) -> dict:
    errors: list[str] = []
    warnings: list[str] = []
    z = zipfile.ZipFile(path)
    names = set(z.namelist())

    # encryption.xml ≠ 必然 DRM：IDPF/Adobe 字体混淆也合法使用它（只列字体文件）。
    # 只有正文/内容文件被加密才是真 DRM（《飘》dkagent.css 那类）。
    if "META-INF/encryption.xml" in names:
        enc = z.read("META-INF/encryption.xml").decode("utf-8", "ignore")
        targets = re.findall(r'CipherReference\s+URI="([^"]+)"', enc)
        non_font = [t for t in targets
                    if not t.lower().endswith((".ttf", ".otf", ".woff", ".woff2"))]
        if non_font:
            errors.append(f"加密 EPUB（DRM，加密了 {non_font[:3]} 等），xochitl/KOReader 都读不了")
        else:
            warnings.append(f"仅字体混淆（{len(targets)} 个字体文件，非 DRM，可读）")

    tocs = [n for n in names
            if n.endswith(".ncx") or re.search(r"nav[^/]*\.x?html$", n.rsplit("/", 1)[-1])]
    entries: list[tuple[str, str, str]] = []  # (toc文件, 目标文件, 锚点)
    for toc in tocs:
        base = posixpath.dirname(toc)
        data = z.read(toc).decode("utf-8", "ignore")
        for href in re.findall(r'(?:src|href)="([^"#]+)(#[^"]*)?"', data):
            # href 是 URI（%2a 等百分号编码），zip 条目名是原字符——必须先解码
            rel = urllib.parse.unquote(href[0])
            target = posixpath.normpath(posixpath.join(base, rel)) if base else rel
            entries.append((toc, target, urllib.parse.unquote(href[1].lstrip("#"))))
    if not tocs or not entries:
        (errors if require_toc else warnings).append("无 nav/ncx 或目录零条目")

    file_hit = sum(1 for _, t, _ in entries if t in names)
    frag_entries = [(t, f) for _, t, f in entries if f and t in names]
    frag_hit = 0
    html_cache: dict[str, str] = {}
    for target, frag in frag_entries:
        if target not in html_cache:
            html_cache[target] = z.read(target).decode("utf-8", "ignore")
        if re.search(r'(?:id|name)="' + re.escape(frag) + '"', html_cache[target]):
            frag_hit += 1
    if entries and file_hit / len(entries) < 0.8:
        errors.append(f"目录 href 文件命中率过低 {file_hit}/{len(entries)}")
    if frag_entries and frag_hit < len(frag_entries):
        warnings.append(f"目录锚点丢失 {len(frag_entries) - frag_hit}/{len(frag_entries)}（xochitl 退化到文件级跳转）")

    # 双 id 属性 = xochitl 严格 XML 解析整章白屏（《消失的爱人》7 页事故根因）。
    # AZW3/MOBI 转来的书是高危路径；设备端 collapse_dup_id_attrs 缺位时这里必须拦。
    dup_id = 0
    for n in names:
        if n.endswith((".xhtml", ".html", ".htm")):
            for tag in re.finditer(r"<[a-zA-Z][^>]*>", z.read(n).decode("utf-8", "ignore")):
                if len(re.findall(r'\bid="', tag.group(0))) > 1:
                    dup_id += 1
    if dup_id:
        errors.append(f"{dup_id} 个标签带双 id 属性（非法 XHTML，xochitl 整章白屏）")

    return {
        "ok": not errors,
        "file": path,
        "toc_files": tocs,
        "toc_entries": len(entries),
        "href_file_hit": file_hit,
        "frag_hit": f"{frag_hit}/{len(frag_entries)}",
        "errors": errors,
        "warnings": warnings,
    }


def main() -> int:
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    if len(args) != 1:
        print(__doc__)
        return 2
    require_toc = "--require-toc" in sys.argv
    if args[0].lower().endswith(".epub"):
        report = check_epub(args[0], require_toc)
    elif args[0].lower().endswith(".pdf"):
        report = check(args[0], require_toc, any_size="--any-size" in sys.argv)
    else:
        print(__doc__)
        return 2
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
