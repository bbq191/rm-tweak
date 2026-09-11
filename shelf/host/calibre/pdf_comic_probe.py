"""pdf_comic_probe.py —— PDF 是否漫画的探针（pymupdf 子进程，被 `shelf_cli/comic.py` 经
`calibre_bridge.py::pdf_comic_stats()` 调用）：抽样统计"有图且几乎无文字"的页占比，跟
`shelf_cli/comic.py` 里 PalmDB/EPUB 那两条判定共用同一套阈值（MIN_PAGES=20、图片页占比 ≥0.6）。
PDF 判定做不到其余分支那种"零依赖毫秒级"（要真正打开、逐页解析），所以单独拆成子进程脚本，
不直接塞进 `comic.py`——保持它其余分支"廉价探针"的承诺不被拖慢、也不强制装 pymupdf。

用法: python3 shelf/host/calibre/pdf_comic_probe.py 输入.pdf
输出（末行 JSON，跟 pdf_reflow_move.py 等同一契约）: {"pages": 总页数, "ratio": 抽样页里"图片页"占比}
只用 pymupdf，见 shelf_cli/calibre_bridge.py::py_with_pymupdf()。
"""
from __future__ import annotations

import json
import sys

try:
    import pymupdf as fitz  # 新名（PyMuPDF ≥1.24）
except ImportError:
    import fitz  # type: ignore[no-redef]

TEXT_PER_PAGE = 40  # 页面可见字符数低于这个值且至少有一张图，判"图片页"（跟 comic.py 的 EPUB_TEXT_PER_IMAGE 同一档）
SAMPLE_CAP = 24  # 大部头只均匀抽样，不逐页全扫；跟 pdf_reflow_move.py 采样前 10 页判 born-digital 是同类取舍


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    try:
        doc = fitz.open(sys.argv[1])
    except Exception as e:  # noqa: BLE001
        print(f"打不开：{e}", file=sys.stderr)
        return 1
    n = doc.page_count or 0
    if n == 0:
        print(json.dumps({"pages": 0, "ratio": 0.0}))
        return 0
    if n <= SAMPLE_CAP:
        idxs = list(range(n))
    else:
        idxs = [round(i * (n - 1) / (SAMPLE_CAP - 1)) for i in range(SAMPLE_CAP)]  # 均匀抽样，含首尾
    image_pages = 0
    for i in idxs:
        page = doc[i]
        text_len = len(page.get_text("text"))
        has_image = len(page.get_images()) > 0
        if has_image and text_len < TEXT_PER_PAGE:
            image_pages += 1
    print(json.dumps({"pages": n, "ratio": image_pages / len(idxs)}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
