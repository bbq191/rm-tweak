"""极简 EPUB3 骨架（纯 stdlib）：pdf_reflow_move / txt_to_epub / render_probe 三处各写一遍 mimetype + container + opf +
nav + 逐章 xhtml 的重复收编于此（2026-09-06 体检）。布局固定：`OEBPS/content.opf · nav.xhtml · <css_name> · text/cN.xhtml ·
images/*`；目录两级（`Chapter.level` 1/2，2 级挂在前一个 1 级之下，没有父级就按 1 级）。产物随后由 wash/epub-optimize
统一优化，这里不注排版细节。"""
from __future__ import annotations

import html
import zipfile
from dataclasses import dataclass
from pathlib import Path

CONTAINER_XML = '<?xml version="1.0"?>\n<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>'
MEDIA_TYPES = {".jpg": "image/jpeg", ".jpeg": "image/jpeg", ".png": "image/png", ".gif": "image/gif", ".svg": "image/svg+xml", ".webp": "image/webp"}


@dataclass
class Chapter:
    title: str
    body: str  # `<body>` 内的 XHTML 片段（调用方已转义）
    level: int = 1


def chapter_xhtml(title: str, body: str, css_href: str = "") -> str:
    link = f'<link rel="stylesheet" type="text/css" href="{css_href}"/>' if css_href else ""
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n<html xmlns="http://www.w3.org/1999/xhtml"><head><meta charset="utf-8"/>'
        f"<title>{html.escape(title)}</title>{link}</head><body>\n{body}\n</body></html>"
    )


def nav_items(entries: list[tuple[str, str, int]]) -> str:
    """(href, title, level) → `<li>` 序列（两级；不会产出空 `<ol></ol>`）。"""
    out: list[str] = []
    parent_open = sub_open = False
    for href, title, level in entries:
        li = f'<li><a href="{href}">{html.escape(title)}</a>'
        if level >= 2 and parent_open:
            if not sub_open:
                out.append("<ol>")
                sub_open = True
            out.append(li + "</li>")
        else:
            if parent_open:
                out.append("</ol></li>" if sub_open else "</li>")
            out.append(li)
            parent_open, sub_open = True, False
    if parent_open:
        out.append("</ol></li>" if sub_open else "</li>")
    return "".join(out)


def write_epub(out: Path, title: str, chapters: list[Chapter], *, css: str | None = None, css_name: str = "style.css", author: str = "", lang: str = "zh", uid: str | None = None, images: list[Path] | tuple[Path, ...] = ()) -> Path:
    if not chapters:
        raise ValueError("EPUB 至少要一章")
    out.parent.mkdir(parents=True, exist_ok=True)
    manifest = ['<item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>']
    if css is not None:
        manifest.append(f'<item id="css" href="{css_name}" media-type="text/css"/>')
    spine, entries, files = [], [], {}
    for i, c in enumerate(chapters, 1):
        fname = f"text/c{i}.xhtml"
        files[fname] = chapter_xhtml(c.title, c.body, f"../{css_name}" if css is not None else "")
        manifest.append(f'<item id="c{i}" href="{fname}" media-type="application/xhtml+xml"/>')
        spine.append(f'<itemref idref="c{i}"/>')
        entries.append((fname, c.title, c.level))
    imgs = [Path(p) for p in images]
    for i, img in enumerate(imgs, 1):
        manifest.append(f'<item id="img{i}" href="images/{img.name}" media-type="{MEDIA_TYPES.get(img.suffix.lower(), "application/octet-stream")}"/>')
    creator = f"<dc:creator>{html.escape(author)}</dc:creator>" if author else ""
    opf = (
        '<?xml version="1.0" encoding="UTF-8"?>\n<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bid">'
        f'<metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:identifier id="bid">{html.escape(uid or f"shelf:{title}")}</dc:identifier>'
        f"<dc:title>{html.escape(title)}</dc:title>{creator}<dc:language>{lang}</dc:language></metadata>"
        f'<manifest>{"".join(manifest)}</manifest><spine>{"".join(spine)}</spine></package>'
    )
    nav = (
        '<?xml version="1.0" encoding="UTF-8"?>\n<html xmlns="http://www.w3.org/1999/xhtml" xmlns:epub="http://www.idpf.org/2007/ops">'
        f'<head><meta charset="utf-8"/><title>目录</title></head><body><nav epub:type="toc"><ol>{nav_items(entries)}</ol></nav></body></html>'
    )
    with zipfile.ZipFile(out, "w") as z:
        z.writestr("mimetype", "application/epub+zip", compress_type=zipfile.ZIP_STORED)  # 规范：首个条目且不压缩
        z.writestr("META-INF/container.xml", CONTAINER_XML, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("OEBPS/content.opf", opf, compress_type=zipfile.ZIP_DEFLATED)
        z.writestr("OEBPS/nav.xhtml", nav, compress_type=zipfile.ZIP_DEFLATED)
        if css is not None:
            z.writestr(f"OEBPS/{css_name}", css, compress_type=zipfile.ZIP_DEFLATED)
        for fname, data in files.items():
            z.writestr(f"OEBPS/{fname}", data, compress_type=zipfile.ZIP_DEFLATED)
        for img in imgs:
            z.writestr(f"OEBPS/images/{img.name}", img.read_bytes(), compress_type=zipfile.ZIP_STORED)  # 已压缩图不再 deflate
    return out
