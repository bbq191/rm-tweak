#!/bin/sh
# hl-snap 安装脚本（vellum-xovi 结构，只碰 /home，不碰 /usr）——独立于 chinese-ime/langhook 之外的
# 最小 xovi 扩展，只做荧光笔精确吸附一件事。
#
# 2026-09-20：流程收进 packaging/xovi-ext-install.sh（与 handwriting-stroke 共用，数据驱动），本文件
# 只剩这个扩展的数据。运行需要同目录有 xovi-ext-install.sh 与 devlib.sh——由 `packaging/deploy-hl-snap.sh`
# 一起推送；脱离编排手动跑时先把这两个文件（packaging/ 下）连同 hl-snap.so 放到设备上对应位置。
#
# 前置：vellum add xovi。
# 用法：./install.sh [--no-restart]
#   --no-restart  只落盘 hl-snap.so，不重启 xochitl——多个扩展各自重启会在短时间内撞 xochitl 的
#   watchdog+StartLimit（2026-09-11 真机踩过一次意外整机重启）；install-all 用它，最后统一重启一次。
#   不带它时：xovi 已生效或装了 xovi-reenable → 换入后主动整机重启（2026-09-25 起，不单独重启 xochitl）；
#   否则 xovi/start（见 packaging/devlib.sh 的 cj_xochitl_apply；xovi 已生效时跑 xovi/start 会 SEGV 整机重启）。
# shellcheck disable=SC2034  # 下面这些变量由 source 进来的 xovi-ext-install.sh 使用
set -eu
HERE="$(cd "$(dirname "$0")" && pwd)"
EXT_NAME=hl-snap
EXT_SO=hl-snap.so
EXT_MAPTAG=hl-snap
RQOL_INIT='{"hlSnapCjk":true}'
RQOL_MSG='建最小配置（hlSnapCjk 默认开）'
OK_MSG='划中文即精确吸附（划哪吸哪）。'
NEXT_MSG=''
[ -f "$HERE/xovi-ext-install.sh" ] || { echo "!! 缺 $HERE/xovi-ext-install.sh（用 packaging/deploy-hl-snap.sh 部署，它会一起推送）"; exit 1; }
# shellcheck disable=SC1091
. "$HERE/xovi-ext-install.sh"
