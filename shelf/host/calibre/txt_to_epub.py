"""中文 TXT（网文主格式）→ 带目录的 EPUB（纯 stdlib），随后交 wash_epub.sh 收尾（Calibre 深洗 + epub-optimize + 体检）。

- 编码：utf-8-sig → utf-16（BOM）→ gb18030（严格）→ utf-8 替换；结果写进 JSON。
- 段落：一行一段（网文惯例）；行首全角空格/nbsp/空白剥掉（缩进交 `p{text-indent:2em}`，不烘全角空格——xochitl 会吞、
  KOReader 随字体变宽，见书架白皮书 §03y）；空行只当分隔。
- 章节：`第X卷/部/集`→卷（一级），`第X章/回/节/话`→章（有卷时二级）；`序章/楔子/尾声/番外…`按章；标题行 ≤40 字、
  正文里"第三章说过……"这种长句不算。一个章节没认出 → 每 8000 字硬切「第 N 部分」并在 JSON 里 `detected:false`。
- 书名/作者：文件名 `书名 - 作者` / `书名（作者）` / `书名`。
用法: python3 txt_to_epub.py <in.txt> <输出目录>   → 末行 JSON {"out","chapters","volumes","encoding","detected","title","author"}
"""
from __future__ import annotations

import html
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from epub_skel import Chapter, write_epub  # noqa: E402  共享 EPUB 骨架

_NUM = r"[零〇一二三四五六七八九十百千两0-9０-９]{1,8}"
_SEP = r"\s*[:：、.．\-—－]?\s*"
VOL_RE = re.compile(rf"^(第{_NUM}[卷部集]){_SEP}(.{{0,30}}?)\s*$")
CH_RE = re.compile(rf"^(第{_NUM}[章回节話话]){_SEP}(.{{0,30}}?)\s*$")
SPECIAL_RE = re.compile(r"^(序章|序言|序幕|楔子|前言|引子|尾声|尾聲|终章|終章|后记|後記|番外.{0,20}|外传.{0,20}|外傳.{0,20})\s*$")
HEADING_MAX_CHARS = 40
HARD_SPLIT_CHARS = 8000
_LEAD = "　  \t"
_ZW = dict.fromkeys(map(ord, "​‌‍﻿"), None)


def decode(data: bytes) -> tuple[str, str]:
    if data.startswith(b"\xef\xbb\xbf"):
        return data.decode("utf-8-sig", "replace"), "utf-8-sig"
    if data[:2] in (b"\xff\xfe", b"\xfe\xff"):
        return data.decode("utf-16", "replace"), "utf-16"
    for enc in ("utf-8", "gb18030"):
        try:
            return data.decode(enc), enc
        except UnicodeDecodeError:
            continue
    return data.decode("utf-8", "replace"), "utf-8?"


def classify(line: str) -> tuple[str, str] | None:
    """标题行 → ('vol'|'ch', 标题文本)；否则 None。"""
    s = line.strip(_LEAD).translate(_ZW).strip()
    if not s or len(s) > HEADING_MAX_CHARS:
        return None
    m = VOL_RE.match(s)
    if m:
        return "vol", (m.group(1) + (" " + m.group(2) if m.group(2) else ""))
    m = CH_RE.match(s)
    if m:
        return "ch", (m.group(1) + (" " + m.group(2) if m.group(2) else ""))
    if SPECIAL_RE.match(s):
        return "ch", s
    return None


def split_chapters(text: str) -> tuple[list[dict], bool]:
    """→ [{kind:'vol'|'ch', title, paras:[...]}]，detected。开头正文（无标题）归「开头」。"""
    chapters: list[dict] = []
    cur = {"kind": "ch", "title": "", "paras": []}
    for raw in text.splitlines():
        line = raw.translate(_ZW).rstrip()
        c = classify(line)
        if c:
            if cur["paras"] or cur["title"]:
                chapters.append(cur)
            cur = {"kind": c[0], "title": c[1], "paras": []}
            continue
        p = line.strip(_LEAD).strip()
        if p:
            cur["paras"].append(p)
    if cur["paras"] or cur["title"]:
        chapters.append(cur)
    detected = any(c["title"] for c in chapters)
    if not detected:
        paras = [p for c in chapters for p in c["paras"]]
        chapters, buf, n = [], [], 0
        for p in paras:
            buf.append(p)
            n += len(p)
            if n >= HARD_SPLIT_CHARS:
                chapters.append({"kind": "ch", "title": f"第 {len(chapters) + 1} 部分", "paras": buf})
                buf, n = [], 0
        if buf:
            chapters.append({"kind": "ch", "title": f"第 {len(chapters) + 1} 部分", "paras": buf})
    chapters = drop_contents_listing(chapters)
    for c in chapters:
        if not c["title"]:
            c["title"] = "开头"
    return chapters, detected


_TRAIL_PAGE = re.compile(r"[\s　·.…]*\d{1,4}\s*$")


def drop_contents_listing(chapters: list[dict]) -> list[dict]:
    """TXT 开头常带一份目录（"第一部　一天的國王　1"…每行一个标题+页码）：这些行会被当成一串**空章**。
    规则：没有正文的章，其标题（去掉尾部页码）在后面再次出现 → 是目录行，丢掉；有正文的卷/章节点原样保留。"""
    def norm(t: str) -> str:
        return _TRAIL_PAGE.sub("", t).strip()

    positions: dict[str, list[int]] = {}
    for i, c in enumerate(chapters):
        positions.setdefault(norm(c["title"]), []).append(i)
    keep = []
    for i, c in enumerate(chapters):
        if not c["paras"] and c["title"] and any(j > i for j in positions[norm(c["title"])]):
            continue
        keep.append(c)
    return keep


def title_author(stem: str) -> tuple[str, str]:
    s = stem.strip()
    if " - " in s:
        t, a = s.split(" - ", 1)
        return t.strip(), a.strip()
    m = re.match(r"^(.*?)\s*[（(]([^（）()]{1,30})[）)]\s*$", s)
    if m:
        return m.group(1).strip() or s, m.group(2).strip()
    return s, ""


def build_epub(chapters: list[dict], title: str, author: str, out: Path) -> None:
    """卷 → h1 / 1 级目录；章 → 有卷时 h2 / 2 级，否则 h1 / 1 级（共享骨架 epub_skel）。"""
    has_vol = any(c["kind"] == "vol" for c in chapters)
    chs = []
    for c in chapters:
        is_vol = c["kind"] == "vol"
        tag = "h1" if (is_vol or not has_vol) else "h2"
        body = f"<{tag}>{html.escape(c['title'])}</{tag}>\n" + "\n".join(f"<p>{html.escape(p)}</p>" for p in c["paras"])
        chs.append(Chapter(c["title"], body, level=2 if (has_vol and not is_vol) else 1))
    write_epub(out, title, chs, author=author, uid=f"shelf-txt-{title}")


def convert(src: Path, outdir: Path) -> dict:
    text, enc = decode(src.read_bytes())
    chapters, detected = split_chapters(text)
    if not chapters:
        raise SystemExit("TXT 里没有正文")
    title, author = title_author(src.stem)
    out = outdir / f"{src.stem}.epub"
    outdir.mkdir(parents=True, exist_ok=True)
    build_epub(chapters, title, author, out)
    return {"out": str(out), "chapters": sum(1 for c in chapters if c["kind"] == "ch"), "volumes": sum(1 for c in chapters if c["kind"] == "vol"), "encoding": enc, "detected": detected, "title": title, "author": author}


def main() -> int:
    if len(sys.argv) != 3:
        print("用法: txt_to_epub.py <in.txt> <输出目录>", file=sys.stderr)
        return 2
    print(json.dumps(convert(Path(sys.argv[1]), Path(sys.argv[2])), ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
