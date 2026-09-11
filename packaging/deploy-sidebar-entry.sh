#!/bin/sh
# host 侧一键构建+推送 Sidebar 一级直达入口（KOReader，装了第三方 WeRead app 时自动带上它）。
#
# 2026-09-13 从「手动 SSH 上机部署」捞回 packaging/ 变成可重复脚本：源 QML 补丁本来在
# oldbak/xovi-extensions/reading-qol/（2026-09-11 大整理搬出 git 仓库时带走的），2026-09-13
# 给这台设备装第三方 WeRead app 时又手动改过一次、手动重新部署过——这次把最终版本捞回
# packaging/，往后走这个脚本，不用再记住那一串手动步骤。
#
# 两份 qmd 二选一（同一个仓库都带，脚本按设备实际装了什么选）：
#   sidebar-entry-koreader-only.qmd      只有 KOReader 一项（WeRead 没装时用这份，避免装出一个
#                                         点了没反应的按钮）
#   sidebar-entry-koreader-weread.qmd    KOReader + WeRead 两项（探测到设备装了 WeRead 时用这份）
# 图标资源 sidebar-icons.qrc + 两张 png 本地用 rcc 编译成 cangjie-icons.rcc 再推上去（跟
# qt-resource-rebuilder 已经在用的资源文件同名，直接覆盖）。
#
# 前置：设备已 vellum add qt-resource-rebuilder（qmd/rcc 靠它的 .rcc 通道加载，缺失时本脚本
# 探测不到 ~/xovi/exthome/qt-resource-rebuilder/ 就跳过，不算失败）+ 已 vellum add appload
# （本 qmd 的 onClicked 靠 appload 暴露的 CJAppLoad.AppLoadLauncher 单例发起启动，appload 没装
# 这个调用打不到目标，缺失时探测不到 ~/xovi/exthome/appload/ 也跳过，不算失败）。KOReader 本身
# 是否已经通过 appload 侧载不在本脚本探测范围——沿用 2026-09-02 首版设计（未装 KOReader 时这个
# 按钮点了也没反应，属已知取舍，见 sidebar-entry-koreader-only.qmd 头注）。
#
# 【appload 在 3.28 上要打过 PR #59 兼容补丁】appload v0.5.3 自带的内嵌 qmd 钩的是 3.27 的旧
# Sidebar/MainView 锚点，3.28 已经改了名字——没打这个补丁时 qmldiff 会报 "Couldn't resolve
# the hashed identifier"，appload 自己往 MainView 注入的常驻 Loader（CJAppLoad.AppLoadLauncher
# 单例就活在这个 Loader 里）根本建不起来，这样即使本脚本把 Sidebar 按钮插上去了，点了也没反应
# ——不是本脚本的 bug，是 appload 那份 .so 本身在这个固件版本上不兼容。检测靠读当前这次开机
# 的 journalctl：appload 自己的 qmd 处理成功会打一行 "Loaded external AppLoad hooks in main
# UI"；没这行说明大概率没打过这个补丁（或者压根还没重启过 xochitl 应用刚装好的 appload），本
# 脚本探测不到就跳过、不硬装一个不会响应的按钮。真要修：见
# `oldbak/xovi-extensions/reading-qol/tools/appload_patch_328.py` + 该目录 README「3.28 适配」
# 一节——这一步需要拿到上游 PR #59 的 qmd 文本手动打补丁，不是 install-all.sh 能代劳的，`vellum
# add appload` 装的是官方发行版，不会带这个第三方未合并的修复。
#
# 用法：./deploy-sidebar-entry.sh [host]      host 默认 10.11.99.1
#   环境 DEFER_XOVI_START=1：只把 qmd/rcc 落盘，不在这一步跑 xovi/start——install-all.sh 编排
#   多个 xovi 扩展时用这个避免短时间内反复重启 xochitl（撞 watchdog+StartLimit 的风险，
#   2026-09-11 真机踩过），改成全部落盘完最后统一跑一次（deploy-xovi-apply.sh）。单独跑本脚本
#   不用管这个变量，默认行为不变（装完立即 xovi/start 生效 + 健康检查）。
set -eu
cd "$(dirname "$0")"
HOST="${1:-10.11.99.1}"
QRR_DIR=/home/root/xovi/exthome/qt-resource-rebuilder
RCC_LOCAL="$(mktemp -t sidebar-icons.XXXXXX.rcc)"
trap 'rm -f "$RCC_LOCAL"' EXIT

echo "== 探测设备端 qt-resource-rebuilder =="
if ! ssh "root@$HOST" "[ -d $QRR_DIR ]"; then
    echo "-- 设备没装 qt-resource-rebuilder（vellum add qt-resource-rebuilder）——跳过，非失败"
    exit 0
fi

echo "== 探测设备端 appload =="
if ! ssh "root@$HOST" "[ -d /home/root/xovi/exthome/appload ]"; then
    echo "-- 设备没装 appload（vellum add appload）——跳过，非失败"
    exit 0
fi

echo "== 探测 appload 自己的 qmd 在这台固件上是否兼容 =="
# 正面信号："Loaded external AppLoad hooks in main UI" 是 appload 自己那份内嵌 qmd 成功处理后
# 打的日志——没这行不代表一定没打过 PR #59 补丁（也可能是装完 appload 后还没重启过 xochitl），
# 但按钮多半点了没反应，所以一律当作"暂不满足"处理，不硬装。
# ⚠ 这条只是"本次开机内某个时刻出现过"的一次性判据，不代表现在正在跑的 xochitl 就是那次成功
# 挂载的同一个实例——如果这行日志之后设备上跑过 `vellum upgrade`（会用未打补丁的官方版本盖掉
# appload）却还没重启过 xochitl，这里还是会读到旧的成功信号（2026-09-15
# 全量代码审查审出）。真正当次生效与否，靠下面本脚本自己触发的这次重启之后重新核对同一行信号
# （`DEFER_XOVI_START=1` 模式不在这一步重启，没法当场复核，见该分支注释）。
if ! ssh "root@$HOST" "journalctl -b 0 -u xochitl --no-pager 2>/dev/null | grep -q 'Loaded external AppLoad hooks in main UI'"; then
    echo "-- 没在这次开机日志里看到 appload 成功挂载的信号（可能是 appload 在这个固件版本上没打"
    echo "   过 PR #59 兼容补丁，也可能是刚装完 appload 还没 xovi/start 过一次）——跳过，非失败。"
    echo "   见本脚本头注「appload 在 3.28 上要打过 PR #59 兼容补丁」一节。"
    exit 0
fi

echo "== 探测设备是否已装第三方 WeRead app =="
if ssh "root@$HOST" "[ -x /home/root/.local/opt/remarkable-weread/bin/start-remarkable-weread.sh ]"; then
    QMD_SRC=sidebar-entry-koreader-weread.qmd
    echo "-- 装了 WeRead，用 $QMD_SRC（KOReader + WeRead 两项）"
else
    QMD_SRC=sidebar-entry-koreader-only.qmd
    echo "-- 没装 WeRead，用 $QMD_SRC（只有 KOReader 一项）"
fi

echo "== 本地编译图标资源 =="
if ! command -v rcc >/dev/null 2>&1; then
    echo "!! 本机没有 rcc（Qt Resource Compiler，随 Qt 开发包/qt6-base-devel 一类包提供）"
    exit 1
fi
rcc --binary -o "$RCC_LOCAL" sidebar-icons.qrc

echo "== 备份设备上现有的 qmd/rcc（若存在）=="
# shellcheck disable=SC2029  # 远端路径固定字面量，无用户输入拼接风险
ssh "root@$HOST" "
    [ -f $QRR_DIR/koreader-sidebar-entry.qmd ] && cp $QRR_DIR/koreader-sidebar-entry.qmd $QRR_DIR/koreader-sidebar-entry.qmd.bak.pre-sidebar-entry-deploy
    [ -f $QRR_DIR/cangjie-icons.rcc ] && cp $QRR_DIR/cangjie-icons.rcc $QRR_DIR/cangjie-icons.rcc.bak.pre-sidebar-entry-deploy
    true
"

echo "== 推送到 root@$HOST =="
scp "$QMD_SRC" "root@$HOST:$QRR_DIR/koreader-sidebar-entry.qmd"
scp "$RCC_LOCAL" "root@$HOST:$QRR_DIR/cangjie-icons.rcc"

echo "== md5 校验 =="
LOCAL_QMD_MD5="$(md5sum "$QMD_SRC" | awk '{print $1}')"
LOCAL_RCC_MD5="$(md5sum "$RCC_LOCAL" | awk '{print $1}')"
REMOTE_MD5S="$(ssh "root@$HOST" "md5sum $QRR_DIR/koreader-sidebar-entry.qmd $QRR_DIR/cangjie-icons.rcc" | awk '{print $1}')"
REMOTE_QMD_MD5="$(echo "$REMOTE_MD5S" | sed -n 1p)"
REMOTE_RCC_MD5="$(echo "$REMOTE_MD5S" | sed -n 2p)"
if [ "$LOCAL_QMD_MD5" != "$REMOTE_QMD_MD5" ] || [ "$LOCAL_RCC_MD5" != "$REMOTE_RCC_MD5" ]; then
    echo "!! md5 对不上（qmd: $LOCAL_QMD_MD5 vs $REMOTE_QMD_MD5；rcc: $LOCAL_RCC_MD5 vs $REMOTE_RCC_MD5）"
    exit 1
fi
echo "-- md5 一致"

if [ "${DEFER_XOVI_START:-0}" = "1" ]; then
    echo "-- DEFER_XOVI_START=1：只落盘，不在这一步跑 xovi/start（由后续统一步骤处理）"
    echo "   ⚠ appload 兼容信号只在上面探测的那一刻核对过，这一步不重启就没法当场复核；"
    echo "     后续统一步骤（deploy-xovi-apply.sh）真正重启后如果按钮点了没反应，先查"
    echo "     一遍 appload 是不是重启前又被 vellum upgrade 覆盖过。"
    exit 0
fi

echo "== 设备端跑一次 xovi/start 让新 qmd/rcc 生效 + 健康检查 =="
# 重启前打个时间戳，重启后拿它重新核对 appload 兼容信号——只信"本次重启之后新出现的"这一条，
# 不再相信上面探测阶段那次可能已经过期的"本次开机内某个时刻出现过"（见上面探测那步的头注）。
SINCE="$(ssh "root@$HOST" "date '+%Y-%m-%d %H:%M:%S'")"
# shellcheck disable=SC2087  # heredoc 内变量就是要在本地展开，全部是固定字面量，无远端注入风险；
# $SINCE 是唯一需要传给远端的本地值，走位置参数（$1），不塞进带引号的 heredoc 正文里（那样只会被
# 远端 shell 当成它自己从未定义过的变量，展开成空字符串）。
ssh "root@$HOST" "sh -s" "$SINCE" <<'DEVICE_SCRIPT'
set -eu
SINCE="$1"
OLD_PID="$(systemctl show xochitl -p MainPID --value 2>/dev/null || echo 0)"
/home/root/xovi/start
sleep 5
STATE="$(systemctl is-active xochitl 2>/dev/null || true)"
NEW_PID="$(systemctl show xochitl -p MainPID --value 2>/dev/null || echo 0)"
NREST="$(systemctl show xochitl -p NRestarts --value 2>/dev/null || echo '?')"
echo "  is-active : $STATE   (期望 active)"
echo "  MainPID   : $OLD_PID -> $NEW_PID   (期望有变化)"
echo "  NRestarts : $NREST   (期望 0/不增)"
if [ "$STATE" != "active" ] || [ "$NEW_PID" = "0" ]; then
    echo "⚠️  健康检查未达预期。查 journalctl -u xochitl"
    exit 1
fi
if journalctl -u xochitl --since "$SINCE" --no-pager 2>/dev/null | grep -q 'Loaded external AppLoad hooks in main UI'; then
    echo "✅ 部署完成（appload 兼容信号在这次重启之后重新出现，不是复用重启前的旧信号）"
else
    echo "⚠️  部署已落盘、xochitl 重启健康，但这次重启之后没有重新看到 appload 兼容信号——"
    echo "   探测阶段那次可能已经过期（比如中间跑过 vellum upgrade 把 appload 换回未打补丁的"
    echo "   官方版本）。Sidebar 按钮大概率点了没反应，去 journalctl -u xochitl --since \"$SINCE\""
    echo "   核实，必要时重新走一遍「appload 3.28 免SDK补丁法」。"
    exit 1
fi
DEVICE_SCRIPT
