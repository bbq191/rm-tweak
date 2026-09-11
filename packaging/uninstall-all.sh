#!/bin/sh
# ═══════════════════════════════════════════════════════════════════════════
# cang-jie 全新设备统一卸载器（host 侧编排，2026-09-16 新写）—— install-all.sh 的对称卸载。
# 2026-09-20：步骤表与 install-all 共用 lib.sh（STEP_ORDER / STEP_CONFIG_ONLY），设备端动作走 devlib.sh；
# 每个非"配置覆写/纯动作"的安装步骤在这里都必须有 uninstall_<步骤名> 函数——tests/run_sim_tests.sh 会核对。
# 2026-09-22 审计：步骤按 install-all 的**逆序**执行；每步另清 deploy-* 推到设备的载荷目录；--dry-run；-h。
#
# 只编排、不重新实现——依次在设备端停用/删除 install-all.sh 各步骤留下的东西：
#   shelf                          调设备上的 shelf-uninstall（~/.local/bin，优先）或 shelf-pkg 里的 uninstall.sh，
#                                  默认保留用户数据，清单与 install 共用 manifest.sh；成功后删 shelf-pkg 载荷
#   sidebar-entry                  从 qt-resource-rebuilder exthome 摘除 qmd/rcc
#   hl-snap / handwriting-stroke   从 extensions.d 摘除 .so（不碰 reading-qol.json 配置、不碰 cangjie-backups/）
#   chrony-boot-wakelock / xovi-persist / wifi-watch   停用 + 删 /usr 单元（dm-verity 门，跟安装时同一套 devlib 写法）；
#                                  wifi-watch 另删 ~/.local/bin/wifi-watch.sh（单元删不掉时保留它，否则服务反复起不来）
#   battop                         停用 + 删 /usr 单元（--purge 才连 /home/root/battop 数据删）
#   另：每一步清掉 deploy-* 推到设备上的载荷目录（/home/root/pkg-<名>/、hl-snap/、hw-stroke/、shelf-pkg/——只 rm 已知文件
#   再 rmdir，目录里有别的东西就留着）；最后清 ~/.cangjie-stage 暂存目录（待生效标记不动：卸载摘掉的东西也要等 xochitl 重启才停止生效）。
#
# 明确不做的事（范围外，跟 install-all.sh「明确不做的事」对称）：
#   · chrony-cn.sh / timezone-cn.sh 不卸——它们是配置覆写（改 /etc/chrony.conf、/etc/localtime 指向），
#     不是"装了一个独立的东西"，没有卸载语义；xovi-apply 是纯动作。改前备份在设备 cangjie-backups/。
#   · 不卸 vellum/xovi/qt-resource-rebuilder/appload 本体、不卸载 KOReader 侧载——这些从来不是本项目装的。
#   · 不碰中文化（不在本仓库）。
#
# ⚠️ 摘掉 extensions.d/qt-resource-rebuilder 里的文件后，当前正在跑的 xochitl 进程内存里还留着旧的映射——
# 真正"生效"要等下一次 xochitl 启动。本脚本不主动重启（卸载没有"装完立刻验证"的必要）。要立刻停用一律
# **整机重启**（reboot）：2026-09-25 起查明停止 xochitl 本身就有概率在退出途中崩溃（devlib.sh 头注 H3），
# 跟摘没摘 .so 无关；xovi 已生效时也**不要** xovi/start。
#
# 用法：./uninstall-all.sh [host] [--purge] [--dry-run] [--skip a,b,...]
#   host      默认 10.11.99.1（USB）
#   --dry-run 只在本机打印将执行的卸载步骤，不连设备、不删任何东西
#   --purge   额外清用户数据——目前只影响 battop（连 /home/root/battop 的二进制+历史采样数据一起删）；
#             shelf 不受影响，那是几条独立业务线的用户数据，误删风险太高，要删请 --skip shelf 后自己跑
#             设备上的 shelf-uninstall --purge。
#   --skip    逗号分隔，跳过指定的卸载步骤（名字同 install-all）
# ═══════════════════════════════════════════════════════════════════════════
set -eu
cd "$(dirname "$0")"
# shellcheck disable=SC1091
. ./lib.sh

usage() {
    cat <<'USAGE_EOF'
用法：./uninstall-all.sh [host] [--purge] [--dry-run] [--skip a,b,...]
  host       默认 10.11.99.1（USB）
  --purge    额外清 battop 的二进制+历史采样数据（shelf 用户数据不受影响，见脚本头注）
  --dry-run  只在本机打印将执行的卸载步骤，不连设备、不删任何东西
  --skip     逗号分隔，跳过指定的卸载步骤（名字同 install-all）
USAGE_EOF
}

parse_step_args "$@"
[ "$FORCE" = "0" ] || { echo "!! 未知参数：--force（那是 install-all.sh 的）"; exit 2; }
[ "$FORCE_APPLY" = "0" ] || { echo "!! 未知参数：--force-apply（那是 install-all.sh 的）"; exit 2; }

if [ "$DRY" = "1" ]; then
    echo "═══ dry-run：只打印计划，不连设备（目标 root@$HOST，purge=$PURGE）═══"
else
    require_device
fi

# 卸一个 /usr 单元 + 清它的推送载荷目录。$1=单元名 $2=载荷目录名（$HOME 下）其余=载荷目录里的已知文件（先文件后子目录）
uninstall_usr_step() {
    dev_script "$@" <<'DEVICE_SCRIPT'
set -eu
UNIT="$1"; PKG="$2"; shift 2
cj_require_root || exit 1
cj_uninstall_usr_unit "$UNIT" multi-user.target.wants || exit 1
cj_rm_payload "$CJ_HOME/$PKG" "$@"
DEVICE_SCRIPT
}

# 载荷清单取 lib.sh 的 step_payload（与 deploy-* 推送同一份）；有意按词展开成 "目录 文件…"
# shellcheck disable=SC2046
uninstall_chrony_boot_wakelock() { uninstall_usr_step chrony-boot-wakelock.service $(step_payload chrony-boot-wakelock); }
# shellcheck disable=SC2046
uninstall_xovi_persist() { uninstall_usr_step xovi-reenable.service $(step_payload xovi-persist); }

# shellcheck disable=SC2046
uninstall_wifi_watch() { dev_script $(step_payload wifi-watch) <<'DEVICE_SCRIPT'
set -eu
PKG="$1"; shift
cj_require_root || exit 1
rc=0; cj_remove_usr_unit wifi-watch.service multi-user.target.wants || rc=$?
[ "$rc" = 0 ] || [ "$rc" = 3 ] || exit 1
if [ "$rc" = 3 ]; then
    # 单元（Restart=always）删不掉、重启后会被 wants 链接拉起——脚本必须留着，否则它会每 10 秒起一次并失败
    echo "-- 单元仍在 /usr，保留 ~/.local/bin/wifi-watch.sh（删了服务会反复起不来）"
else
    rm -f "$CJ_HOME/.local/bin/wifi-watch.sh"
    echo "-- 已删 ~/.local/bin/wifi-watch.sh（cangjie-backups/ 下的备份不动）"
    cj_rm_payload "$CJ_HOME/$PKG" "$@"
fi
DEVICE_SCRIPT
}

# 从 extensions.d 摘除一个 xovi 扩展本体（+ 清同名的 .crashed 崩溃标记 + 推送载荷目录）。不碰 reading-qol.json（多个扩展
# 共用同一份配置，卸一个不该动别人的开关）、不碰 cangjie-backups/（那是回滚安全网）。$1=.so 文件名 $2=步骤名
remove_xovi_extension() {
    # shellcheck disable=SC2046  # step_payload 有意按词展开成 "目录 文件…"
    dev_script "$1" $(step_payload "$2") <<'DEVICE_SCRIPT'
set -eu
SO="$1"; PKG="$2"; shift 2
cj_require_root || exit 1
EXT="$CJ_XOVI/extensions.d"
# 待换入区里的新版也要撤掉：否则下一次 cj_xochitl_apply（xovi-apply / 任何单独部署）会把刚卸掉的扩展又换进 extensions.d
cj_so_unstage "$SO"
MAPPED=0
if cj_xochitl_has_xovi && [ "$(cj_count_maps "$SO" "$(cj_xochitl_pid)")" -gt 0 ]; then MAPPED=1; fi
if [ -f "$EXT/$SO" ] || [ -e "$EXT/$SO.crashed" ]; then
    rm -f "$EXT/$SO" "$EXT/$SO.crashed"
    echo "-- 已从 extensions.d 摘除 $SO（reading-qol.json 配置、cangjie-backups/ 下的历史备份不动）"
else
    echo "-- $EXT/$SO 本来就不存在"
fi
if [ "$MAPPED" = "1" ]; then
    # 与 devlib.sh 头注 H3 同类：运行中的 xochitl 还映射着刚删掉的 .so，此时让它退出（restart/stop）有崩溃→整机重启的风险
    echo "   ⚠ 运行中的 xochitl 仍加载着 $SO（已删的旧文件）。要立刻停用请**整机重启**（reboot），别 systemctl restart xochitl。"
fi
cj_rm_payload "$CJ_HOME/$PKG" "$@"
DEVICE_SCRIPT
}
uninstall_hl_snap() { remove_xovi_extension hl-snap.so hl-snap; }
uninstall_handwriting_stroke() { remove_xovi_extension hw-stroke.so handwriting-stroke; }

uninstall_sidebar_entry() {
    dev_script <<'DEVICE_SCRIPT'
set -eu
cj_require_root || exit 1
QRR_DIR="$CJ_XOVI/exthome/qt-resource-rebuilder"
if [ ! -d "$QRR_DIR" ]; then
    echo "-- 设备没装 qt-resource-rebuilder，本来就没有这两个文件，跳过"
    exit 0
fi
rm -f "$QRR_DIR/koreader-sidebar-entry.qmd" "$QRR_DIR/cangjie-icons.rcc"
echo "-- 已从 qt-resource-rebuilder exthome 摘除 Sidebar 入口 qmd/rcc（备份在 cangjie-backups/，不动）"
# cangjie-icons.rcc 与历史上别的注入用过同一个文件名——如果你还有别的 qmd 依赖它，从备份里还原最近一份
LAST_RCC="$(ls -1 "$CJ_BACKUP_DIR" 2>/dev/null | grep '^cangjie-icons\.rcc\.bak\.pre-' | sort | tail -n 1 || true)"
if [ -n "$LAST_RCC" ]; then echo "   （若有别的 qmd 需要它：cp $CJ_BACKUP_DIR/$LAST_RCC $QRR_DIR/cangjie-icons.rcc）"; fi
DEVICE_SCRIPT
}

uninstall_battop() {
    dev_script "$PURGE" <<'DEVICE_SCRIPT'
set -eu
PURGE="$1"
cj_require_root || exit 1
# battop 有意不建开机链接（见 enhance/battop/install.sh），这里顺手清可能的旧链接
cj_uninstall_usr_unit battop.service multi-user.target.wants || exit 1
if [ "$PURGE" = "1" ]; then
    BD="$CJ_HOME/battop"
    case "$BD" in /?*/battop) ;; *) echo "!! 拒绝清除异常路径 $BD"; exit 1 ;; esac
    [ -L "$BD" ] && { echo "!! $BD 是符号链接，拒绝清除"; exit 1; }
    if [ -d "$BD" ]; then
        echo "-- --purge：将删除 $BD（$(du -sk "$BD" 2>/dev/null | awk '{print $1}') KB：二进制 + 历史采样数据）"
        rm -rf "$BD"
    fi
    echo "-- --purge：已删 $BD"
else
    echo "-- 保留 $CJ_HOME/battop（二进制 + 历史采样数据）；要连数据一起删加 --purge"
fi
DEVICE_SCRIPT
}

uninstall_shelf() {
    # 优先用已装的 ~/.local/bin/shelf-uninstall（install.sh 每次更新它，是单一事实源）；没有再退回 shelf-pkg 里的副本
    # （旧设备上 shelf-pkg 可能是很久以前的载荷）。成功后删 shelf-pkg 载荷（重装由 deploy.sh 重新推送）。
    dev_script <<'DEVICE_SCRIPT'
set -eu
rc=0
if [ -f "$CJ_HOME/.local/bin/shelf-uninstall" ]; then
    sh "$CJ_HOME/.local/bin/shelf-uninstall" || rc=$?
elif [ -f "$CJ_HOME/shelf-pkg/shelf/uninstall.sh" ]; then
    echo "-- 没有 ~/.local/bin/shelf-uninstall，退回 shelf-pkg 里的 uninstall.sh"
    sh "$CJ_HOME/shelf-pkg/shelf/uninstall.sh" || rc=$?
else
    echo "-- 设备上没找到 shelf-uninstall / shelf-pkg（shelf 从没部署过，或被手动清过），跳过"
fi
[ "$rc" = 0 ] || exit "$rc"
# dm-verity 等原因让 shelf-uninstall 保留了二进制（网关还在）时，载荷里的 uninstall.sh 是"可写后再卸一次"的退路，不能删
if [ -e "$CJ_HOME/.local/bin/gateway" ]; then
    echo "-- 书架二进制仍在（卸载未彻底完成，见上）——保留 shelf-pkg 载荷，可写后重跑本脚本"
    exit 0
fi
# shelf-pkg / shelf-pkg.new 是 deploy.sh 推来的载荷（固定路径、必须是真目录且含 shelf/ 载荷标记才删）
for d in "$CJ_HOME/shelf-pkg" "$CJ_HOME/shelf-pkg.new"; do
    [ -d "$d" ] || continue
    [ -L "$d" ] && { echo "!! $d 是符号链接，不删"; continue; }
    if [ -d "$d/shelf" ]; then rm -rf "$d"; echo "-- 已删载荷目录 $d"; else echo "-- $d 里没有 shelf/ 载荷标记，不删"; fi
done
DEVICE_SCRIPT
}

# 通用收尾：暂存目录里只有本项目的中转文件，清掉（不在 STEP_ORDER 里、不计入"已卸载"清单）
cleanup_staging() {
    dev_script <<'DEVICE_SCRIPT'
set -eu
for f in "$CJ_STAGE_DIR"/.*.new.* "$CJ_STAGE_DIR"/koreader-sidebar-entry.qmd "$CJ_STAGE_DIR"/cangjie-icons.rcc; do
    [ -f "$f" ] && rm -f "$f"
done
cj_stage_cleanup
DEVICE_SCRIPT
}

# 逆序执行 install-all 的步骤表；配置覆写/纯动作步骤没有卸载语义，跳过
REV=""
for step in $STEP_ORDER; do REV="$step $REV"; done
for step in $REV; do
    if word_in "$step" "$STEP_CONFIG_ONLY"; then continue; fi
    fn="uninstall_$(echo "$step" | tr '-' '_')"
    run_step "$step" "$fn"
done
[ "$DRY" = "1" ] || cleanup_staging || true

echo
echo "═══════════════════════════════════════════════════════════"
if [ "$DRY" = "1" ]; then
    echo "dry-run 计划（未连接设备、未执行）：${DONE:-（无）}"
else
    echo "已卸载：${DONE:-（无）}"
fi
[ -z "$SKIPPED" ] || echo "已跳过（--skip）：$SKIPPED"
if [ -n "$FAILED" ]; then
    echo "❌ 失败：$FAILED —— 看对应步骤上面的原始报错，不会自动重试"
fi
echo "─── 不在本脚本范围内 ───"
echo "· chrony-cn.sh / timezone-cn.sh：配置覆写，没有卸载语义，不动（改前备份在设备 /home/root/cangjie-backups/，要还原自己取）"
echo "· vellum/xovi/qt-resource-rebuilder/appload 本体、KOReader 侧载：不代卸"
echo "· ~/.local/share/cangjie-ime/reading-qol.json（各扩展共用的设置）与 cangjie-backups/（回滚备份）：保留，确认无用后可手动删"
echo "· 中文化（输入法/候选栏/UI 汉化）：不在本仓库，本脚本管不到"
echo "· 以上改动多数要等下次 xochitl 重启才会在当前运行中的进程里真正停止生效——本脚本不主动触发重启；"
echo "    摘了 xovi 扩展 .so（hl-snap/handwriting-stroke）而 xochitl 还加载着它：要立刻停用请整机重启（reboot，见上面该步的提示）；"
echo "      这种状态下任何 stop/restart xochitl 都会让它退出途中崩溃、再由系统整机重启（2026-09-25 真机）——所以直接 reboot；"
echo "      紧接着重装也没问题：install-all 最后一步认得这种状态，会换入新版后主动整机重启，不再停 xochitl；"
echo "    只摘了 qmd（sidebar-entry/shelf）：同样整机重启（reboot）——停 xochitl 本身也会概率性退出途中崩溃；xovi 已生效时别用 xovi/start"
echo "═══════════════════════════════════════════════════════════"
[ -z "$FAILED" ]
