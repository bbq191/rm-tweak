"""漫画 AZW3/MOBI/EPUB → CBZ：calibre 解包成 EPUB 中转，按 OPF spine 顺序抽
每页图片，零填充序号打包 CBZ。KOReader 漫画体验以 CBZ 最佳（AZW3 漫画类
KF8 在 KOReader/crengine 下支持不佳，2026-09-02 用户真机反馈）。**漫画不投原生**
（2026-09-05 用户定）：`shelf push` 判为漫画的书只出 CBZ 进母版库、加入 KOReader。

用法: python3 shelf/host/calibre/comic2cbz.py 输入.azw3 [输出.cbz]
只用 stdlib + ebook-convert，不需要 pymupdf。
⚠ 用系统 python3，别用 `uv run`：ebook-convert shebang 是 `env python3`，
uv venv 会把它劫进 3.12 环境导致 calibre 启动即炸（2026-09-02 踩过）。
"""

from __future__ import annotations

import os
import posixpath
import re
import subprocess
import sys
import tempfile
import zipfile

IMG_EXTS = (".jpg", ".jpeg", ".png", ".gif", ".webp")


def spine_ordered_images(epub: zipfile.ZipFile) -> list[str]:
    # 2026-09-09 审计修：跟 shelf_cli/comic.py::epub_image_stats 一样包一层 try/except——中转 EPUB
    # 缺 META-INF/container.xml、或 container.xml 没有 full-path 属性时，原来会在这里抛
    # KeyError/AttributeError，冒到 main() 变成裸 traceback 甩给用户。解析失败按"抽不到图"处理，
    # main() 现成的 `len(images) < 3` 分支会给出"不像漫画，放弃"这句人话，不用另外处理。
    try:
        container = epub.read("META-INF/container.xml").decode("utf-8", "ignore")
        m = re.search(r'full-path="([^"]+)"', container)
        if not m:
            return []
        opf_path = m.group(1)
        opf = epub.read(opf_path).decode("utf-8", "ignore")
    except (KeyError, zipfile.BadZipFile):
        return []
    opf_dir = posixpath.dirname(opf_path)

    manifest: dict[str, str] = {}
    for m in re.finditer(r"<item\b[^>]*>", opf):
        tag = m.group(0)
        mid = re.search(r'\bid="([^"]+)"', tag)
        href = re.search(r'\bhref="([^"]+)"', tag)
        if mid and href:
            manifest[mid.group(1)] = href.group(1)
    spine = re.findall(r'<itemref[^>]*\bidref="([^"]+)"', opf)

    names = set(epub.namelist())
    images: list[str] = []
    seen: set[str] = set()
    for idref in spine:
        href = manifest.get(idref)
        if not href:
            continue
        page = posixpath.normpath(posixpath.join(opf_dir, href)) if opf_dir else href
        if page.lower().endswith(IMG_EXTS):
            srcs = [page]
        elif page in names:
            html = epub.read(page).decode("utf-8", "ignore")
            page_dir = posixpath.dirname(page)
            srcs = [posixpath.normpath(posixpath.join(page_dir, s)) if page_dir else s
                    for s in re.findall(r'<(?:img|image)[^>]*(?:src|xlink:href)="([^"]+)"', html)]
        else:
            continue
        for s in srcs:
            if s in names and s.lower().endswith(IMG_EXTS) and s not in seen:
                seen.add(s)
                images.append(s)
    return images


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    src = sys.argv[1]
    out = sys.argv[2] if len(sys.argv) > 2 else re.sub(r"\.[^.]+$", "", src) + ".cbz"

    # 中转文件放输出目录旁（裸 /tmp 在沙箱环境可能不可写）
    with tempfile.TemporaryDirectory(dir=os.path.dirname(os.path.abspath(out)) or ".") as td:
        mid = os.path.join(td, "mid.epub")
        if src.lower().endswith(".epub"):
            mid = src
        else:
            r = subprocess.run(["ebook-convert", src, mid], capture_output=True, text=True)
            if r.returncode != 0:
                print(r.stderr[-1500:], file=sys.stderr)
                return 1
        epub = zipfile.ZipFile(mid)
        images = spine_ordered_images(epub)
        if len(images) < 3:
            print(f"只抽到 {len(images)} 张图，不像漫画，放弃", file=sys.stderr)
            return 1
        with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as cbz:  # 图片已压缩，STORED 即可
            for i, name in enumerate(images):
                ext = posixpath.splitext(name)[1].lower().replace(".jpeg", ".jpg")
                cbz.writestr(f"{i:04d}{ext}", epub.read(name))
    print(f"产物: {out}（{len(images)} 页）")
    return 0


if __name__ == "__main__":
    sys.exit(main())
