#!/bin/sh
# host 侧一键构建+推送+安装 enhance/handwriting-stroke（CJK 手写笔迹渲染优化（独立最小 xovi 扩展））。
# 2026-09-20 起是 deploy-xovi-ext.sh 的薄包装（构建/推送/md5/安装流程与另一个扩展共用一份）——
# 用法不变：./deploy-handwriting-stroke.sh [host]      host 默认 10.11.99.1；环境 DEFER_XOVI_START=1 / CJ_SKIP_BUILD=1 / XOVI_DIR 同 deploy-xovi-ext.sh 头注。
set -eu
cd "$(dirname "$0")"
[ $# -le 1 ] || { echo "!! 用法：deploy-handwriting-stroke.sh [host]"; exit 2; }
exec sh ./deploy-xovi-ext.sh hw-stroke "$@"
