"""Calibre 桥（host 高质量路）：subprocess 调 `shelf/host/calibre/` 的脚本。
统一清洗环境：去掉 VIRTUAL_ENV / PATH 里的 .venv/bin —— `ebook-convert` 的 `#!/usr/bin/env python3`
在 venv 里会被劫持即炸（阅读白皮书 §11.2）。"""
from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

CALIBRE_DIR = Path(__file__).resolve().parent.parent / "calibre"


def clean_env(env: dict | None = None) -> dict:
    e = dict(os.environ if env is None else env)
    e.pop("VIRTUAL_ENV", None)
    e["PATH"] = os.pathsep.join(p for p in e.get("PATH", "").split(os.pathsep) if ".venv/bin" not in p and p)
    return e


def has_calibre() -> bool:
    return shutil.which("ebook-convert", path=clean_env().get("PATH")) is not None


class CalibreError(RuntimeError):
    pass


def _run(cmd: list[str], env_extra: dict | None = None, **kw) -> subprocess.CompletedProcess:
    env = clean_env()
    if env_extra:
        env.update(env_extra)
    return subprocess.run(cmd, env=env, check=False, text=True, capture_output=True, **kw)


REPO_ROOT = Path(__file__).resolve().parents[3]


def py_with_pymupdf() -> list[str]:
    """跑纯 pymupdf 脚本（check_output / pdf_crop_move）的解释器：它们不调 ebook-convert，可以走仓库 uv 环境
    （pymupdf 在 `calibre` 依赖组）；无 uv/pyproject 时退回系统 python3（需自行 pip 装 pymupdf）。"""
    if shutil.which("uv") and (REPO_ROOT / "pyproject.toml").is_file():
        return ["uv", "run", "--project", str(REPO_ROOT), "--group", "calibre", "python"]
    return ["python3"]


def wash(src: Path, out_dir: Path, env: dict | None = None) -> Path:
    """C2 洗书：wash_epub.sh（拍平 CSS/重建 TOC/伪 DRM 剥离/末步 epub-optimize）。返回产物路径。
    `env` 透传给脚本（如 `WASH_KEEP_PARA_SPACING=1` 保留段距）。"""
    r = _run(["sh", str(CALIBRE_DIR / "wash_epub.sh"), str(src), str(out_dir)], env_extra=env)
    if r.returncode != 0:
        raise CalibreError(f"wash_epub.sh 失败（rc={r.returncode}）：{r.stderr.strip()[-800:]}")
    outs = sorted(out_dir.glob("*.epub"), key=lambda p: p.stat().st_mtime)
    if not outs:
        raise CalibreError("wash_epub.sh 未产出 EPUB")
    return outs[-1]


def epub_optimize_bin() -> str | None:
    """定位 `epub-optimize` 二进制：`WASH_OPTIMIZE_BIN` 环境变量覆盖（跟 `wash_epub.sh` 认的是同一个变量，
    两条路径共用一个"在哪找二进制"的旋钮）→ PATH → 仓库内 `shelf/target/release/`（`cargo build --release
    -p bookconv --bin epub-optimize` 的默认产物位置）。找不到返回 None（调用方负责报错/提示）。"""
    override = os.environ.get("WASH_OPTIMIZE_BIN")
    if override:
        return override
    found = shutil.which("epub-optimize", path=clean_env().get("PATH"))
    if found:
        return found
    local = REPO_ROOT / "shelf" / "target" / "release" / "epub-optimize"
    return str(local) if local.is_file() else None


def optimize_only(src: Path, out: Path, keep_spacing: bool = False) -> Path:
    """C2'：`--no-calibre` 通道——直接调 `epub-optimize`（跟设备端 book-serve `Staging::optimize`/
    `wash_epub.sh` 末步是同一个 Rust 函数 `optimize_epub_with`），不经 `ebook-convert`，不需要装 Calibre，
    也就没有 `wash_epub.sh` 那部分"series 命名"（读 `ebook-meta`）——产物文件名跟输入一致，落在 `out`。
    伪 DRM 剥离/CSS 锁剥离/边距段距归零/清洗+优化全部在 `optimize_epub_with` 内部完成，不需要额外步骤。"""
    optbin = epub_optimize_bin()
    if not optbin:
        raise CalibreError("找不到 epub-optimize（cd shelf && cargo build --release -p bookconv --bin epub-optimize，"
                            "或用 WASH_OPTIMIZE_BIN 指路径）")
    cmd = [optbin]
    if keep_spacing:
        cmd.append("--keep-spacing")
    cmd += [str(src), str(out)]
    r = _run(cmd)
    if r.returncode != 0:
        raise CalibreError(f"epub-optimize 失败（rc={r.returncode}）：{r.stderr.strip()[-800:]}")
    return out


def to_pdf(src: Path, out: Path) -> Path:
    """C1 定稿：epub2pdf_move.sh → 954×1696 固定版式 PDF。"""
    r = _run(["sh", str(CALIBRE_DIR / "epub2pdf_move.sh"), str(src), str(out)])
    if r.returncode != 0 or not out.is_file():
        raise CalibreError(f"epub2pdf_move.sh 失败（rc={r.returncode}）：{r.stderr.strip()[-800:]}")
    return out


class ScannedPdf(Exception):
    """pdf_crop_move.py 退出码 3：扫描型 PDF 不产出，建议改投 KOReader（KOPT 重排）。"""


def crop_pdf(src: Path, out: Path) -> Path:
    r = _run([*py_with_pymupdf(), str(CALIBRE_DIR / "pdf_crop_move.py"), str(src), str(out)])
    if r.returncode == 3:
        raise ScannedPdf(r.stdout.strip() or r.stderr.strip())
    if r.returncode != 0 or not out.is_file():
        raise CalibreError(f"pdf_crop_move.py 失败（rc={r.returncode}）：{r.stderr.strip()[-800:]}")
    return out


def has_k2pdfopt() -> bool:
    return shutil.which("k2pdfopt", path=clean_env().get("PATH")) is not None


def _run_json(cmd: list[str], label: str, env_extra: dict | None = None) -> dict:
    """跑一个"末行打印 JSON"的脚本（reflow / txt 切章 / 漫画 16 灰 / 探针 / 量测共用契约）：非零退出或末行不是 JSON 都报 CalibreError。"""
    r = _run(cmd, env_extra=env_extra)
    if r.returncode != 0:
        raise CalibreError(f"{label} 失败（rc={r.returncode}）：{(r.stderr or r.stdout).strip()[-800:]}")
    try:
        return json.loads(r.stdout.strip().splitlines()[-1])
    except Exception as e:  # noqa: BLE001
        raise CalibreError(f"{label} 输出不可解析（{e}）：{r.stdout.strip()[-400:]}") from None


def pdf_comic_stats(path: Path) -> tuple[int, float]:
    """PDF 是否漫画的探针（pymupdf 子进程，见 `shelf/host/calibre/pdf_comic_probe.py`）：抽样统计
    "有图且几乎无文字"的页占比。查询失败（没装 pymupdf 的 calibre 依赖组/PDF 损坏）返回
    `(0, 0.0)`——`comic.is_comic()` 据此算出 False，不阻断推送，退回这次改动前的行为（PDF 一律
    走文字书洗书路）。"""
    try:
        d = _run_json([*py_with_pymupdf(), str(CALIBRE_DIR / "pdf_comic_probe.py"), str(path)], "pdf_comic_probe.py")
        return int(d.get("pages", 0)), float(d.get("ratio", 0.0))
    except CalibreError:
        return 0, 0.0


def reflow_pdf(src: Path, outdir: Path) -> tuple[Path, str]:
    """PDF 重排（born-digital 结构化→EPUB / 扫描件 k2pdfopt|裁边→PDF）。返回 (产物路径, kind∈{'epub','pdf'})。"""
    d = _run_json([*py_with_pymupdf(), str(CALIBRE_DIR / "pdf_reflow_move.py"), str(src), str(outdir)], "pdf_reflow_move.py")
    return Path(d["out"]), d["kind"]


def check(path: Path, require_toc: bool = False) -> tuple[bool, str]:
    """C5 体检（check_output.py）：返回 (通过?, 输出)。硬拦项非零退出。"""
    cmd = [*py_with_pymupdf(), str(CALIBRE_DIR / "check_output.py"), str(path)]
    if require_toc:
        cmd.append("--require-toc")
    r = _run(cmd)
    return r.returncode == 0, (r.stdout + r.stderr).strip()


def comic_gray(src: Path, out: Path, rtl: bool = True) -> tuple[Path, dict]:
    """漫画省刷新档 + 跨页拆分 + 白边裁切：CBZ → CBZ（comic_gray.py，Pillow 走 uv calibre 组）。
    `rtl=False` 时跨页拆分按从左往右排（`--ltr`；缺省从右往左，东亚漫画传统）。
    返回 (产物, {pages,split,gray,color,bytes_in,bytes_out,flagged})。"""
    cmd = [*py_with_pymupdf(), str(CALIBRE_DIR / "comic_gray.py")]
    if not rtl:
        cmd.append("--ltr")
    cmd += [str(src), str(out)]
    d = _run_json(cmd, "comic_gray.py")
    if not out.is_file():
        raise CalibreError("comic_gray.py 未产出 CBZ")
    return out, d


def txt_to_epub(src: Path, outdir: Path) -> tuple[Path, dict]:
    """中文 TXT → 带目录 EPUB（txt_to_epub.py，stdlib）。返回 (产物, 元数据 {chapters, volumes, encoding, detected,…})。"""
    d = _run_json(["python3", str(CALIBRE_DIR / "txt_to_epub.py"), str(src), str(outdir)], "txt_to_epub.py")
    return Path(d["out"]), d


CBZ2PDF_BIN_ENV = "CBZ2PDF_BIN"


def _cbz2pdf_bin() -> str | None:
    """定位编译好的 `cbz2pdf`：环境变量覆盖 → PATH → `shelf/target/release/` 兜底（照抄 wash_epub.sh
    找 epub-optimize 的顺序）。"""
    env = os.environ.get(CBZ2PDF_BIN_ENV)
    if env:
        return env
    found = shutil.which("cbz2pdf", path=clean_env().get("PATH"))
    if found:
        return found
    fallback = REPO_ROOT / "shelf" / "target" / "release" / "cbz2pdf"
    return str(fallback) if fallback.is_file() else None


def cbz_to_pdf(src: Path, out: Path) -> Path:
    """漫画 CBZ → 固定版式 PDF（`cbz2pdf` bin，包 `bookconv::convert::cbz::cbz_to_pdf`）。喂给它的该是
    已经过 `comic_gray` 处理的 CBZ——缺省 `--off` 档原样直嵌，不重新抖动，避免二次处理损画质。"""
    bin_path = _cbz2pdf_bin()
    if not bin_path:
        raise CalibreError(f"找不到 cbz2pdf（{CBZ2PDF_BIN_ENV} 指定路径 / PATH / shelf/target/release/ 都没有）；" "先 cd shelf && cargo build --release -p bookconv --bin cbz2pdf")
    r = _run([bin_path, str(src), str(out)])
    if r.returncode != 0 or not out.is_file():
        raise CalibreError(f"cbz2pdf 失败（rc={r.returncode}）：{r.stderr.strip()[-800:]}")
    return out


def comic2cbz(src: Path, out: Path) -> Path:
    """漫画 AZW3/MOBI/EPUB → CBZ（Calibre 解包成 EPUB 中转，按 spine 顺序抽整页图）。"""
    r = _run(["python3", str(CALIBRE_DIR / "comic2cbz.py"), str(src), str(out)])
    if r.returncode != 0 or not out.is_file():
        raise CalibreError(f"comic2cbz.py 失败（rc={r.returncode}）：{r.stderr.strip()[-800:]}")
    return out


def render_probe(out_dir: Path, title: str) -> Path:
    """`shelf doctor --render` 的探针 EPUB（纯 stdlib 脚本 render_probe.py）。"""
    return Path(_run_json(["python3", str(CALIBRE_DIR / "render_probe.py"), str(out_dir), title], "render_probe.py")["out"])


def render_measure(pdf: Path) -> dict:
    """量 xochitl 渲染缓存里探针段的首行缩进（pymupdf）。返回 {rows, ok, problems}。"""
    return _run_json([*py_with_pymupdf(), str(CALIBRE_DIR / "render_measure.py"), str(pdf)], "render_measure.py")


def workdir(prefix: str = "shelf-push-") -> Path:
    return Path(tempfile.mkdtemp(prefix=prefix))
