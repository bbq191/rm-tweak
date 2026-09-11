#!/bin/sh
# shellcheck shell=sh
# ═══════════════════════════════════════════════════════════════════════════
# xovi-ext-install.sh —— 设备侧"单个 xovi 扩展"通用安装器（被各扩展的 deploy/install.sh 薄包装 source）。
#
# 2026-09-20 脚本审计：hl-snap 与 handwriting-stroke 两份 install.sh 归一化后 ~95% 逐字重复，且都
# ① 原地 cp 覆盖 xochitl 已映射的 .so（M4）② 不带 --no-restart 时无条件 xovi/start（H1）③ 备份自己写
# 一遍（M6）。现在数据（名字/配置键）留在各扩展的薄包装里，流程只此一份。
#
# 包装需在 source 本文件前设好：
#   EXT_NAME     扩展名（日志用）              EXT_SO       .so 文件名（在 payload 目录里，即 deploy/ 的上一级）
#   EXT_MAPTAG   /proc/PID/maps 里用来确认已加载的字符串
#   RQOL_INIT    首次装才建的 reading-qol.json 内容（已有非空文件绝不覆盖）
#   RQOL_MSG     建配置时的提示           OK_MSG / NEXT_MSG   成功后的提示
#   HERE         包装脚本所在目录（要求同目录有本文件与 devlib.sh；由 deploy-xovi-ext.sh 一起推送）
# 用法（包装转发参数）：install.sh [--no-restart]
#   --no-restart  只落盘，不重启 xochitl（外部编排方 install-all 用，最后统一重启一次）
# ═══════════════════════════════════════════════════════════════════════════

# shellcheck disable=SC1091
. "$HERE/devlib.sh"

NO_RESTART=0
for a in "$@"; do
    case "$a" in
        --no-restart) NO_RESTART=1 ;;
        *) echo "!! 未知参数：$a"; exit 2 ;;
    esac
done

PAYLOAD="$(dirname "$HERE")"   # deploy/ 的上一级，.so 在这
EXTDIR="$CJ_XOVI/extensions.d"
DATADIR="$CJ_HOME/.local/share/cangjie-ime"   # 复用同一个 reading-qol.json

echo "== $EXT_NAME 安装（vellum-xovi 结构，只碰 /home，不碰 /usr）=="

cj_require_root || exit 1
[ -f "$CJ_XOVI/xovi.so" ] || { echo "!! 没找到 $CJ_XOVI/xovi.so —— 先跑：vellum add xovi"; exit 1; }
[ -f "$PAYLOAD/$EXT_SO" ] || { echo "!! 没找到 $PAYLOAD/$EXT_SO，先在 host 侧构建（make aarch64）再部署"; exit 1; }

# 三种情况（旧版备份进 cangjie-backups——绝不能留在 extensions.d：xovi 把该目录下任意文件当扩展加载，
# 同名扩展重复注册是致命错误；内容没变就不备份，只保留最近几份）：
#  · 与已装的逐字节相同 → 不动（顺手撤掉过时的待换入版本）；
#  · 运行中的 xochitl 正映射着它 → 不当场换，放进待换入区，由 cj_xochitl_apply 换入后整机重启（或下次开机由 xovi-reenable 换入）
#    （换完再 restart 会让旧进程退出时崩溃、整机重启，2026-09-24 真机第二次复现，见 devlib.sh 头注 H3）；
#    同一个新版已经在待换入区（上一轮 --no-restart 放进去、还没重启）→ 不再重复备份/重放；
#  · 否则原子替换：先写到 extensions.d 之外的暂存目录再 rename 进去，中途失败不在 extensions.d 里留半个 .so；
#    同样撤掉过时的待换入版本（否则下一次生效时它会把新版盖回旧版）。
EXT_CHANGED=0
if [ -f "$EXTDIR/$EXT_SO" ] && cmp -s "$PAYLOAD/$EXT_SO" "$EXTDIR/$EXT_SO"; then
    cj_so_unstage "$EXT_SO"
elif cj_xochitl_has_xovi && [ "$(cj_count_maps "$EXT_MAPTAG" "$(cj_xochitl_pid)")" -gt 0 ]; then
    if [ -f "$CJ_SO_PENDING_DIR/$EXT_SO" ] && cmp -s "$PAYLOAD/$EXT_SO" "$CJ_SO_PENDING_DIR/$EXT_SO"; then
        echo "-- 同一个新版 $EXT_SO 已在待换入区（$CJ_SO_PENDING_DIR），等重启 xochitl 时换入"
    else
        cj_backup_if_differs "$PAYLOAD/$EXT_SO" "$EXTDIR/$EXT_SO" || exit 1
        echo "-- 运行中的 xochitl 正在用旧版 $EXT_SO → 新版先放进待换入区（$CJ_SO_PENDING_DIR），重启 xochitl 时换入"
        cj_so_stage "$PAYLOAD/$EXT_SO" || { echo "!! 放入待换入区失败"; exit 1; }
    fi
    EXT_CHANGED=1
else
    cj_backup_if_differs "$PAYLOAD/$EXT_SO" "$EXTDIR/$EXT_SO" || exit 1
    echo "-- 装 $EXT_SO -> $EXTDIR/"
    cj_safe_replace "$PAYLOAD/$EXT_SO" "$EXTDIR/$EXT_SO" "$CJ_STAGE_DIR" 755 || { echo "!! 写 $EXTDIR/$EXT_SO 失败"; exit 1; }
    EXT_CHANGED="$CJ_REPLACED"
    # 待换入区里若还躺着更早一轮放进去的版本（放进去后设备没经 xovi-reenable 就重启过、xovi 暂未生效等），它已过时：
    # 不撤掉的话，接下来的 cj_xochitl_apply / 开机 xovi-reenable 会拿它把刚装好的新版盖回去（2026-09-25 审计）
    cj_so_unstage "$EXT_SO"
fi
if [ -e "$EXTDIR/$EXT_SO.crashed" ]; then EXT_CHANGED=1; rm -f "$EXTDIR/$EXT_SO.crashed"; fi   # 清旧崩溃标记（有过崩溃标记 = 需要重启重新载入一次）
cj_stage_cleanup

# reading-qol.json 首次装才建（不覆盖已有设置）；其它键留给别的功能各自维护，这里不动。
RQOL="$DATADIR/reading-qol.json"
if [ ! -s "$RQOL" ]; then
    echo "-- $RQOL_MSG -> $RQOL"
    mkdir -p "$DATADIR"
    printf '%s' "$RQOL_INIT" > "$RQOL"
fi

# 有变化就记待生效标记（两种模式都记：单独跑时重启失败，标记留着，之后 deploy-xovi-apply.sh 还能补上）
if [ "$EXT_CHANGED" = "1" ]; then cj_pending_mark "$EXT_NAME" || true; fi

if [ "$NO_RESTART" = "1" ]; then
    if [ "$EXT_CHANGED" = "1" ]; then
        echo "-- --no-restart：$EXT_SO 已落盘（有变化），未重启 xochitl（由外部编排方稍后统一执行一次）"
        echo "✅ 已就位，尚未生效——外部编排方跑完这轮 xochitl 重启后再确认"
    else
        echo "-- --no-restart：$EXT_SO 与设备上已装的逐字节相同，无需重启 xochitl"
        echo "✅ 已是最新"
    fi
    exit 0
fi

# 单独跑：没有任何东西要生效（本扩展没变且已在运行中的 xochitl 里加载、没有别的待生效改动、xovi 已生效）就不重启——
# 重复跑不再每次闪屏、不白白消耗 xochitl 的 StartLimit 名额
if [ "$EXT_CHANGED" = "0" ] && ! cj_apply_needed && [ "$(cj_count_maps "$EXT_MAPTAG" "$(cj_xochitl_pid)")" -gt 0 ]; then
    echo "-- $EXT_SO 与已装的逐字节相同且已加载，也没有别的待生效改动——不重启 xochitl"
    echo "✅ 已是最新"
    exit 0
fi

OLD_PID="$(cj_xochitl_pid)"
cj_xochitl_apply || exit 1
if [ "$CJ_APPLY_REBOOTED" = 1 ]; then
    exit 0   # 已排上整机重启；健康检查留给设备回来后的 verify-on-device.sh
fi
if cj_xochitl_health "$OLD_PID" "$EXT_MAPTAG" && [ "$(cj_count_maps "$EXT_MAPTAG" "$(cj_xochitl_pid)")" -gt 0 ]; then
    echo "✅ 安装完成。$OK_MSG"
    echo "$NEXT_MSG"
    echo "⚠️  真机重启后需恢复 xovi：装了 xovi-reenable.service 会自动；否则手动 /home/root/xovi/start（或 vellum reenable）"
else
    echo "⚠️  健康检查未达预期。查 journalctl -u xochitl | grep $EXT_MAPTAG"
    exit 1
fi
