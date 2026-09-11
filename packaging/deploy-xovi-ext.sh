#!/bin/sh
# host 侧一键构建+推送+安装一个"独立最小 xovi 扩展"（数据驱动，2026-09-20 起 hl-snap 与
# handwriting-stroke 共用这一份，原先两份 deploy 脚本 ~29 行差异全是名字）。
#   hl-snap  → enhance/hl-snap（荧光笔 CJK 精确吸附，hook FUN_00f05ad0）
#   hw-stroke→ enhance/handwriting-stroke（CJK 手写笔迹渲染，hook FUN_00f47530）；两者不冲突，可同时装。
# 入口仍是 deploy-hl-snap.sh / deploy-handwriting-stroke.sh（install-all 与文档沿用的名字，薄包装）。
#
# 前置：设备已 vellum add xovi（设备端 install.sh 检查，缺失清楚报错）。
# 前置（host 侧构建）：需要 asivery/xovi 的 clone 供 xovigen 生成元数据胶水，缺省找 ../../../xovi；
# 不在默认位置就 `XOVI_DIR=<clone路径> sh deploy-hl-snap.sh <host>`。CJ_SKIP_BUILD=1 跳过构建、直接用仓库里已提交的 .so。
#
# 用法：./deploy-xovi-ext.sh <hl-snap|hw-stroke> [host]      host 默认 10.11.99.1
#   环境 DEFER_XOVI_START=1：只把 .so 落盘（设备端 install.sh --no-restart），不重启 xochitl——install-all 编排多个
#   扩展时用，最后统一重启一次（多次重启撞 watchdog+StartLimit，2026-09-11 真机踩过整机重启）。
#   单独跑不用管：装完自动生效（xovi 已生效或装了 xovi-reenable → 换入后主动整机重启、设备回来后自动跑
#   verify-on-device.sh；都没有才 xovi/start，见 devlib.sh 的 cj_xochitl_apply）；
#   .so 没变、已加载、也没有别的待生效改动时什么都不做（2026-09-24）。
set -eu
cd "$(dirname "$0")"
# shellcheck disable=SC1091
. ./lib.sh
USAGE="用法：deploy-xovi-ext.sh <hl-snap|hw-stroke> [host]      环境：DEFER_XOVI_START=1（只落盘不重启）CJ_SKIP_BUILD=1 XOVI_DIR=<xovi clone>"
case "${1:-}" in -h|--help) echo "$USAGE"; exit 0 ;; esac
NAME="${1:?$USAGE}"; shift
host_arg "$USAGE" "$@"
case "$NAME" in
    hl-snap)   DIR=../enhance/hl-snap;            SO=hl-snap.so;   STEP=hl-snap ;;
    hw-stroke) DIR=../enhance/handwriting-stroke; SO=hw-stroke.so; STEP=handwriting-stroke ;;
    *) echo "!! 未知扩展 $NAME（hl-snap|hw-stroke）"; exit 2 ;;
esac
DEST="/home/root/$(step_payload_dir "$STEP")"   # 载荷目录与 uninstall-all 共用 lib.sh 的 step_payload

require_device
echo "== 构建 $SO =="
# ⚠️ 不跑 `make clean`——产物已提交进仓库，缺外部 xovi clone 时重编会失败；不清现有 .so/xovi_glue.{c,h}
# 才能在那种情况下退回用仓库里已提交的版本（先删了才发现编不出新的，真机实测踩过）。
if [ "${CJ_SKIP_BUILD:-0}" = "1" ]; then
    echo "-- CJ_SKIP_BUILD=1：不构建，用现有 $DIR/$SO"
    [ -f "$DIR/$SO" ] || { echo "!! 没有 $DIR/$SO"; exit 1; }
elif ! make -C "$DIR" aarch64; then
    if [ -f "$DIR/$SO" ]; then
        echo "⚠️  重新构建失败（大概率是本机缺 asivery/xovi clone，见 XOVI_DIR 提示）——"
        echo "    改用仓库里已提交的 $DIR/$SO（可能不是最新源码对应的版本）"
    else
        echo "!! 构建失败，且仓库里也没有已提交的 $DIR/$SO，无法继续"
        exit 1
    fi
fi

echo "== 推送到 root@$HOST:$DEST（暂存位置，不是 extensions.d；md5 逐个校验）=="
push_verified "$DIR/$SO" "$DEST/$SO" \
    "$DIR/deploy/install.sh" "$DEST/deploy/install.sh" \
    ./xovi-ext-install.sh "$DEST/deploy/xovi-ext-install.sh" \
    ./devlib.sh "$DEST/deploy/devlib.sh"

echo "== 设备端安装 =="
ARGS=""
# 注：不用 `[ ... ] && ARGS=...`——条件为假时该写法本身以非零退出，set -e 下会把整个脚本提前炸掉，必须 if/fi。
if [ "${DEFER_XOVI_START:-0}" = "1" ]; then
    ARGS="--no-restart"
fi
run_apply rssh "sh $(shquote "$DEST/deploy/install.sh") $ARGS"
