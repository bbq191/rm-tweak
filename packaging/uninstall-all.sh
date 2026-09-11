#!/bin/sh
# ═══════════════════════════════════════════════════════════════════════════
# cang-jie 全新设备统一卸载器（host 侧编排，2026-09-16 新写）—— install-all.sh 的对称卸载，
# 回应 packaging/README.md「已知缺口」里明确点名的"没有对称 uninstall-all.sh"。
#
# 只编排、不重新实现——依次在设备端停用/删除 install-all.sh 各步骤留下的东西：
#   chrony-boot-wakelock.service   停用 + 删 /usr 持久化单元（dm-verity 门，跟安装时同一套写法）
#   xovi-persist                   同上（xovi-reenable.service）
#   hl-snap / handwriting-stroke   从 extensions.d 摘除 .so（不碰 reading-qol.json 配置、
#                                  不碰 cangjie-backups/ 下的历史备份）
#   sidebar-entry                  从 qt-resource-rebuilder exthome 摘除 qmd/rcc
#   battop                         停用 + 删 /usr 单元（--purge 才连 /home/root/battop 数据删）
#   shelf                          调用已推送到设备的 shelf/uninstall.sh（默认保留用户数据，
#                                  见该脚本自己的 --purge）
#
# 明确不做的事（范围外，跟 install-all.sh「明确不做的事」对称）：
#   · chrony-cn.sh / timezone-cn.sh 不卸——它们是配置覆写（改 /etc/chrony.conf、/etc/localtime
#     指向），不是"装了一个独立的东西"，没有卸载语义。
#   · 不卸 vellum/xovi/qt-resource-rebuilder/appload 本体、不卸载 KOReader 侧载——这些从来
#     不是本项目装的，不该由本项目卸。
#   · 不碰中文化（输入法/候选栏/UI 汉化）——那条链路本来就没跟着 install-all.sh 装，见该脚本
#     「明确不做的事」。
#
# ⚠️ 摘掉 extensions.d/qt-resource-rebuilder 里的文件后，当前正在跑的 xochitl 进程内存里
# 还留着旧的映射——真正"生效"（不再被加载）要等下一次 xochitl 重启（含下次真机重启后的
# xovi/start）。本脚本不主动重启 xochitl：卸载不像安装那样有"装完立刻验证"的必要，强制重启
# 反而平白多一次触发 xochitl watchdog/StartLimit 的机会（install-all.sh 头注记录过的同一个
# 风险），没必要。
#
# 用法：./uninstall-all.sh [host] [--purge]
#     [--skip chrony-boot-wakelock,xovi-persist,hl-snap,handwriting-stroke,sidebar-entry,battop,shelf]
#   host    默认 10.11.99.1（USB）
#   --purge 额外清用户数据——目前只影响 battop（连 /home/root/battop 的二进制+历史采样数据一起
#           删）；shelf 不受这个 --purge 影响，那是四条独立业务线的用户数据，误删风险太高，
#           要删数据请单独 --skip=shelf 后自己跑 shelf/uninstall.sh --purge（见该脚本用法）。
#   --skip  逗号分隔，跳过指定的卸载步骤
# ═══════════════════════════════════════════════════════════════════════════
set -eu
cd "$(dirname "$0")"

HOST="${1:-10.11.99.1}"; [ $# -gt 0 ] && shift
PURGE=0
SKIP=""
for a in "$@"; do
    case "$a" in
        --purge) PURGE=1 ;;
        --skip=*) SKIP="${a#--skip=}" ;;
        --skip) ;;
        *) if [ "${_prev:-}" = "--skip" ]; then SKIP="$a"; else echo "!! 未知参数：$a"; exit 2; fi ;;
    esac
    case "$a" in --skip) _prev="$a" ;; *) _prev="" ;; esac
done

skip_has() { case ",$SKIP," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }

DONE=""
FAILED=""

run_step() {
    name="$1"; shift
    if skip_has "$name"; then
        echo; echo "-- 跳过 $name（--skip）"
        return 0
    fi
    echo; echo "═══ $name ═══"
    if "$@"; then
        DONE="$DONE $name"
    else
        echo "!! $name 失败（见上面这一步的原始报错）"
        FAILED="$FAILED $name"
    fi
}

# 停用 + 删一个装在 /usr 的 systemd 持久化单元（dm-verity 门 + remount rw/ro，跟对应
# deploy-*.sh 安装时用的同一套写法对称）。$1=unit 文件名（含 .service）
remove_usr_unit() {
    unit="$1"
    ssh "root@$HOST" "sh -s" "$unit" <<'DEVICE_SCRIPT'
set -eu
UNIT="$1"
SYSD=/usr/lib/systemd/system
systemctl disable --now "$UNIT" 2>/dev/null || true
if [ ! -f "$SYSD/$UNIT" ]; then
    echo "-- $SYSD/$UNIT 本来就不存在，跳过"
    exit 0
fi
if dmsetup ls --target verity 2>/dev/null | grep -q .; then
    echo "✋ dm-verity 激活，rootfs 不可写——单元已 disable，但 /usr 里的文件删不掉（下次固件"
    echo "   OTA 冲掉 rootfs 后这份 /usr 也会跟着没了，等同已卸载；OTA 之前它还在，但已经"
    echo "   disabled，不会自动运行）。"
    exit 0
fi
mount -o remount,rw /
rm -f "$SYSD/$UNIT" "$SYSD/multi-user.target.wants/$UNIT"
sync
mount -o remount,ro / || true
systemctl daemon-reload
echo "-- 已删 $SYSD/$UNIT 及其 wants 软链"
DEVICE_SCRIPT
}

uninstall_chrony_boot_wakelock() { remove_usr_unit chrony-boot-wakelock.service; }
uninstall_xovi_persist() { remove_usr_unit xovi-reenable.service; }

# 从 extensions.d 摘除一个 xovi 扩展本体（+ 清同名的 .crashed 崩溃标记）。不碰
# reading-qol.json（多个扩展共用同一份配置文件，卸一个不该动别人的开关）、不碰
# cangjie-backups/ 下的历史备份（那是回滚安全网，卸载不等于放弃回滚能力）。$1=文件名（不含路径）
remove_xovi_extension() {
    so="$1"
    ssh "root@$HOST" "sh -s" "$so" <<'DEVICE_SCRIPT'
set -eu
SO="$1"
EXT=/home/root/xovi/extensions.d
if [ ! -f "$EXT/$SO" ]; then
    echo "-- $EXT/$SO 本来就不存在，跳过"
    exit 0
fi
rm -f "$EXT/$SO" "$EXT/$SO.crashed"
echo "-- 已从 extensions.d 摘除 $SO（reading-qol.json 配置、cangjie-backups/ 下的历史备份不动）"
DEVICE_SCRIPT
}

uninstall_hl_snap() { remove_xovi_extension hl-snap.so; }
uninstall_handwriting_stroke() { remove_xovi_extension hw-stroke.so; }

uninstall_sidebar_entry() {
    ssh "root@$HOST" "sh -s" <<'DEVICE_SCRIPT'
set -eu
QRR_DIR=/home/root/xovi/exthome/qt-resource-rebuilder
if [ ! -d "$QRR_DIR" ]; then
    echo "-- 设备没装 qt-resource-rebuilder，本来就没有这两个文件，跳过"
    exit 0
fi
rm -f "$QRR_DIR/koreader-sidebar-entry.qmd" "$QRR_DIR/cangjie-icons.rcc"
echo "-- 已从 qt-resource-rebuilder exthome 摘除 Sidebar 入口 qmd/rcc（.bak.pre-sidebar-entry-deploy 备份不动）"
DEVICE_SCRIPT
}

uninstall_battop() {
    ssh "root@$HOST" "sh -s" "$PURGE" <<'DEVICE_SCRIPT'
set -eu
PURGE="$1"
SYSD=/usr/lib/systemd/system
systemctl disable --now battop.service 2>/dev/null || true
if [ -f "$SYSD/battop.service" ]; then
    if dmsetup ls --target verity 2>/dev/null | grep -q .; then
        echo "✋ dm-verity 激活，rootfs 不可写——battop.service 已 disable，/usr 里的单元文件删不掉"
    else
        mount -o remount,rw /
        rm -f "$SYSD/battop.service"
        sync
        mount -o remount,ro / || true
        systemctl daemon-reload
        echo "-- 已删 $SYSD/battop.service"
    fi
else
    echo "-- $SYSD/battop.service 本来就不存在，跳过"
fi
if [ "$PURGE" = "1" ]; then
    rm -rf /home/root/battop
    echo "-- --purge：已删 /home/root/battop（二进制 + 历史采样数据）"
else
    echo "-- 保留 /home/root/battop（二进制 + 历史采样数据）；要连数据一起删加 --purge"
fi
DEVICE_SCRIPT
}

uninstall_shelf() {
    REMOTE_SCRIPT=/home/root/shelf-pkg/shelf/uninstall.sh
    if ! ssh "root@$HOST" "[ -f $REMOTE_SCRIPT ]"; then
        echo "-- 设备上没找到 $REMOTE_SCRIPT（shelf 从没部署过，或部署目录被手动清过），跳过"
        return 0
    fi
    # shellcheck disable=SC2029  # 远端路径固定字面量，无用户输入拼接风险
    ssh "root@$HOST" "sh $REMOTE_SCRIPT"
}

# 顺序：跟 install-all.sh 保持一一对应，方便对照；各步骤互不依赖，顺序本身不影响正确性。
run_step chrony-boot-wakelock uninstall_chrony_boot_wakelock
run_step xovi-persist uninstall_xovi_persist
run_step hl-snap uninstall_hl_snap
run_step handwriting-stroke uninstall_handwriting_stroke
run_step sidebar-entry uninstall_sidebar_entry
run_step battop uninstall_battop
run_step shelf uninstall_shelf

echo
echo "═══════════════════════════════════════════════════════════"
echo "已卸载：${DONE:-（无）}"
if [ -n "$FAILED" ]; then
    echo "❌ 失败：$FAILED —— 看对应步骤上面的原始报错，不会自动重试"
fi
echo "─── 不在本脚本范围内 ───"
echo "· chrony-cn.sh / timezone-cn.sh：配置覆写，没有卸载语义，不动"
echo "· vellum/xovi/qt-resource-rebuilder/appload 本体、KOReader 侧载：不代卸"
echo "· 中文化（输入法/候选栏/UI 汉化）：不在仓库版本控制内，本脚本管不到"
echo "· 以上改动多数要等下次 xochitl 重启（手动 systemctl restart xochitl，或下次真机重启后"
echo "    的 xovi/start）才会在当前运行中的进程里真正停止生效——本脚本不主动触发重启"
echo "═══════════════════════════════════════════════════════════"
[ -z "$FAILED" ]
