#!/bin/sh
# host 侧一键构建+推送 Sidebar 一级直达入口（KOReader，装了第三方 WeRead app 时自动带上它）。
#
# 2026-09-13 从「手动 SSH 上机部署」捞回 packaging/ 变成可重复脚本：源 QML 补丁本来在
# 2026-09-11 大整理时搬出 git 仓库的 reading-qol/ 里，2026-09-13
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
# 【appload 要 ≥ 0.6.0 才兼容 3.28】appload 0.5.3 自带的内嵌 qmd 钩的是 3.27 的旧 Sidebar/MainView
# 锚点，3.28 已经改了名字——qmldiff 会报 "Couldn't resolve the hashed identifier"，appload 自己往
# MainView 注入的常驻 Loader（CJAppLoad.AppLoadLauncher 单例就活在这个 Loader 里）建不起来，即使
# 本脚本把 Sidebar 按钮插上去了点了也没反应。上游 PR #59（3.28 支持）已并入 v0.6.0（2026-09-19，
# 同版还加了 3.29 支持），`vellum add/upgrade appload` 拿到的就是它；2026-09-21 真机验证（md5 与官方
# 发布包一致、xochitl 日志有 "Loaded external AppLoad hooks in main UI"、侧栏入口点开 KOReader/
# WeRead 正常）。此前用的"等长回填 qmd 进 .so"补丁工具已随之删除（git 历史可找回）。
# 检测靠读当前这次开机的 journalctl：appload 自己的 qmd 处理成功会打一行 "Loaded external AppLoad
# hooks in main UI"；没这行说明 appload 版本太旧（或刚装/升级完还没重启），本脚本探测不到就跳过、
# 不硬装一个不会响应的按钮。⚠ 换 appload 文件后**不要 `systemctl restart xochitl`**：运行中的旧
# 进程在退出时会 SIGSEGV，触发 xochitl 单元的 OnFailure=emergency.target 整机重启（2026-09-21
# 真机踩到）——换完直接整机重启，xovi 会自动生效。
#
# 用法：./deploy-sidebar-entry.sh [host]      host 默认 10.11.99.1
#   环境 DEFER_XOVI_START=1：只把 qmd/rcc 落盘，不在这一步重启 xochitl——install-all.sh 编排
#   多个 xovi 扩展时用这个避免短时间内反复重启 xochitl（撞 watchdog+StartLimit 的风险，
#   2026-09-11 真机踩过），改成全部落盘完最后统一重启一次（deploy-xovi-apply.sh）。单独跑本脚本
#   不用管这个变量：装完立即让它生效（qmd/rcc 没变、也没有别的待生效改动时什么都不做，2026-09-24）——
#   怎么生效由设备端 devlib.sh 的 cj_xochitl_apply 判定（2026-09-25 起：xovi 已生效或装了 xovi-reenable →
#   主动整机重启，设备回来后自动跑 verify-on-device.sh；都没有才 xovi/start），之前会提示"打断阅读"并留 5 秒宽限。
#
# 2026-09-20 改动（脚本审计）：qmd/rcc 先推到暂存目录并 md5 校验，通过后设备端才原子 rename 进 qrr 目录
# （旧版直接 scp 覆盖，md5 不符时坏文件已在 qrr 里）；旧文件备份进 cangjie-backups（保留最近几份），
# 不再在 qrr 目录里放 .bak.pre-*。
# 2026-09-25 审计：四项探测合成一次 ssh、落位与生效合成一次 ssh、重启前的时间戳在设备端取（连接数 14 → 6）。
set -eu
cd "$(dirname "$0")"
# shellcheck disable=SC1091
. ./lib.sh
host_arg "用法：./deploy-sidebar-entry.sh [host]      host 默认 10.11.99.1；环境 DEFER_XOVI_START=1 只落盘不重启 xochitl" "$@"
require_device
QRR_DIR=/home/root/xovi/exthome/qt-resource-rebuilder
STAGE="$CJ_STAGE_REMOTE"
RCC_LOCAL="$(mktemp -t sidebar-icons.XXXXXX.rcc)"
trap 'rm -f "$RCC_LOCAL"' EXIT

echo "== 探测设备端 qt-resource-rebuilder / appload / appload 兼容信号 / WeRead（一次连接）=="
# appload 兼容信号："Loaded external AppLoad hooks in main UI" 是 appload 自己那份内嵌 qmd 成功处理后打的日志——
# 没这行不代表 appload 一定太旧（也可能是装完 appload 后还没重启过），但按钮多半点了没反应，一律当"暂不满足"，不硬装。
# ⚠ 这条只是"本次开机内某个时刻出现过"的一次性判据，不代表现在正在跑的 xochitl 就是那次成功挂载的同一个实例——
# 这行日志之后设备上跑过 `vellum upgrade`/`vellum del appload` 却还没重启过，这里还是会读到旧的成功信号（2026-09-15
# 全量代码审查审出）。真正当次生效与否，靠下面本脚本自己触发的这次重启之后重新核对同一行信号
# （`DEFER_XOVI_START=1` 模式不在这一步重启，没法当场复核，见该分支注释）。
PROBE="$(dev_script "$QRR_DIR" <<'DEVICE_SCRIPT'
if [ ! -d "$1" ]; then echo NOQRR; exit 0; fi
if [ ! -d "$CJ_XOVI/exthome/appload" ]; then echo NOAPPLOAD; exit 0; fi
if ! journalctl -b 0 -u xochitl --no-pager 2>/dev/null | grep -q 'Loaded external AppLoad hooks in main UI'; then echo NOSIGNAL; exit 0; fi
if [ -x "$CJ_HOME/.local/opt/remarkable-weread/bin/start-remarkable-weread.sh" ]; then echo WEREAD; else echo KOREADER-ONLY; fi
DEVICE_SCRIPT
)" || { echo "!! 探测失败（ssh 中断？）"; exit 1; }
case "$PROBE" in
    NOQRR) step_skipped "设备没装 qt-resource-rebuilder（vellum add qt-resource-rebuilder）"; exit 0 ;;
    NOAPPLOAD) step_skipped "设备没装 appload（vellum add appload）"; exit 0 ;;
    NOSIGNAL)
        echo "-- 没在这次开机日志里看到 appload 成功挂载的信号（可能是 appload 版本 < 0.6.0、在 3.28"
        echo "   上不兼容，也可能是刚装/升级完 appload 还没重启设备）。"
        echo "   先 vellum upgrade appload 到 ≥ 0.6.0 并整机重启，见本脚本头注「appload 要 ≥ 0.6.0」一节。"
        step_skipped "没看到 appload 成功挂载的信号（appload < 0.6.0，或装/升级后还没重启设备）"
        exit 0 ;;
    WEREAD) QMD_SRC=sidebar-entry-koreader-weread.qmd; echo "-- 装了 WeRead，用 $QMD_SRC（KOReader + WeRead 两项）" ;;
    KOREADER-ONLY) QMD_SRC=sidebar-entry-koreader-only.qmd; echo "-- 没装 WeRead，用 $QMD_SRC（只有 KOReader 一项）" ;;
    *) echo "!! 探测结果看不懂：$PROBE"; exit 1 ;;
esac

echo "== 本地编译图标资源 =="
if ! command -v rcc >/dev/null 2>&1; then
    echo "!! 本机没有 rcc（Qt Resource Compiler，随 Qt 开发包/qt6-base-devel 一类包提供）"
    exit 1
fi
rcc --binary -o "$RCC_LOCAL" sidebar-icons.qrc

echo "== 推送到设备暂存目录（md5 校验；通过前不碰 qrr 目录）=="
push_verified "$QMD_SRC" "$STAGE/koreader-sidebar-entry.qmd" "$RCC_LOCAL" "$STAGE/cangjie-icons.rcc"

DEFER="${DEFER_XOVI_START:-0}"
if [ "$DEFER" = "1" ]; then
    echo "== 设备端落位（备份进 cangjie-backups + 原子 rename）=="
else
    echo "== 设备端落位 + 让新 qmd/rcc 生效（有变化才整机重启，会打断设备上的阅读/书写）=="
fi
# 落位与生效同一次 ssh。DEFER=1 时只落位（run_apply 看不到 CJ-APPLY-REBOOTING，原样返回退出码）
run_apply dev_script "$QRR_DIR" "$STAGE" "$DEFER" <<'DEVICE_SCRIPT'
set -eu
QRR="$1"; STG="$2"; DEFER="$3"
cj_require_root || exit 1
[ -f "$STG/koreader-sidebar-entry.qmd" ] && [ -f "$STG/cangjie-icons.rcc" ] || { echo "!! 暂存文件缺失"; exit 1; }
for f in koreader-sidebar-entry.qmd cangjie-icons.rcc; do
    cj_backup_if_differs "$STG/$f" "$QRR/$f"   # 内容没变就不堆重复备份
done
cj_safe_replace "$STG/koreader-sidebar-entry.qmd" "$QRR/koreader-sidebar-entry.qmd" "$STG" 644
CH1="$CJ_REPLACED"
cj_safe_replace "$STG/cangjie-icons.rcc" "$QRR/cangjie-icons.rcc" "$STG" 644
CH2="$CJ_REPLACED"
if [ "$CH1$CH2" != "00" ]; then cj_pending_mark sidebar-entry || true; fi   # 有变化才需要重启 xochitl 才生效
rm -f "$STG/koreader-sidebar-entry.qmd" "$STG/cangjie-icons.rcc"
rmdir "$STG" 2>/dev/null || true
echo "-- 已落位 $QRR/{koreader-sidebar-entry.qmd,cangjie-icons.rcc}"

if [ "$DEFER" = "1" ]; then
    echo "-- DEFER_XOVI_START=1：只落盘，不在这一步重启 xochitl（由后续统一步骤处理）"
    echo "   ⚠ appload 兼容信号只在上面探测的那一刻核对过，这一步不重启就没法当场复核；"
    echo "     后续统一步骤（deploy-xovi-apply.sh）真正重启后如果按钮点了没反应，先查"
    echo "     一遍 appload 版本是否 ≥ 0.6.0、是不是重启前又被 vellum 换过。"
    exit 0
fi
# qmd/rcc 没变（上面落位时没记标记）、也没有别的待生效改动、xovi 已生效 → 不重启：重复跑不再每次闪屏
if ! cj_apply_needed; then
    echo "-- qmd/rcc 与设备上已装的逐字节相同，也没有别的待生效改动——不重启 xochitl"
    echo "✅ 已是最新"
    exit 0
fi
# 重启前打个时间戳，重启后拿它重新核对 appload 兼容信号——只信"本次重启之后新出现的"这一条，
# 不再相信探测阶段那次可能已经过期的"本次开机内某个时刻出现过"
SINCE="$(date '+%Y-%m-%d %H:%M:%S')"
OLD_PID="$(cj_xochitl_pid)"
cj_xochitl_apply || exit 1
if [ "$CJ_APPLY_REBOOTED" = 1 ]; then
    echo "   设备回来后核对 appload 兼容信号：journalctl -u xochitl -b | grep 'Loaded external AppLoad hooks in main UI'"
    exit 0   # 已排上整机重启；健康检查与兼容信号核对留给设备回来后
fi
cj_xochitl_health "$OLD_PID" || { echo "⚠️  健康检查未达预期。查 journalctl -u xochitl"; exit 1; }
if journalctl -u xochitl --since "$SINCE" --no-pager 2>/dev/null | grep -q 'Loaded external AppLoad hooks in main UI'; then
    echo "✅ 部署完成（appload 兼容信号在这次重启之后重新出现，不是复用重启前的旧信号）"
else
    echo "⚠️  部署已落盘、xochitl 重启健康，但这次重启之后没有重新看到 appload 兼容信号——"
    echo "   探测阶段那次可能已经过期（比如中间 appload 被换成了 < 0.6.0 的版本）。"
    echo "   Sidebar 按钮大概率点了没反应，去 journalctl -u xochitl --since \"$SINCE\" 核实，"
    echo "   必要时 vellum upgrade appload 到 ≥ 0.6.0 后整机重启。"
    exit 1
fi
DEVICE_SCRIPT
