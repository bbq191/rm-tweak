"""漫画识别（不调 Calibre 的廉价探针）：决定 `shelf push` 走文字书洗书路还是漫画路（CBZ + PDF）。

- `.cbz`：天然漫画。
- `.azw3/.mobi/.azw/.prc`（PalmDB 容器）：数以 JPEG/PNG 魔数开头的记录，**图片记录字节占文件 ≥ 60% 且 ≥ 20 张**判漫画
  （漫画 KF8 几乎全是图片记录，文字书图片只占零头；不解 KF8、不解压文本，毫秒级）。
- `.epub`：按 OPF spine 统计全书 `<img>` 数与可见文字数：图 ≥ 20 张且**平均每张图配的文字 < 40 字**判漫画
  （Calibre 洗过的漫画 EPUB 一页 xhtml 塞十几张图、几乎无字；文字书是几百字配零星插图）。
其余格式不判（False）。判错可用 `--comic / --no-comic` 手动覆盖。
"""
from __future__ import annotations

import posixpath
import re
import struct
import zipfile
from pathlib import Path

MIN_PAGES = 20
PALM_IMAGE_RATIO = 0.6
EPUB_TEXT_PER_IMAGE = 40
_JPEG = b"\xff\xd8\xff"
_PNG = b"\x89PNG"


def palmdb_image_ratio(data: bytes) -> tuple[int, float]:
    """(图片记录数, 图片字节占比)。非 PalmDB → (0, 0)。"""
    if len(data) < 78:
        return 0, 0.0
    nrec = struct.unpack(">H", data[76:78])[0]
    if nrec == 0 or len(data) < 78 + nrec * 8:
        return 0, 0.0
    offs = [struct.unpack(">I", data[78 + i * 8: 82 + i * 8])[0] for i in range(nrec)] + [len(data)]
    n = 0
    img_bytes = 0
    for a, b in zip(offs, offs[1:]):
        if a < b <= len(data) and (data[a:a + 3] == _JPEG or data[a:a + 4] == _PNG):
            n += 1
            img_bytes += b - a
    return n, (img_bytes / len(data) if data else 0.0)


def epub_image_stats(z: zipfile.ZipFile) -> tuple[int, int]:
    """(spine 页里 <img>/<image> 总数, 可见文字总字数)。"""
    try:
        container = z.read("META-INF/container.xml").decode("utf-8", "ignore")
        opf_path = re.search(r'full-path="([^"]+)"', container).group(1)
        opf = z.read(opf_path).decode("utf-8", "ignore")
    except Exception:  # noqa: BLE001
        return 0, 0
    opf_dir = posixpath.dirname(opf_path)
    manifest = {}
    for m in re.finditer(r"<item\b[^>]*>", opf):
        mid = re.search(r'\bid="([^"]+)"', m.group(0))
        href = re.search(r'\bhref="([^"]+)"', m.group(0))
        if mid and href:
            manifest[mid.group(1)] = href.group(1)
    names = set(z.namelist())
    images = 0
    text = 0
    for idref in re.findall(r'<itemref[^>]*\bidref="([^"]+)"', opf):
        href = manifest.get(idref)
        if not href:
            continue
        page = posixpath.normpath(posixpath.join(opf_dir, href)) if opf_dir else href
        if page.lower().endswith((".jpg", ".jpeg", ".png", ".gif", ".webp")):
            images += 1
            continue
        if page not in names:
            continue
        html = z.read(page).decode("utf-8", "ignore")
        images += len(re.findall(r"<(?:img|image)\b", html, re.I))
        body = re.sub(r"(?is)<(script|style|head)\b.*?</\1>", "", html)
        text += len(re.sub(r"\s+", "", re.sub(r"<[^>]+>", "", body)))
    return images, text


def is_comic(path: Path) -> bool:
    suf = path.suffix.lower()
    if suf == ".cbz":
        return True
    if suf in (".azw3", ".mobi", ".azw", ".prc"):
        n, ratio = palmdb_image_ratio(path.read_bytes())
        return n >= MIN_PAGES and ratio >= PALM_IMAGE_RATIO
    if suf == ".epub":
        try:
            with zipfile.ZipFile(path) as z:
                images, text = epub_image_stats(z)
        except zipfile.BadZipFile:
            return False
        return images >= MIN_PAGES and text / images < EPUB_TEXT_PER_IMAGE
    return False
