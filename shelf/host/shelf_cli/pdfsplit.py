"""大 PDF 分卷：xochitl `/upload` 有体积上限（真机：188MB 被 `multipart body is too large` 拒、60MB 稳），
>split_mb 的 PDF 按页数均分成若干卷。实际切分在 `host/calibre/pdf_split.py`（pymupdf），由 `calibre_bridge.py_with_pymupdf()`
选解释器跑——**缺 pymupdf 必须报错而不是静默不分卷**（2026-09-05 真机：297MB/188MB 漫画 PDF 没分卷就推上去，投原生被拒）。"""
from __future__ import annotations

import json
from pathlib import Path

from . import calibre_bridge as cb

SPLIT_SCRIPT = cb.CALIBRE_DIR / "pdf_split.py"


def needs_split(path: Path, split_mb: int) -> bool:
    return split_mb > 0 and path.suffix.lower() == ".pdf" and path.stat().st_size > split_mb * 1024 * 1024


def volumes_for(size_bytes: int, split_mb: int) -> int:
    cap = split_mb * 1024 * 1024
    return max(1, -(-size_bytes // cap))  # ceil


def split(path: Path, split_mb: int, out_dir: Path) -> list[Path]:
    """>split_mb 的 PDF 切成 ceil(size/cap) 卷；不需要切原样返回。切不了（缺 pymupdf / 脚本失败）抛 CalibreError。"""
    n = volumes_for(path.stat().st_size, split_mb)
    if n <= 1:
        return [path]
    r = cb._run([*cb.py_with_pymupdf(), str(SPLIT_SCRIPT), str(path), str(out_dir), str(n)])
    if r.returncode != 0:
        raise cb.CalibreError(f"PDF 分卷失败（{path.name} {path.stat().st_size >> 20}MB 超过 {split_mb}MB 上限，xochitl 收不下整本）：需要 pymupdf（uv `calibre` 依赖组）；{r.stderr.strip()[-400:]}")
    try:
        parts = [Path(p) for p in json.loads(r.stdout.strip().splitlines()[-1])["parts"]]
    except Exception as e:  # noqa: BLE001
        raise cb.CalibreError(f"pdf_split.py 输出不可解析（{e}）：{r.stdout.strip()[-300:]}") from None
    if not parts:
        raise cb.CalibreError(f"pdf_split.py 未产出分卷：{path.name}")
    return parts
