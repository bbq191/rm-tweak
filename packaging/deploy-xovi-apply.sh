#!/bin/sh
# host 侧对设备统一"让已落盘的 xovi 扩展/qmd 生效"一次（2026-09-25 起＝主动整机重启，见 devlib.sh 的 cj_xochitl_apply）。
#
# 用在所有"只落盘、不自己重启 xochitl"的步骤跑完之后，最后调用一次——hl-snap/handwriting-stroke/
# sidebar-entry 用 DEFER_XOVI_START=1 时只落盘不重启；shelf 的字体菜单/回收站/建夹/漫画页边距/阅读器翻页 qmd 本来就只落盘。
# 没有"只重载一个扩展"的机制，多个步骤各自重启等于短时间内重启 xochitl 多次，会撞 xochitl 的
# watchdog+StartLimit（2026-09-11 install-all 连续装 hl-snap+handwriting-stroke，真机触发过意外整机重启）。
#
# ⚠ 怎么"重启"由设备端 devlib.sh 的 cj_xochitl_apply 判定：
#   xovi 已在运行的 xochitl 里生效，或装了 xovi-reenable → 换入待换入区后主动整机重启（2026-09-25 起；单独
#   restart xochitl 有概率在它退出途中崩溃）；都没有才 xovi/start。
#   旧版无条件 xovi/start：在已生效的 xochitl 上它会 umount 重挂 drop-in 目录，xochitl SEGV → 整机自动重启
#   （2026-09-20 真机事故；重跑 install-all 必踩）。重启前会打印"打断阅读"提示并留 5 秒宽限。
#
# 只在"有待生效的落盘改动（含待换入的扩展 .so）"或"xovi 还没在 xochitl 里生效"时才重启（判据 devlib.sh 的
# cj_apply_needed；各落盘步骤真的改了文件时才记标记，见
# devlib.sh 的 cj_pending_mark；标记在 /run，重启设备即清）——重复跑 install-all 不再每次闪屏。
# 没标记但想强制重启（比如手工换过 .so）：--force。
#
# 用法：./deploy-xovi-apply.sh [host] [--force]      host 默认 10.11.99.1
set -eu
cd "$(dirname "$0")"
# shellcheck disable=SC1091
. ./lib.sh
USAGE="用法：./deploy-xovi-apply.sh [host] [--force]      host 默认 10.11.99.1；--force = 无论有无待生效改动都让它生效一次（整机重启）"
FORCE_RESTART=0
# 把 --force 摘出去，其余位置参数（[host]）原样交给 host_arg（轮转一遍 "$@"）
_n=$#
while [ "$_n" -gt 0 ]; do
    _a=$1; shift; _n=$((_n - 1))
    case "$_a" in --force) FORCE_RESTART=1 ;; *) set -- "$@" "$_a" ;; esac
done
host_arg "$USAGE" "$@"
require_device

echo "== 设备端让 xovi 扩展 + qmd 生效（有待生效改动才整机重启一次，约 1 分钟，会打断设备上正在做的事）=="
run_apply dev_script "$FORCE_RESTART" <<'DEVICE_SCRIPT'
set -eu
FORCE_RESTART="$1"
cj_require_root || exit 1
PENDING="$(cj_pending_list | tr '\n' ' ')"
if [ "$FORCE_RESTART" != "1" ]; then
    if ! cj_apply_needed; then
        echo "-- 没有待生效的落盘改动，且 xovi 已在 xochitl 里生效——不重启 xochitl（要强制重启：--force）"
        exit 0
    fi
    if [ -z "$PENDING" ] && ! cj_xochitl_has_xovi && [ ! -x "$CJ_XOVI/start" ]; then
        echo "-- 设备没装 xovi（没有 $CJ_XOVI/start）也没有待生效改动，没有需要生效的东西，跳过"
        exit 0
    fi
fi
[ -z "$PENDING" ] || echo "-- 待生效：$PENDING"
OLD_PID="$(cj_xochitl_pid)"
cj_xochitl_apply || exit 1
if [ "$CJ_APPLY_REBOOTED" = 1 ]; then
    exit 0   # 已排上整机重启；健康检查留给设备回来后的 verify-on-device.sh
fi
TAGS=""
[ -f "$CJ_XOVI/extensions.d/hl-snap.so" ] && TAGS="$TAGS hl-snap"
[ -f "$CJ_XOVI/extensions.d/hw-stroke.so" ] && TAGS="$TAGS hw-stroke"
# shellcheck disable=SC2086  # TAGS 有意按词展开
if cj_xochitl_health "$OLD_PID" $TAGS; then
    echo "✅ xochitl 重启完成，xovi 扩展/qmd 已重新注入"
else
    echo "⚠️  健康检查未达预期。查 journalctl -u xochitl"
    exit 1
fi
DEVICE_SCRIPT
