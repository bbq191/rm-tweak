#!/bin/sh
# host 侧一键构建+推送+安装 enhance/battop（电池刺客，纯 Rust systemd 常驻采样服务，
# 跟 xovi/vellum 完全无关——不检查、不依赖 xovi 是否已装）。这三步（cargo build → scp →
# ssh 跑 install.sh）之前一直是手动做的，这里只是脚本化，不改任何构建/安装逻辑本身；
# enhance/battop/install.sh 本身仍然可以脱离本脚本独立跑（先手动 scp 二进制再跑它）。
#
# 用法：./deploy-battop.sh [host]      host 默认 10.11.99.1
set -eu
cd "$(dirname "$0")"
HOST="${1:-10.11.99.1}"
TARGET=aarch64-unknown-linux-musl
DIR=../enhance/battop
DEST=/home/root/battop

echo "== 交叉编译 battop（$TARGET 全静态）=="
# ⚠️ 必须先 cd 进 $DIR 再跑 cargo——Cargo 搜 .cargo/config.toml（CC/AR 覆盖）是按当前工作
# 目录往上找，不是按 --manifest-path；从别处用 --manifest-path 调用会吃不到
# enhance/battop/.cargo/config.toml，在这台机器上实测直接在链接 crt1.o 这步报
# "Relocations in generic ELF" 炸掉（cd 进去再编立刻恢复正常，已验证）。
(cd "$DIR" && cargo build --release --target "$TARGET")
BIN="$DIR/target/$TARGET/release/battop"
[ -f "$BIN" ] || { echo "!! 构建后仍缺 $BIN"; exit 1; }

echo "== 推送到 root@$HOST:$DEST =="
# ⚠️ 重装（不是首次装）时 battop.service 可能已经在跑——scp 直接覆盖一个正在执行的二进制会被
# 内核拒绝（ETXTBSY，scp 报 "dest open ... Failure"，真机实测过一次）。先停服务再传，
# install.sh 最后会自己重新 enable --now，不影响"停了忘记重启"的风险。
ssh "root@$HOST" "mkdir -p $DEST; systemctl stop battop.service 2>/dev/null || true"
# 备份旧二进制（若是重装）——必须在 scp 覆盖它之前做，$DEST 本身不是 xovi extensions.d，
# 同目录放备份没有"被当成扩展重复加载"的风险。2026-09-15 全量代码审查补（deploy-sidebar-
# entry.sh 已经这么做，这几个部署脚本当时漏了）。
ssh "root@$HOST" "[ -f $DEST/battop ] && cp $DEST/battop $DEST/battop.bak.pre-\$(date +%Y%m%d-%H%M%S) && echo '-- 已备份旧版本' || true"
scp "$BIN" "root@$HOST:$DEST/battop"
scp "$DIR/install.sh" "root@$HOST:$DEST/install.sh"

echo "== md5 校验（跟 deploy-sidebar-entry.sh 同款套路，2026-09-15 全量代码审查补）=="
LOCAL_MD5="$(md5sum "$BIN" | awk '{print $1}')"
REMOTE_MD5="$(ssh "root@$HOST" "md5sum $DEST/battop" | awk '{print $1}')"
if [ "$LOCAL_MD5" != "$REMOTE_MD5" ]; then
    echo "!! md5 对不上（$LOCAL_MD5 vs $REMOTE_MD5），传输可能损坏，不继续安装"
    exit 1
fi
echo "-- md5 一致"

echo "== 设备端安装 =="
# shellcheck disable=SC2029  # 远端路径就是要在本地展开（固定字面量，无用户输入拼接风险）
ssh "root@$HOST" "sh $DEST/install.sh"
