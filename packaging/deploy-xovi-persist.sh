#!/bin/sh
# host 侧一键安装 xovi 开机持久化恢复链（xovi-reenable.service）。真正的持久化单元定义在
# packaging/xovi-reenable.service，本脚本只负责推过去 + 在设备端按 shelf/install.sh 已经验证
# 过的同一套"dm-verity 门 + remount rw/ro 写 /usr"模式装上，不重新发明这套安全写法。
#
# 前置：设备已 vellum add xovi（/home/root/xovi/start 存在）——本脚本不装 xovi 本体，缺失时
# 清楚报错退出。
#
# 用法：./deploy-xovi-persist.sh [host]      host 默认 10.11.99.1
set -eu
cd "$(dirname "$0")"
HOST="${1:-10.11.99.1}"
DEST=/home/root/xovi-persist

echo "== 推送 xovi-reenable.service 到 root@$HOST:$DEST =="
ssh "root@$HOST" "mkdir -p $DEST"
scp xovi-reenable.service "root@$HOST:$DEST/xovi-reenable.service"

echo "== 设备端安装（dm-verity 门 + remount 写 /usr，同 shelf/install.sh 套路）=="
# shellcheck disable=SC2087  # heredoc 内变量就是要在本地展开，全部是固定字面量，无远端注入风险
ssh "root@$HOST" "sh -s" <<'DEVICE_SCRIPT'
set -eu
SYSD=/usr/lib/systemd/system
SRC=/home/root/xovi-persist/xovi-reenable.service

[ -f "$SRC" ] || { echo "!! 缺 $SRC（推送失败？）"; exit 1; }
if [ ! -f /home/root/xovi/start ]; then
    echo "!! 没找到 /home/root/xovi/start —— 先在设备上跑：vellum add xovi"
    exit 1
fi

if dmsetup ls --target verity 2>/dev/null | grep -q .; then
    # 写 /usr + 重启 → root hash 变 → A/B 回滚变砖（2026-08-16 真机踩过），dm-verity 激活时绝不写。
    echo "✋ 检测到 dm-verity 激活 —— 跳过写 /usr（不装开机持久，避免变砖）。"
    echo "   功能不受影响，只是重启后仍需手动 /home/root/xovi/start。"
    exit 0
fi

mount -o remount,rw /
cp "$SRC" "$SYSD/xovi-reenable.service"
chmod 644 "$SYSD/xovi-reenable.service"
mkdir -p "$SYSD/multi-user.target.wants"
ln -sf ../xovi-reenable.service "$SYSD/multi-user.target.wants/xovi-reenable.service"
sync
mount -o remount,ro / || true
systemctl daemon-reload

echo "-- xovi-reenable.service 已装入 /usr（普通重启不丢；下次真机重启会自动重跑 xovi/start）："
ls -l "$SYSD/multi-user.target.wants/xovi-reenable.service"
DEVICE_SCRIPT

echo "== 完成 =="
echo "   真正验证需要重启一次设备，确认 xovi 扩展/qmd 不再需要手动 xovi/start 就自动恢复。"
