#!/bin/sh
# host 侧一键安装 xovi 开机持久化恢复链（xovi-reenable.service，前置：vellum add xovi）。
# 2026-09-20 起是 deploy-usr-unit.sh 的薄包装（"dm-verity 门 + 带 trap 的 rw 窗口写 /usr"只此一份）。
# 用法：./deploy-xovi-persist.sh [host]      host 默认 10.11.99.1
set -eu
cd "$(dirname "$0")"
[ $# -le 1 ] || { echo "!! 用法：deploy-xovi-persist.sh [host]"; exit 2; }
exec sh ./deploy-usr-unit.sh xovi-persist "$@"
