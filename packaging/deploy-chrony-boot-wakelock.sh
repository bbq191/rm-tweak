#!/bin/sh
# host 侧一键安装 开机头几十秒防自动休眠打断 chronyd 首次校时（chrony-boot-wakelock.service；根因见该 .service 头注）。
# 2026-09-20 起是 deploy-usr-unit.sh 的薄包装（"dm-verity 门 + 带 trap 的 rw 窗口写 /usr"只此一份）。
# 用法：./deploy-chrony-boot-wakelock.sh [host]      host 默认 10.11.99.1
set -eu
cd "$(dirname "$0")"
[ $# -le 1 ] || { echo "!! 用法：deploy-chrony-boot-wakelock.sh [host]"; exit 2; }
exec sh ./deploy-usr-unit.sh chrony-boot-wakelock "$@"
