#!/bin/sh
# host 侧一键探测+打上 appload 3.28 兼容补丁（appload_patch_328.py，见同目录
# appload-qmd-PROVENANCE.md）。**不接入 install-all.sh 的自动编排**——这个补丁改的是设备端
# 已装好的 appload.so 二进制内容，这台写代码的机器上没有真机可连，也没有真实的 appload.so
# 试跑过，字节替换逻辑只在构造出的假二进制片段上过了 host 单测（见 packaging/tests/
# test_appload_patch_328.py），对真实 appload.so 有没有效还没有真机验证过——按工程纪律
# "改变设备行为的改动，没在真机上跑通之前不说已完成"，先作为独立手动步骤存在，真机验证过
# 之后再考虑要不要并入 install-all.sh。
#
# 流程：探测设备上 appload.so 的当前状态（unpatched/patched/unknown）→ 只在 unpatched 时
# 备份+打补丁+传回+校验 → 提示需要 xovi/start 才会在运行中的 xochitl 里生效。
#
# 前置：设备已 vellum add appload（`/home/root/xovi/extensions.d/appload.so` 存在）。
#
# 用法：./deploy-appload-patch.sh [host]      host 默认 10.11.99.1
set -eu
cd "$(dirname "$0")"
HOST="${1:-10.11.99.1}"
REMOTE_SO=/home/root/xovi/extensions.d/appload.so
LOCAL_TMP="$(mktemp -d)"
trap 'rm -rf "$LOCAL_TMP"' EXIT

echo "== 探测设备端 appload.so =="
if ! ssh "root@$HOST" "[ -f $REMOTE_SO ]"; then
    echo "-- 设备没装 appload（$REMOTE_SO 不存在，先 vellum add appload）——跳过，非失败"
    exit 0
fi

echo "== 拉取设备端 appload.so 到本地判定状态 =="
scp "root@$HOST:$REMOTE_SO" "$LOCAL_TMP/appload.so"

STATUS="$(python3 appload_patch_328.py --check "$LOCAL_TMP/appload.so")"
echo "-- 当前状态：$STATUS"
case "$STATUS" in
    patched)
        echo "✅ 已经是打过 3.28 补丁的版本，不需要再打（幂等，非失败）"
        exit 0
        ;;
    unknown)
        echo "!! 这份 appload.so 既不是已知的 v0.5.3 原始版本，也不是已知的打过补丁的版本——"
        echo "   可能是不同的 appload 版本，本工具只认 v0.5.3，不碰这个文件，避免在不确定的"
        echo "   情况下写坏它。见 appload-qmd-PROVENANCE.md。"
        exit 1
        ;;
    unpatched) ;;
    *)
        echo "!! 未知返回：$STATUS"
        exit 1
        ;;
esac

echo "== 打补丁（本地）=="
python3 appload_patch_328.py "$LOCAL_TMP/appload.so" "$LOCAL_TMP/appload.so.patched"

echo "== 备份设备端旧版本（cangjie-backups/，绝不放 extensions.d/ 本身）=="
ssh "root@$HOST" "mkdir -p /home/root/cangjie-backups && cp $REMOTE_SO /home/root/cangjie-backups/appload.so.bak.pre-328patch-\$(date +%Y%m%d-%H%M%S)"

echo "== 推送打过补丁的版本 =="
scp "$LOCAL_TMP/appload.so.patched" "root@$HOST:$REMOTE_SO"

echo "== md5 校验 =="
LOCAL_MD5="$(md5sum "$LOCAL_TMP/appload.so.patched" | awk '{print $1}')"
REMOTE_MD5="$(ssh "root@$HOST" "md5sum $REMOTE_SO" | awk '{print $1}')"
if [ "$LOCAL_MD5" != "$REMOTE_MD5" ]; then
    echo "!! md5 对不上（$LOCAL_MD5 vs $REMOTE_MD5），传输可能损坏——设备上的备份还在"
    echo "   cangjie-backups/，需要手动核实/回滚，本脚本不自动重试。"
    exit 1
fi
echo "-- md5 一致"

echo "== 完成（落盘）=="
echo "   要在当前运行中的 xochitl 里生效，需要跑一次 /home/root/xovi/start（或重启设备），"
echo "   跟其它 xovi 扩展改动一样不在这里自动重启——见 packaging/install-all.sh 头注为什么"
echo "   不让每一步各自触发重启。"
echo "   ⚠ 真机验证清单（还没做过，需要用户确认）：xovi/start 之后重启 xochitl 健康"
echo "     （is-active/NRestarts/MainPID）+ journalctl 里能看到 appload 自己的"
echo "     'Loaded external AppLoad hooks in main UI' 成功信号 + Sidebar 里挂的入口点了有反应。"
