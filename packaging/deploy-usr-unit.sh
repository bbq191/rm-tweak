#!/bin/sh
# host 侧统一的"把一个 systemd 单元装进设备 /usr（rootfs，普通重启不丢，OTA 冲掉后重跑）"部署器。
# 2026-09-20 起 chrony-boot-wakelock / xovi-persist / wifi-watch 共用这一份（原先前两个各写一遍
# "dm-verity 门 + remount rw + cp + wants 链接 + reload"，且中途失败会把 rootfs 留在 rw）。
# 入口仍是 deploy-chrony-boot-wakelock.sh / deploy-xovi-persist.sh / deploy-wifi-watch.sh（薄包装）。
#
# 设备端全部走 devlib.sh：dm-verity 激活 → 跳过写 /usr（非失败，exit 0）；rw 窗口带 trap，失败也恢复 ro；
# 单元已是最新则完全不 remount；旧单元先备份进 cangjie-backups（保留最近几份）。
#
# 用法：./deploy-usr-unit.sh <chrony-boot-wakelock|xovi-persist|wifi-watch> [host]      host 默认 10.11.99.1
set -eu
cd "$(dirname "$0")"
# shellcheck disable=SC1091
. ./lib.sh
USAGE="用法：deploy-usr-unit.sh <chrony-boot-wakelock|xovi-persist|wifi-watch> [host]"
case "${1:-}" in -h|--help) echo "$USAGE"; exit 0 ;; esac
NAME="${1:?$USAGE}"; shift
host_arg "$USAGE" "$@"
# 设备端用两个语义记号（不再对路径做 shell 二次展开）：NEEDS=xovi-start 要求 $CJ_XOVI/start 存在；EXTRA_DST=local-bin 落 ~/.local/bin/
EXTRA_SRC=""; EXTRA_DST="-"; NEEDS="-"; START=0
case "$NAME" in
    chrony-boot-wakelock)
        UNIT=chrony-boot-wakelock.service; SRC=chrony-boot-wakelock.service
        VERITY_NOTE="功能不受影响，只是重启后 chrony 首次同步仍可能被自动休眠打断，慢几分钟报 synced。"
        DONE_NOTE="真正验证需要重启一次设备，确认 chronyd 在这把锁保护的窗口内完成首次同步、timedatectl 不再需要等自动休眠反复打断+退避那十几分钟才显示 synchronized: yes。" ;;
    xovi-persist)
        UNIT=xovi-reenable.service; SRC=xovi-reenable.service
        NEEDS=xovi-start   # 没装 xovi 本体就没东西可恢复
        VERITY_NOTE="功能不受影响，只是重启后仍需手动 /home/root/xovi/start。"
        DONE_NOTE="真正验证需要重启一次设备，确认 xovi 扩展/qmd 不再需要手动 xovi/start 就自动恢复。" ;;
    wifi-watch)
        UNIT=wifi-watch.service; SRC=wifi-watch/wifi-watch.service
        EXTRA_SRC=wifi-watch/wifi-watch.sh; EXTRA_DST=local-bin; START=1
        VERITY_NOTE="脚本已就位，但单元没装进 /usr——重启后 wifi-watch 不会自启（可手动 sh ~/.local/bin/wifi-watch.sh &）。"
        DONE_NOTE="验证：ssh 上设备 systemctl is-active wifi-watch；journalctl -u wifi-watch 看固化/重连日志。" ;;
    *) echo "!! 未知单元 $NAME"; exit 2 ;;
esac
DEST="/home/root/$(step_payload_dir "$NAME")"   # 载荷目录与 uninstall-all 共用 lib.sh 的 step_payload
require_device

echo "== 推送 $UNIT 到 root@$HOST:$DEST（md5 校验）=="
if [ -n "$EXTRA_SRC" ]; then
    push_verified "$SRC" "$DEST/$(basename "$SRC")" "$EXTRA_SRC" "$DEST/$(basename "$EXTRA_SRC")"
else
    push_verified "$SRC" "$DEST/$(basename "$SRC")"
fi

echo "== 设备端安装（dm-verity 门 + 带 trap 的 rw 窗口，devlib.sh）=="
# 设备端退出码 10 = dm-verity 激活、单元从没装过、这步实际没装上（非失败，汇总里记"前置条件不满足"）
DEV_RC=0
dev_script "$UNIT" "$DEST/$(basename "$SRC")" "${EXTRA_SRC:+$DEST/$(basename "$EXTRA_SRC")}" "$EXTRA_DST" "$NEEDS" "$START" "$VERITY_NOTE" <<'DEVICE_SCRIPT' || DEV_RC=$?
set -eu
UNIT="$1"; SRC="$2"; EXTRA_SRC="$3"; EXTRA_DST="$4"; NEEDS="$5"; START="$6"; VERITY_NOTE="$7"
cj_require_root || exit 1
# 先把能预先校验的全部校验完，再动任何东西（失败时不留半成品）
[ -f "$SRC" ] || { echo "!! 缺 $SRC（推送失败？）"; exit 1; }
if [ "$NEEDS" = "xovi-start" ]; then
    [ -e "$CJ_XOVI/start" ] || { echo "!! 没找到 $CJ_XOVI/start —— 先在设备上跑：vellum add xovi"; exit 1; }
fi
if [ -n "$EXTRA_SRC" ]; then
    [ -f "$EXTRA_SRC" ] || { echo "!! 缺 $EXTRA_SRC（推送失败？）"; exit 1; }
    case "$EXTRA_DST" in
        local-bin) EXTRA_DST="$CJ_HOME/.local/bin/$(basename "$EXTRA_SRC")" ;;
        *) echo "!! 内部错误：未知 EXTRA_DST 记号 $EXTRA_DST"; exit 1 ;;
    esac
    cj_backup_if_differs "$EXTRA_SRC" "$EXTRA_DST" || exit 1   # 内容没变就不堆重复备份
    cj_safe_replace "$EXTRA_SRC" "$EXTRA_DST" "$CJ_STAGE_DIR" 755 || { echo "!! 写 $EXTRA_DST 失败"; exit 1; }
    cj_stage_cleanup
fi
SCRIPT_CHANGED="$CJ_REPLACED"
rc=0
cj_install_usr_unit "$UNIT" "$SRC" multi-user.target.wants || rc=$?
case "$rc" in
    0) ;;
    3)
        if [ -f "$CJ_SYSD/$UNIT" ]; then
            # 单元是以前装的（verity 之后才激活）：/usr 动不了，但脚本已更新——在跑的服务要重启才会用上新脚本
            echo "   /usr 里已有此前装的 $UNIT（本次没法更新它）。"
            if [ "$START" = "1" ] && [ "$SCRIPT_CHANGED" = "1" ]; then
                systemctl restart "$UNIT" || { echo "!! systemctl restart $UNIT 失败"; exit 1; }
                echo "-- 脚本有更新，已重启 $UNIT：$(systemctl is-active "$UNIT" 2>/dev/null || echo '?')"
            fi
        else
            echo "   $VERITY_NOTE"
            exit 10
        fi
        exit 0 ;;
    *) exit 1 ;;
esac
if [ "$START" = "1" ]; then
    # 只在"脚本或单元有变化，或服务没在跑"时才重启——重复部署不打扰正在跑的看护进程
    if [ "$SCRIPT_CHANGED" = "1" ] || [ "$CJ_UNIT_CHANGED" = "1" ] || [ "$(systemctl is-active "$UNIT" 2>/dev/null || true)" != "active" ]; then
        systemctl restart "$UNIT" || { echo "!! systemctl restart $UNIT 失败"; exit 1; }
    else
        echo "-- $UNIT 已是最新且在跑，不重启"
    fi
    echo "-- $UNIT 状态：$(systemctl is-active "$UNIT" 2>/dev/null || echo '?')"
fi
ls -l "$CJ_SYSD/multi-user.target.wants/$UNIT"
DEVICE_SCRIPT
[ "$DEV_RC" = 0 ] || [ "$DEV_RC" = 10 ] || exit "$DEV_RC"
if [ "$DEV_RC" = 10 ]; then
    step_skipped "dm-verity 激活，$UNIT 没法装进 /usr"
    exit 0
fi

echo "== 完成 =="
echo "   ${DONE_NOTE}"
