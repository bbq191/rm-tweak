"""PDF 重排（host 侧，2026-09-04 定案：born-digital 结构化重排→EPUB，扫描件回退 k2pdfopt/裁边）。

第一性依据（见书架白皮书）：k2pdfopt 是位图重排引擎（为通吃扫描件，代价=文字变图不可选）；
born-digital 学术 PDF 自带文字+图/公式 bbox（PyMuPDF），**结构化重排比位图更简单更好、且产物 EPUB
能复用书架 EPUB 管线**（xochitl 内联脚注/KOReader 弹窗/一致排版）。故：
  - 有可抽取文字层 → 结构化：抽 blocks/图 bbox、按 x 聚列、列内按 y 定阅读序；**整本平铺成单元后做
    文档级段落合并**（PyMuPDF 对杂志/论文常常每行一个块，"块=段"不成立）；**按标题分章**（无标题则每
    N 页一章）；图/公式裁原区当整块不切、高度压到 ≤ 屏高 60%；组 EPUB → 上层 wash + epub-optimize 统一优化。
    ⚠ 2026-09-05 真机《财新周刊》教训：曾"每页一章 + 每行一段" → 每页末强制翻页留大片空白、段落碎成行。
  - 无/极低文字层（扫描件）→ 调 k2pdfopt 二进制（缺则回退 pdf_crop_move.py 裁边）→ 输出 PDF。

用法: pdf_reflow_move.py <输入.pdf> <输出目录>
输出: 打印一行 JSON {"out": "<产物路径>", "kind": "epub"|"pdf"}；rc 0 成功，2 失败。
"""

from __future__ import annotations

import html
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

try:
    import pymupdf as fitz  # 新名（PyMuPDF ≥1.24）
except ImportError:  # 老环境回退旧名
    import fitz

from epub_skel import Chapter, write_epub  # noqa: E402  共享 EPUB 骨架
from move_screen import H_PX, W_PX  # noqa: E402  单一事实源屏常量

# born-digital 判据：平均每页可抽取文字 ≥ 该字符数 → 有文字层。扫描件通常近 0。
MIN_CHARS_PER_PAGE = 80
# 标题判据：span 字号 ≥ 页正文中位字号 × 该倍数，且位于列首/页顶（排除正文中间的大字引语）。
HEADING_FONT_RATIO = 1.35
HEADING_TOP_FRAC = 0.2
# 无标题时每章页数（避免单文件过大，也避免每页翻章）。
PAGES_PER_CHAPTER_FALLBACK = 12
# 图块输出上限：宽 ≤ 屏宽、高 ≤ 屏高 60%（整屏高的图放不进剩余空间会整体推到下页留白）。
FIG_MAX_W = W_PX
FIG_MAX_H = int(H_PX * 0.6)

_TERMINAL = "。！？；…”」』）】》.!?:\"'"
_ZW = {0x200B: None, 0x200C: None, 0x200D: None, 0xFEFF: None, 0x00AD: None, 0x2060: None}
# 杂志版式的"非正文行"（2026-09-06 《财新》第 33 期 770 段实测：15% 段不以句末标点结尾，其中六成是这些，不是断句）：
#   署名行「文｜某某」/「图｜」/「摄影｜」、贡献行「某某对此文亦有贡献」、图注「图：…」「…/IC photo」「…视觉中国」、
#   原文链接行、小标题（短、字号略大、不带句末标点）。它们各自成段、绝不进目录、绝不与上下段续接。
_BYLINE_RE = re.compile(r"^(文|图|摄影|摄|编辑|记者|撰文|整理|译|插画|制图)\s*[｜|/／]")
_CREDIT_RE = re.compile(r"(对此文亦有贡献|对本文亦有贡献|亦有贡献)")
_CAPTION_RE = re.compile(r"^(图|表|图片来源|资料来源|来源|摄影?|制图|数据来源)\s*[：:]|(/IC photo|视觉中国|/东方IC|/CFP|图/[^/]{1,12})$")
_LINK_RE = re.compile(r"^原文链接\s*[：:]")
# 小标题：不超过这个字数、不以句末标点结尾、字号 ≥ 正文 + SUBHEAD_SIZE_DELTA
SUBHEAD_MAX_CHARS = 25
SUBHEAD_SIZE_DELTA = 0.5
# 文章级标题（分章）：字号 ≥ 正文 × HEADING_FONT_RATIO 且不超过这个字数、不以句末标点结尾——不再要求列首/页顶
# （杂志正文中段的栏目标题「显影｜…」「专栏｜…」此前因不在列首漏判成正文）。
HEADING_MAX_CHARS = 40


def _classify(t: str) -> str | None:
    """署名 / 贡献 / 图注 / 链接 → 'byline'|'caption'|'link'；其它 None。"""
    t = t.strip()
    if not t:
        return None
    if _BYLINE_RE.search(t) or _CREDIT_RE.search(t):
        return "byline"
    if _LINK_RE.search(t):
        return "link"
    if _CAPTION_RE.search(t):
        return "caption"
    return None


def _split_inline_byline(t: str) -> tuple[str, str | None]:
    """导语与署名被 PDF 排在同一块时（"…一帆风顺文｜财新周刊 …"）从「文｜」处切开。"""
    m = re.search(r"(?<=[^\s｜|/／])(文|图|摄影)\s*[｜|/／]", t)
    if m and m.start() >= 6:
        return t[: m.start()].rstrip(), t[m.start() :].strip()
    return t, None


def _clean(t: str) -> str:
    """剥 PDF 私用字形映射出来的 "{{" / "}}" 垃圾（《财新》导播栏每行前缀）。"""
    return t.replace("{{", "").replace("}}", "").strip()


def has_k2pdfopt() -> bool:
    return shutil.which("k2pdfopt") is not None


def _columns(blocks: list[dict], page_w: float) -> list[list[dict]]:
    """分栏：只有存在一条**贯穿页面的竖向空白带**（页中部某 x 没有任何文字块横跨）才算双栏，按该 x 切左/右。
    ⚠ 不能按"块中点在中线左/右"分：单栏页里 PyMuPDF 的块宽窄不一（段末短行窄、多行段宽），中点法会把宽块
    全甩到"右栏"排到最后，阅读序被打乱、段落全断（真机《财新》坐实）。跨栏的宽块本身就是单栏（或跨栏标题）的证据。"""
    text_blocks = [b for b in blocks if b.get("type", 0) == 0 and b.get("lines")]
    if len(text_blocks) < 4:
        return [blocks]
    best = None
    step = max(page_w / 200, 1.0)
    x = page_w * 0.35
    while x <= page_w * 0.65:
        crossing = sum(1 for b in text_blocks if b["bbox"][0] < x - 2 and b["bbox"][2] > x + 2)
        if crossing == 0:
            left = [b for b in text_blocks if b["bbox"][2] <= x + 2]
            right = [b for b in text_blocks if b["bbox"][0] >= x - 2]
            if len(left) >= 2 and len(right) >= 2:
                # 取空白带最宽处：记录该 x 两侧最近块的距离
                gap = min(b["bbox"][0] for b in right) - max(b["bbox"][2] for b in left)
                if best is None or gap > best[1]:
                    best = (x, gap)
        x += step
    if best is None:
        return [blocks]
    split = best[0]
    left = [b for b in blocks if b.get("type", 0) != 0 and (b["bbox"][0] + b["bbox"][2]) / 2 < split or (b.get("type", 0) == 0 and b["bbox"][2] <= split + 2)]
    right = [b for b in blocks if b not in left]
    return [left, right]


def _is_cjk(ch: str) -> bool:
    return "一" <= ch <= "鿿" or ch in "，。！？；：、“”‘’（）《》〈〉【】…—"


def _block_text(block: dict) -> tuple[str, float, float]:
    """一个文字块 → (文本, 最大字号, 行高)。块内各行按语言拼接（中文不加空格，拉丁加空格）。"""
    lines_out: list[str] = []
    max_size = 0.0
    line_h = 0.0
    for line in block.get("lines", []):
        parts = []
        for span in line.get("spans", []):
            t = span.get("text", "")
            if t:
                parts.append(t)
                max_size = max(max_size, span.get("size", 0.0))
        if parts:
            # 剥零宽字符（​ 零宽空格 / ﻿ BOM / ­ 软连字符）：杂志 PDF 行尾常带 ​，会挡住句末标点判断
            lines_out.append("".join(parts).translate(_ZW).strip())
        bb = line.get("bbox")
        if bb:
            line_h = max(line_h, bb[3] - bb[1])
    text = ""
    for s in lines_out:
        if not s:
            continue
        text = _join(text, s)
    return _clean(text), max_size, line_h


def _join(a: str, b: str) -> str:
    """两段文字拼接：中文相邻不加空格；拉丁行尾连字符去掉直接接；其余加空格。"""
    if not a:
        return b
    if not b:
        return a
    if a.endswith("-") and not _is_cjk(b[0]):
        return a[:-1] + b
    if _is_cjk(a[-1]) or _is_cjk(b[0]):
        return a + b
    return a + " " + b


def _page_units(page: fitz.Page, imgdir: Path, page_no: int) -> list[dict]:
    """一页 → 阅读序单元列表：{k: text|h|fig, t, size, x0,x1,y0,y1, col0,col1, lh, page, html}。
    只做结构抽取，不做段落合并（合并在文档级 `_assemble_paragraphs`，因为要跨列跨页续接）。"""
    d = page.get_text("dict")
    page_w = d.get("width", page.rect.width)
    page_h = d.get("height", page.rect.height)
    blocks = d.get("blocks", [])
    # 正文字号 = 按字符数加权最多的那档（比中位鲁棒：稀疏页里标题不会把基准抬高）。
    size_chars: dict[int, int] = {}
    for b in blocks:
        if b.get("type", 0) != 0:
            continue
        for ln in b.get("lines", []):
            for sp in ln.get("spans", []):
                sz = round(sp.get("size", 0.0))
                if sz > 0:
                    size_chars[sz] = size_chars.get(sz, 0) + len(sp.get("text", ""))
    body_size = float(max(size_chars, key=lambda k: size_chars[k])) if size_chars else 0.0
    units: list[dict] = []
    for col in _columns(blocks, page_w):
        tb = [b for b in col if b.get("type", 0) == 0 and b.get("lines")]
        # 列左沿取 x0 的**中位数**（不用最小值：页眉/引语/页码等更靠左的块会把左沿拉偏，导致每行都被判"缩进"开新段）
        xs = sorted(b["bbox"][0] for b in tb)
        col0 = xs[len(xs) // 2] if xs else 0.0
        col1 = max((b["bbox"][2] for b in tb), default=page_w)
        first_text = True
        for b in sorted(col, key=lambda b: (round(b["bbox"][1] / 4), b["bbox"][0])):  # 列内按 y 再 x
            x0, y0, x1, y1 = b["bbox"]
            # 页眉/页脚：贴页顶或页底的短文字（刊名·日期·页码）不进正文，也不当标题
            if b.get("type", 0) == 0 and (y1 < page_h * 0.06 or y0 > page_h * 0.94):
                if len("".join(sp.get("text", "") for ln in b.get("lines", []) for sp in ln.get("spans", []))) <= 40:
                    continue
            if b.get("type", 0) == 1:  # 图块：裁原区当整块保留（不切，含图/公式/表），尺寸封顶
                bbox = fitz.Rect(b["bbox"])
                if bbox.width < 30 or bbox.height < 30:
                    continue  # 图标/装饰级小图不进正文
                scale = min(2.0, FIG_MAX_W / bbox.width, FIG_MAX_H / bbox.height)
                pix = page.get_pixmap(clip=bbox, matrix=fitz.Matrix(scale, scale))
                # 照片存 JPEG（PNG 一张 954 宽照片 ≈1.3MB，39 张就 21MB；JPEG q80 约 1/8）；编码不可用回退 PNG
                name = f"p{page_no}_{len(units)}.jpg"
                try:
                    (imgdir / name).write_bytes(pix.tobytes("jpeg", jpg_quality=80))
                except Exception:  # noqa: BLE001
                    name = name[:-4] + ".png"
                    pix.save(str(imgdir / name))
                units.append({"k": "fig", "t": "", "size": 0.0, "x0": x0, "x1": x1, "y0": y0, "y1": y1, "col0": col0, "col1": col1, "lh": 0.0, "page": page_no, "html": f'<p class="fig"><img src="../images/{name}" alt=""/></p>'})
                continue
            text, max_size, lh = _block_text(b)
            if not text:
                continue
            big = body_size > 0 and max_size >= body_size * HEADING_FONT_RATIO
            cls = _classify(text)
            # 紧跟图块的短行（≤60 字、无句末标点）= 图注
            if cls is None and units and units[-1]["k"] == "fig" and len(text) <= 60 and not _terminal(text):
                cls = "caption"
            # 标题：字号显著大 且 ((列首 或 页顶区 且 ≤60 字) 或 (≤HEADING_MAX_CHARS 字且无句末标点))；
            # 署名/贡献/图注/链接、冒号结尾（「本刊产业新闻部：」）一律不算标题——它们曾混进目录（2026-09-05 用户指出）
            is_h = big and cls is None and not text.rstrip().endswith(("：", ":")) and (
                ((first_text or y0 < page_h * HEADING_TOP_FRAC) and len(text) <= 60) or (len(text) <= HEADING_MAX_CHARS and not _terminal(text))
            )
            units.append({"k": "h" if is_h else "text", "cls": None if is_h else cls, "t": text, "size": max_size, "x0": x0, "x1": x1, "y0": y0, "y1": y1, "col0": col0, "col1": col1, "lh": lh or max_size * 1.2, "page": page_no, "html": ""})
            first_text = False
    return units


def _terminal(t: str) -> bool:
    t = t.rstrip()
    return bool(t) and t[-1] in _TERMINAL


def _assemble_paragraphs(units: list[dict]) -> list[dict]:
    """文档级段落合并。规则：
    - 同列且竖向间距 ≤ 1.6 行高 → 续接，除非有"新段信号"：本块首行缩进（x0 比列左沿多 >1.2 字）、
      或上段最后一行是短行且以句末标点结束。
    - 换列 / 换页 → 仅当上段"未完"（不以句末标点结束，或最后一行顶到列右沿）且本块不缩进时续接。
    - 标题 / 图 不参与合并，且打断合并。"""
    out: list[dict] = []
    for u in units:
        if u["k"] != "text":
            out.append(u)
            continue
        prev = out[-1] if out else None
        joined = False
        if prev is not None and prev["k"] == "text" and abs(prev["size"] - u["size"]) <= 1.5 and not u.get("cls") and not prev.get("cls"):
            sz = max(u["size"], 1.0)
            same_col = prev["page"] == u["page"] and abs(prev["col0"] - u["col0"]) < 5
            if same_col:
                # 首行缩进相对**上一行的起点**判（不是列左沿）：段首比上一行右移 ≥1 字；段内第 2 行反而比缩进的首行靠左 → 续接
                indented = (u["x0"] - prev["lx0"]) > 1.0 * sz
                gap = u["y0"] - prev["y1"]
                short_last = prev["x1"] < prev["col1"] - 1.5 * max(prev["size"], 1.0)
                new_para = indented or (short_last and _terminal(prev["t"]))
                if gap <= 1.6 * max(prev["lh"], u["lh"], 1.0) and not new_para:
                    joined = True
            else:
                indented = (u["x0"] - u["col0"]) > 1.0 * sz  # 换列/换页无上一行可比，用列左沿（中位数）
                unfinished = (not _terminal(prev["t"])) or prev["x1"] >= prev["col1"] - 1.5 * max(prev["size"], 1.0)
                if unfinished and not indented:
                    joined = True
        if joined:
            prev["t"] = _join(prev["t"], u["t"])
            prev.update({"x1": u["x1"], "y1": u["y1"], "col1": u["col1"], "col0": u["col0"], "page": u["page"], "lh": u["lh"], "lx0": u["x0"]})
        else:
            nu = dict(u)
            nu["lx0"] = u["x0"]  # 最后一行的起点（缩进判断用）
            out.append(nu)
    return _mark_subheads(out)


def _mark_subheads(paras: list[dict]) -> list[dict]:
    """文档级：短、无句末标点、字号略大于全书正文字号的独立文本 → 小标题（k='h3'，不分章、不进目录）。
    正文字号 = 全书按字数加权最多的那档。"""
    size_chars: dict[int, int] = {}
    for p in paras:
        if p["k"] == "text" and not p.get("cls"):
            size_chars[round(p["size"])] = size_chars.get(round(p["size"]), 0) + len(p["t"])
    if not size_chars:
        return paras
    body = float(max(size_chars, key=lambda k: size_chars[k]))
    for p in paras:
        if p["k"] == "text" and not p.get("cls") and len(p["t"]) <= SUBHEAD_MAX_CHARS and not _terminal(p["t"]) and p["size"] >= body + SUBHEAD_SIZE_DELTA:
            p["k"] = "h3"
    return paras


def _demote_section_heads(paras: list[dict]) -> list[dict]:
    """标题分档（2026-09-06 《财新》实测：刊头 25.5 / 文章题 19.5 / 节题 16.5）：有 ≥3 个字号档时，
    第二大档及以上才分章进目录，其余降为 h3（节题不翻页、不进目录——否则 10 章变 45 章、每节末留白）。"""
    sizes = sorted({round(p["size"] * 2) / 2 for p in paras if p["k"] == "h"}, reverse=True)
    if len(sizes) < 3:
        return paras
    threshold = sizes[1] - 0.3
    for p in paras:
        if p["k"] == "h" and p["size"] < threshold:
            p["k"] = "h3"
    return paras


def _chapters(paras: list[dict], n_pages: int) -> list[tuple[str, str]]:
    """按标题分章（标题开新章）；全书无标题则每 PAGES_PER_CHAPTER_FALLBACK 页一章。返回 [(章名, body_html)]。"""
    paras = _demote_section_heads(paras)
    has_h = any(p["k"] == "h" for p in paras)
    chapters: list[tuple[str, list[str]]] = []
    cur_title = "开头"
    cur: list[str] = []
    cur_start_page = 0

    def flush():
        nonlocal cur, cur_title
        if cur:
            chapters.append((cur_title, cur))
        cur = []

    def cur_chars() -> int:
        return sum(len(re.sub(r"<[^>]+>", "", x)) for x in cur)

    for p in paras:
        if p["k"] == "h":
            if cur and cur_chars() >= 200:  # 标题开新章；正文不足 200 字的迷你章（封面/栏目页）并入下一章，免整屏空白
                flush()
            cur_title = p["t"][:40]
            cur_start_page = p["page"]
            cur.append(f"<h2>{html.escape(p['t'])}</h2>")
        elif p["k"] == "fig":
            cur.append(p["html"])
        elif p["k"] == "h3":
            cur.append(f"<h3>{html.escape(p['t'])}</h3>")
        else:
            if not has_h and cur and p["page"] - cur_start_page >= PAGES_PER_CHAPTER_FALLBACK:
                flush()
                cur_title = f"第 {p['page'] + 1} 页起"
                cur_start_page = p["page"]
            cls = p.get("cls")
            if cls:
                cur.append(f'<p class="{cls}">{html.escape(p["t"])}</p>')
                continue
            body_t, byline = _split_inline_byline(p["t"])
            cur.append(f"<p>{html.escape(body_t)}</p>")
            if byline:
                cur.append(f'<p class="byline">{html.escape(byline)}</p>')
    flush()
    if not chapters:
        chapters.append(("正文", ["<p>&#160;</p>"]))
    return [(t, "\n".join(body)) for t, body in chapters]


def _build_epub(chapters: list[tuple[str, str]], imgdir: Path, title: str, out: Path) -> None:
    """极简 EPUB3（共享骨架 epub_skel）：每章一个 xhtml，图片在 images/。产物随后由上层 wash/epub-optimize 统一优化。"""
    css = "@page{margin:0}body{margin:0}img{max-width:100%}.fig{text-align:center;margin:0}"
    css += "h3{font-size:1.1em;margin:1em 0 .3em}.byline{font-size:.9em;margin:0 0 .8em}.caption{font-size:.85em;text-align:center;margin:0 0 .8em}.link{font-size:.8em;word-break:break-all}"
    css += f"/* Move {W_PX}x{H_PX} */"  # 竖向 CSS 提示（认 CSS 的阅读器用；xochitl 忽略、走自身列宽）
    imgs = sorted(p for p in imgdir.iterdir() if p.suffix in (".png", ".jpg"))
    write_epub(out, title, [Chapter(t, body) for t, body in chapters], css=css, uid=f"shelf-reflow-{title}", images=imgs)


def _reflow_scanned(src: Path, out_pdf: Path) -> bool:
    """扫描件：k2pdfopt 位图重排（缺则回退 pdf_crop_move.py 裁边）。返回是否产出。"""
    if has_k2pdfopt():
        # -w/-h 目标屏、-mode fw(fit width) 保图整块、-x 退出不等按键、-o 输出
        r = subprocess.run(["k2pdfopt", str(src), "-w", str(W_PX), "-h", str(H_PX), "-mode", "fw", "-x", "-o", str(out_pdf)], check=False, capture_output=True, text=True)
        return r.returncode == 0 and out_pdf.is_file()
    # 回退裁边（同目录 pdf_crop_move.py）
    crop = Path(__file__).resolve().parent / "pdf_crop_move.py"
    r = subprocess.run([sys.executable, str(crop), str(src), str(out_pdf)], check=False, capture_output=True, text=True)
    return r.returncode in (0,) and out_pdf.is_file()


def main() -> int:
    if len(sys.argv) != 3:
        print("用法: pdf_reflow_move.py <输入.pdf> <输出目录>", file=sys.stderr)
        return 2
    src, outdir = Path(sys.argv[1]), Path(sys.argv[2])
    outdir.mkdir(parents=True, exist_ok=True)
    try:
        doc = fitz.open(str(src))
    except Exception as e:  # noqa: BLE001
        print(f"打开 PDF 失败: {e}", file=sys.stderr)
        return 2
    n = doc.page_count or 1
    total_chars = sum(len(doc[i].get_text("text")) for i in range(min(n, 10)))  # 采样前 10 页
    born_digital = (total_chars / min(n, 10)) >= MIN_CHARS_PER_PAGE
    title = src.stem
    if born_digital:
        imgdir = outdir / "_img"
        imgdir.mkdir(exist_ok=True)
        units: list[dict] = []
        for i in range(n):
            units.extend(_page_units(doc[i], imgdir, i))
        paras = _assemble_paragraphs(units)
        chapters = _chapters(paras, n)
        out = outdir / f"{title}.epub"
        _build_epub(chapters, imgdir, title, out)
        shutil.rmtree(imgdir, ignore_errors=True)
        body_paras = [p for p in paras if p["k"] == "text" and not p.get("cls")]
        unterminated = sum(1 for p in body_paras if not _terminal(_split_inline_byline(p["t"])[0]))
        stats = {"out": str(out), "kind": "epub", "chapters": len(chapters), "paragraphs": len(body_paras),
                 "subheads": sum(1 for p in paras if p["k"] == "h3"), "classified": sum(1 for p in paras if p.get("cls")),
                 "unterminated": unterminated, "unterminated_pct": round(100.0 * unterminated / max(1, len(body_paras)), 1)}
        print(json.dumps(stats, ensure_ascii=False))
        return 0
    out_pdf = outdir / f"{title}.reflow.pdf"
    if _reflow_scanned(src, out_pdf):
        print(json.dumps({"out": str(out_pdf), "kind": "pdf"}))
        return 0
    print("扫描件重排失败（无 k2pdfopt 且裁边未产出）", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
