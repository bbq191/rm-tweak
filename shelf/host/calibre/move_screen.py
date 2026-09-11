"""reMarkable Paper Pro Move 屏幕常量与换算（host 侧工具共用，单一事实源）。

屏 7.3″ Gallery 3，竖向 954×1696 px，264 PPI（官方规格，2026-09-02 核实）。
"""

from __future__ import annotations

W_PX = 954
H_PX = 1696
PPI = 264
ASPECT = W_PX / H_PX  # 0.5625
MM_PER_PX = 25.4 / PPI  # ≈0.0962


def fit_scale(w_pt: float, h_pt: float) -> float:
    """PDF 页（pt）整页适配 Move 竖屏时的放大率（设备 px / pt）。"""
    return min(W_PX / w_pt, H_PX / h_pt)


def px_to_mm(px: float) -> float:
    return px * MM_PER_PX
