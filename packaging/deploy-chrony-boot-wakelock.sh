#!/bin/sh
# host 侧一键安装"开机头几十秒防自动休眠打断 chronyd 首次校时"（chrony-boot-wakelock.service）。
# 根因/为什么这么修见 packaging/chrony-boot-wakelock.service 头注；本脚本只负责推过去 + 按
# shelf/install.sh 已经验证过的同一套"dm-verity 门 + remount rw/ro 写 /usr"模式装上。
#
# 用法：./deploy-chrony-boot-wakelock.sh [host]      host 默认 10.11.99.1
set -eu
cd "$(dirname "$0")"
HOST="${1:-10.11.99.1}"
DEST=/home/root/chrony-boot-wakelock

echo "== 推送 chrony-boot-wakelock.service 到 root@$HOST:$DEST =="
ssh "root@$HOST" "mkdir -p $DEST"
scp chrony-boot-wakelock.service "root@$HOST:$DEST/chrony-boot-wakelock.service"

echo "== 设备端安装（dm-verity 门 + remount 写 /usr，同 shelf/install.sh 套路）=="
# shellcheck disable=SC2087  # heredoc 内变量就是要在本地展开，全部是固定字面量，无远端注入风险
ssh "root@$HOST" "sh -s" <<'DEVICE_SCRIPT'
set -eu
SYSD=/usr/lib/systemd/system
SRC=/home/root/chrony-boot-wakelock/chrony-boot-wakelock.service

[ -f "$SRC" ] || { echo "!! 缺 $SRC（推送失败？）"; exit 1; }

if dmsetup ls --target verity 2>/dev/null | grep -q .; then
    # 写 /usr + 重启 → root hash 变 → A/B 回滚变砖（2026-08-16 真机踩过），dm-verity 激活时绝不写。
    echo "✋ 检测到 dm-verity 激活 —— 跳过写 /usr（不装开机持久，避免变砖）。"
    echo "   功能不受影响，只是重启后 chrony 首次同步仍可能被自动休眠打断，慢几分钟报 synced。"
    exit 0
fi

mount -o remount,rw /
cp "$SRC" "$SYSD/chrony-boot-wakelock.service"
chmod 644 "$SYSD/chrony-boot-wakelock.service"
mkdir -p "$SYSD/multi-user.target.wants"
ln -sf ../chrony-boot-wakelock.service "$SYSD/multi-user.target.wants/chrony-boot-wakelock.service"
sync
mount -o remount,ro / || true
systemctl daemon-reload

echo "-- chrony-boot-wakelock.service 已装入 /usr（普通重启不丢）："
ls -l "$SYSD/multi-user.target.wants/chrony-boot-wakelock.service"
DEVICE_SCRIPT

echo "== 完成 =="
echo "   真正验证需要重启一次设备，确认 chronyd 在这把锁保护的窗口内完成首次同步、"
echo "   timedatectl 不再需要等自动休眠反复打断+退避那十几分钟才显示 synchronized: yes。"
