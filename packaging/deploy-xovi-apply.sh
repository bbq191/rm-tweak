#!/bin/sh
# host 侧对设备统一跑一次 xovi/start（全量重扫 extensions.d + 重注入已落盘的 qmd）+ 健康检查。
#
# 用在所有"只落盘、不自己重启 xochitl"的步骤跑完之后，最后调用一次——hl-snap/handwriting-
# stroke 用 DEFER_XOVI_START=1（见 deploy-hl-snap.sh/deploy-handwriting-stroke.sh）时只落盘不
# 重启；shelf 的字体菜单/回收站/建夹三个 qmd 本来就只落盘、从不自己跑 xovi/start（shelf/
# install.sh 的既有设计，见其头注）。**xovi/start 是全量重启 xochitl 重新扫描/注入全部内容，
# 没有"只重载一个扩展"的机制**（系统增强线白皮书有过专门论证）——多个步骤各自都跑一次等于短
# 时间内重启 xochitl 多次，xochitl 有 watchdog+StartLimit，真机验证过这样容易撞
# StartLimitAction 触发整机重启（2026-09-11 packaging/install-all.sh 连续装 hl-snap+
# handwriting-stroke，两步各自跑一次 xovi/start，真机触发了一次意外整机重启）。改成全部落盘
# 完只在最后统一跑这一次。
#
# 用法：./deploy-xovi-apply.sh [host]      host 默认 10.11.99.1
set -eu
cd "$(dirname "$0")"
HOST="${1:-10.11.99.1}"

echo "== 设备端跑一次 xovi/start（重注入全部已落盘的 xovi 扩展 + qmd）=="
# shellcheck disable=SC2087  # heredoc 内变量就是要在本地展开，全部是固定字面量，无远端注入风险
ssh "root@$HOST" "sh -s" <<'DEVICE_SCRIPT'
set -eu
XOVI=/home/root/xovi

if [ ! -f "$XOVI/start" ]; then
    echo "!! 没找到 $XOVI/start —— 先跑：vellum add xovi（没装 xovi 就没有可应用的扩展，跳过）"
    exit 1
fi

OLD_PID="$(systemctl show xochitl -p MainPID --value 2>/dev/null || echo 0)"
"$XOVI/start"
sleep 5

STATE="$(systemctl is-active xochitl 2>/dev/null || true)"
NEW_PID="$(systemctl show xochitl -p MainPID --value 2>/dev/null || echo 0)"
NREST="$(systemctl show xochitl -p NRestarts --value 2>/dev/null || echo '?')"
echo "=================================================="
echo "  is-active : $STATE   (期望 active)"
echo "  MainPID   : $OLD_PID -> $NEW_PID   (期望有变化)"
echo "  NRestarts : $NREST   (期望 0/不增)"
if [ -f "$XOVI/extensions.d/hl-snap.so" ]; then
    HLCNT="$(grep -c hl-snap /proc/"$NEW_PID"/maps 2>/dev/null || echo 0)"
    echo "  hl-snap    : $HLCNT 段（期望 >0）"
fi
if [ -f "$XOVI/extensions.d/hw-stroke.so" ]; then
    HWCNT="$(grep -c hw-stroke /proc/"$NEW_PID"/maps 2>/dev/null || echo 0)"
    echo "  hw-stroke  : $HWCNT 段（期望 >0）"
fi
XOVICNT="$(grep -c xovi.so /proc/"$NEW_PID"/maps 2>/dev/null || echo 0)"
echo "  xovi.so 总段数: $XOVICNT（期望 >0，只要装过任意一个 xovi 扩展）"
echo "=================================================="
if [ "$STATE" = "active" ] && [ "$NEW_PID" != "0" ]; then
    echo "✅ xovi/start 完成"
else
    echo "⚠️  健康检查未达预期。查 journalctl -u xochitl"
    exit 1
fi
DEVICE_SCRIPT
