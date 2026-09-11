#!/bin/sh
# host 侧一键构建+推送+安装 enhance/handwriting-stroke（CJK 手写笔迹渲染优化，独立最小
# xovi 扩展）。结构照抄 deploy-hl-snap.sh——两个扩展的构建/部署模式完全一致，hook 目标
# 不同、彼此不冲突，可以同时装。
#
# 前置：设备已 vellum add xovi（本脚本不装，装不装由设备端 install.sh 检查，缺失会清楚报错）。
# 前置（host 侧构建）：同 deploy-hl-snap.sh，需要 asivery/xovi 的 clone，缺省 ../../../xovi，
# 不在默认位置就 `XOVI_DIR=<clone路径> sh deploy-handwriting-stroke.sh <host>`。
#
# 用法：./deploy-handwriting-stroke.sh [host]      host 默认 10.11.99.1
#   环境 DEFER_XOVI_START=1：只把 hw-stroke.so 落盘，不在这一步跑 xovi/start，理由同
#   deploy-hl-snap.sh 同一处注释——install-all.sh 编排时用这个避免短时间内反复重启 xochitl。
set -eu
cd "$(dirname "$0")"
HOST="${1:-10.11.99.1}"
DIR=../enhance/handwriting-stroke
DEST=/home/root/hw-stroke

echo "== 构建 hw-stroke.so =="
# ⚠️ 不跑 `make clean`——产物已提交进仓库，缺外部 xovi clone 时重编会失败；不清一遍现有
# .so/xovi_glue.{c,h} 才能在那种情况下退回用仓库里已提交的版本，见 deploy-hl-snap.sh 同一处注释。
if ! make -C "$DIR" aarch64; then
    if [ -f "$DIR/hw-stroke.so" ]; then
        echo "⚠️  重新构建失败（大概率是本机缺 asivery/xovi clone，见上面 XOVI_DIR 提示）——"
        echo "    改用仓库里已提交的 $DIR/hw-stroke.so（可能不是最新源码对应的版本）"
    else
        echo "!! 构建失败，且仓库里也没有已提交的 $DIR/hw-stroke.so，无法继续"
        exit 1
    fi
fi

echo "== 推送到 root@$HOST:$DEST =="
ssh "root@$HOST" "mkdir -p $DEST/deploy"
scp "$DIR/hw-stroke.so" "root@$HOST:$DEST/hw-stroke.so"
scp "$DIR/deploy/install.sh" "root@$HOST:$DEST/deploy/install.sh"

echo "== 设备端安装 =="
ARGS=""
if [ "${DEFER_XOVI_START:-0}" = "1" ]; then
    ARGS="--no-restart"
fi
# shellcheck disable=SC2029  # 远端路径就是要在本地展开（固定字面量，无用户输入拼接风险）
ssh "root@$HOST" "sh $DEST/deploy/install.sh $ARGS"
