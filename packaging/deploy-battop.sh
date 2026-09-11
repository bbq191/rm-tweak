#!/bin/sh
# host 侧一键构建+推送+安装 enhance/battop（电池刺客，纯 Rust systemd 常驻采样服务，
# 跟 xovi/vellum 完全无关——不检查、不依赖 xovi 是否已装）。
# enhance/battop/install.sh 是设备端安装器（需要同目录的 devlib.sh，本脚本一并推送）。
#
# 2026-09-20 改动（脚本审计）：
#  · 二进制先以 battop.new 推到 /home/root/battop/、md5 校验通过后由设备端 install.sh 原子 rename 覆盖——
#    不再"先 stop 服务再 scp 覆盖"（scp 失败会让 battop 停着；覆盖运行中的二进制曾 ETXTBSY）；
#  · 旧二进制备份进 cangjie-backups（保留最近几份），不再在 /home/root/battop/ 里散落 .bak.pre-*；
#  · 电池刺客**有意不开机自启**（见 enhance/battop/install.sh 头注与 battop-brick 事故记忆）：装完只 start，不 enable。
#
# 用法：./deploy-battop.sh [host]      host 默认 10.11.99.1
#   环境：CJ_BATTOP_BIN=<已编好的二进制> 跳过交叉编译（测试/离线用）
set -eu
cd "$(dirname "$0")"
# shellcheck disable=SC1091
. ./lib.sh
host_arg "用法：./deploy-battop.sh [host]      host 默认 10.11.99.1；环境 CJ_BATTOP_BIN=<已编好的二进制> 跳过交叉编译" "$@"
TARGET=aarch64-unknown-linux-musl
DIR=../enhance/battop
DEST=/home/root/battop
require_device   # 先确认设备连得上，再做耗时的交叉编译（与 deploy.sh / deploy-xovi-ext.sh 一致）

if [ -n "${CJ_BATTOP_BIN:-}" ]; then
    BIN="$CJ_BATTOP_BIN"
    echo "== CJ_BATTOP_BIN：不编译，用 $BIN =="
else
    echo "== 交叉编译 battop（$TARGET 全静态）=="
    # ⚠️ 必须先 cd 进 $DIR 再跑 cargo——Cargo 搜 .cargo/config.toml（CC/AR 覆盖）按当前工作目录往上找，
    # 不是按 --manifest-path；从别处调用会吃不到，在链接 crt1.o 这步报 "Relocations in generic ELF"。
    (cd "$DIR" && cargo build --release --target "$TARGET")
    BIN="$DIR/target/$TARGET/release/battop"
fi
[ -f "$BIN" ] || { echo "!! 缺 $BIN"; exit 1; }

echo "== 推送到 root@$HOST:$DEST（暂存名 battop.new，md5 校验）=="
push_verified "$BIN" "$DEST/battop.new" "$DIR/install.sh" "$DEST/install.sh" ./devlib.sh "$DEST/devlib.sh"

echo "== 设备端安装 =="
# 设备端退出码 10 = dm-verity 激活、单元从没装过、这步实际没装上（非失败，汇总里记"前置条件不满足"）
DEV_RC=0
rssh "sh $(shquote "$DEST/install.sh")" || DEV_RC=$?
[ "$DEV_RC" = 0 ] || [ "$DEV_RC" = 10 ] || exit "$DEV_RC"
if [ "$DEV_RC" = 10 ]; then step_skipped "dm-verity 激活，battop.service 没法装进 /usr"; fi
