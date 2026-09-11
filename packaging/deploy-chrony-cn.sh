#!/bin/sh
# host 侧一键跑 chrony-cn.sh（国内 NTP 修复）。真正的逻辑全在 chrony-cn.sh 本体（可以直接
# `ssh root@host sh -s < packaging/chrony-cn.sh` 手动跑，见该文件头注）——本脚本只是把它包成
# `deploy-xxx.sh <host>` 的统一形状，方便 install-all.sh 用同一套 run_step 编排调用。
#
# 用法：./deploy-chrony-cn.sh [host]      host 默认 10.11.99.1
set -eu
cd "$(dirname "$0")"
HOST="${1:-10.11.99.1}"

ssh "root@$HOST" sh -s < chrony-cn.sh
