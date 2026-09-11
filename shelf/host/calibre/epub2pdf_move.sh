#!/bin/sh
# EPUB → PDF 定稿流（C1）：按 reMarkable Paper Pro Move 参数渲染固定版式 PDF。
# 屏幕 954×1696 @~264ppi（7.3 寸竖版）。产物推送前必过 check_output.py 体检。
#
# 用法: epub2pdf_move.sh 输入.epub [输出.pdf]
# 环境变量:
#   PDF_FONT  正文衬线字体族（须已装在 host fontconfig），默认霞鹜新致宋
#             （设备阅读主字体，host 转换用同款观感一致；装法见白皮书）
#   PDF_SIZE  基础字号，单位 **CSS px**（Calibre 语义，18px=13.5pt），默认 18
#             → 屏上 em ≈31.6px ≈3.0mm（页 407pt 宽满屏 954px，2.344px/pt）
#   PDF_MARGIN  PDF 页边距 pt，默认 12（≈28px≈2.7mm；页宽 407pt，边距是屏幕的直接成本）
#
# 参数依据（详见阅读白皮书 Calibre 前置渲染节）:
#   --unit devicepixel        直接按设备像素定页面，避免纸张缩放
#   --subset-embedded-fonts   CJK 必开：不子集则整只中文字体十几 MB 起
#   字体必须选 TTF（TrueType 轮廓）：CFF 底字体（Noto CJK OTC 等）会被
#   Chromium/Skia 兜底成 Type 3 匿名字体（2026-09-02 实测坐实）
#   边距：Calibre PDF 输出有**独立**的 --pdf-page-margin-*（默认 72pt！）且优先于
#   通用 --margin-*，两套会叠加。2026-09-02 实测只设 --margin-* 时 72+20=92pt 左边距，
#   正文列只剩屏宽 51%、每行 15 字。必须显式设 --pdf-page-margin-* 并把 --margin-* 归零。
#   页宽 407pt 的小屏没有批注留白的余地，边距只留翻页手指不遮字的最小值。
set -eu

in=${1:?用法: epub2pdf_move.sh 输入.epub [输出.pdf]}
out=${2:-${in%.*}.pdf}
font=${PDF_FONT:-"LXGW Neo ZhiSong Screen Full"}
size=${PDF_SIZE:-18}
margin=${PDF_MARGIN:-12}
root=$(CDPATH='' cd -- "$(dirname -- "$0")/../../.." && pwd)

ebook-convert "$in" "$out" \
    --custom-size 954x1696 --unit devicepixel \
    --output-profile generic_eink_hd \
    --pdf-serif-family "$font" --pdf-standard-font serif \
    --embed-all-fonts --subset-embedded-fonts \
    --pdf-default-font-size "$size" \
    --pdf-page-margin-top "$margin" --pdf-page-margin-bottom "$margin" \
    --pdf-page-margin-left "$margin" --pdf-page-margin-right "$margin" \
    --margin-top 0 --margin-bottom 0 --margin-left 0 --margin-right 0 \
    --preserve-cover-aspect-ratio \
    --extra-css 'body{line-height:1.7} p{text-indent:2em;margin:0}'

# xref 规整：Qt WebEngine 产物偶发交叉引用表瑕疵（pdffonts 报 "try to reconstruct"，
# 2026-09-02 实测），pymupdf garbage=4 重存根治。
uv run --project "$root" --group calibre python - "$out" <<'PYEOF'
import sys
import pymupdf
path = sys.argv[1]
doc = pymupdf.open(path)
doc.save(path + ".clean", garbage=4, deflate=True)
doc.close()
import os
os.replace(path + ".clean", path)
PYEOF

echo "产物: $out"
