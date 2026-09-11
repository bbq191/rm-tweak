#!/bin/sh
# hw-stroke 安装脚本（vellum-xovi 结构，只碰 /home，不碰 /usr）——独立于 chinese-ime/langhook 之外的
# 最小 xovi 扩展，CJK 手写笔迹渲染的变宽笔画几何 hook。
#
# 默认 hwStrokeWidthFactor=1.0、两个效果 min_ratio=1.0（不改变笔画粗细）；效果开关在网页「管理 → 实验室」。
# 逐点日志 2026-09-24 起默认关（hwStrokeDebug），装完只看"hook 安装完成"两行——见 ../README.md。
#
# 2026-09-20：流程收进 packaging/xovi-ext-install.sh（与 hl-snap 共用，数据驱动），本文件只剩这个扩展的
# 数据。运行需要同目录有 xovi-ext-install.sh 与 devlib.sh——由 `packaging/deploy-handwriting-stroke.sh`
# 一起推送。
#
# 前置：vellum add xovi。
# 用法：./install.sh [--no-restart]（含义同 hl-snap/deploy/install.sh）
# shellcheck disable=SC2034  # 下面这些变量由 source 进来的 xovi-ext-install.sh 使用
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
EXT_NAME=hw-stroke
EXT_SO=hw-stroke.so
EXT_MAPTAG=hw-stroke
RQOL_INIT='{"hwStrokeWidthFactor":1.0}'
RQOL_MSG='建最小配置（hwStrokeWidthFactor 默认 1.0，不改变笔画粗细）'
OK_MSG='xochitl 重启后跑：journalctl -u xochitl | grep hw-stroke'
NEXT_MSG='   应有「变宽几何 hook 安装完成」「第二几何 hook 安装完成」两行；效果在网页「管理 → 实验室」开（逐点日志要 hwStrokeDebug=true）。'
[ -f "$HERE/xovi-ext-install.sh" ] || { echo "!! 缺 $HERE/xovi-ext-install.sh（用 packaging/deploy-handwriting-stroke.sh 部署，它会一起推送）"; exit 1; }
# shellcheck disable=SC1091
. "$HERE/xovi-ext-install.sh"
