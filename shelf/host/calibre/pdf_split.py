"""大 PDF 按页数均分成 N 卷（pymupdf）。xochitl `/upload` 有体积上限（188MB 实测被拒、60MB 稳），>split_mb 的 PDF 必须分卷才能投原生。

用法: python pdf_split.py 输入.pdf 输出目录 卷数
stdout 最后一行 JSON：{"parts": ["…(1of3).pdf", …]}
⚠ 依赖 pymupdf——由 shelf_cli.calibre_bridge.py_with_pymupdf() 选解释器（uv `calibre` 依赖组），别用系统 python3 直接跑。
"""
from __future__ import annotations

import json
import sys
from pathlib import Path

try:
    import pymupdf as fitz  # type: ignore
except ImportError:  # 旧包名
    import fitz  # type: ignore


def split(path: Path, n: int, out_dir: Path) -> list[Path]:
    doc = fitz.open(str(path))
    pages = doc.page_count
    per = -(-pages // n)
    outs: list[Path] = []
    for i in range(n):
        a, b = i * per, min(pages, (i + 1) * per) - 1
        if a > b:
            break
        part = fitz.open()
        part.insert_pdf(doc, from_page=a, to_page=b)
        o = out_dir / f"{path.stem} ({i + 1}of{n}).pdf"
        part.save(str(o), garbage=4, deflate=True)
        part.close()
        outs.append(o)
    doc.close()
    return outs


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        return 2
    src, out_dir, n = Path(sys.argv[1]), Path(sys.argv[2]), int(sys.argv[3])
    out_dir.mkdir(parents=True, exist_ok=True)
    parts = split(src, n, out_dir)
    print(json.dumps({"parts": [str(p) for p in parts]}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
