#!/bin/sh
# battop 采集器 安装/OTA 重建（设备端跑，root）。
# 二进制+数据在 /home/root/battop（OTA 不丢）；rootfs 只放 battop.service。
# OTA 后重跑：sh /home/root/battop/install.sh（需要同目录 devlib.sh 与 battop 或 battop.new；
# 由 packaging/deploy-battop.sh 一起推送）。
#
# 输入：$DIR/battop.new（deploy-battop.sh 推来的新二进制，md5 已校验）优先，原子 rename 成 $DIR/battop；
#       没有 .new 时要求 $DIR/battop 已在（OTA 后只重装单元）。
#
# 【有意不开机自启】（2026-09-20 查证记忆 battop-brick-cgroup-rcu-hang 后定案）：
#   2026-08-29 battop 采样触发内核 cgroup/RCU 死锁冻死整机；改常驻后暴露向量 144 次/天→1 次/开机，但根因是
#   内核 RCU stall、未彻底排除。旧安装器 `systemctl enable --now` 的 enable 链接落在 /etc tmpfs，重启即清——
#   "重启后 battop 不自启"实际上一直是被记忆点名保留的缓解。这里把它变成明说的设计：只 start、**不 enable**、
#   不在 /usr 建 wants 链接；要用就在网页「管理→电池刺客」开（POST /api/enhance/battop/start）或手动
#   `systemctl start battop`。想恢复开机自启是有意的决定，需自己评估后再加，别靠安装器悄悄做。
#
# 启动策略：单元首次安装 → start；已在跑且二进制/单元有变化 → restart（载入新版，仅一次 cgroup 迁移）；
#           已在跑且无变化 → 不动；
#           已装但当前停着（用户在网页关了）→ 保持停着。--start 强制启动，--no-start 不启动。
#           dm-verity 激活：单元以前装过 → 照上面的规则（二进制变了且在跑才重启）；从没装过 → 退出码 10（跳过，非失败）。
set -eu
DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck disable=SC1091
. "$DIR/devlib.sh"

MODE=auto
for a in "$@"; do
    case "$a" in
        --start) MODE=start ;;
        --no-start) MODE=no ;;
        *) echo "!! 未知参数：$a"; exit 2 ;;
    esac
done
UNIT=battop.service

cj_require_root || exit 1
if [ -f "$DIR/battop.new" ]; then
    NEWBIN="$DIR/battop.new"
elif [ -x "$DIR/battop" ]; then
    NEWBIN="$DIR/battop"
else
    echo "缺 $DIR/battop.new 或 $DIR/battop（先用 deploy-battop.sh 推二进制）"; exit 1
fi
mkdir -p "$DIR/data"

# 单元内容（常驻 Type=simple：进程内部循环每 ~10min 采一次。**不再用 timer 反复拉起 oneshot**——
# 反复 service-start 的 cgroup 迁移曾撞内核 RCU stall 冻死整机，见 FINDINGS）
UNIT_SRC="$CJ_STAGE_DIR/battop.service.src"
mkdir -p "$CJ_STAGE_DIR"
cat > "$UNIT_SRC" <<'UNIT'
[Unit]
Description=battop battery/usage sampler (resident, ~10min interval)

[Service]
Type=simple
Nice=19
IOSchedulingClass=idle
ExecStart=/home/root/battop/battop
Restart=on-failure
RestartSec=60

[Install]
WantedBy=multi-user.target
UNIT

WAS_ACTIVE="$(systemctl is-active "$UNIT" 2>/dev/null || true)"
UNIT_EXISTED=0
[ -f "$CJ_SYSD/$UNIT" ] && UNIT_EXISTED=1

# 先把二进制换好（备份进 cangjie-backups；rename 原子，运行中的进程继续用旧 inode）
BIN_CHANGED=0
if [ "$NEWBIN" != "$DIR/battop" ]; then
    if [ -f "$DIR/battop" ] && ! cmp -s "$NEWBIN" "$DIR/battop"; then cj_backup_file "$DIR/battop"; fi
    cj_safe_replace "$NEWBIN" "$DIR/battop" "$DIR" 755 || { echo "!! 写 $DIR/battop 失败"; rm -f "$UNIT_SRC"; exit 1; }
    BIN_CHANGED="$CJ_REPLACED"
    rm -f "$NEWBIN"
fi

# 单元：dm-verity 门 + 带 trap 的 rw 窗口（cj_install_usr_unit）；wants 传 "-" = 不建开机链接（见头注）
rc=0
cj_install_usr_unit "$UNIT" "$UNIT_SRC" - || rc=$?
rm -f "$UNIT_SRC"
[ -d "$CJ_STAGE_DIR" ] && rmdir "$CJ_STAGE_DIR" 2>/dev/null || true
VERITY=0
case "$rc" in
    0) ;;
    3)
        VERITY=1
        if [ "$UNIT_EXISTED" = "0" ]; then
            # 退出码 10 = 前置条件不满足、这步实际没装上（非失败）；host 侧 deploy-battop.sh 据此记进汇总的"前置条件不满足"栏
            echo "✋ verity 激活：单元没装进 /usr，battop 不会被 systemd 管理。二进制已就位：$DIR/battop"; exit 10
        fi
        # 单元是以前装的（verity 之后才激活）：/usr 动不了，但二进制已换——在跑的服务照下面的规则重启才会用上新版
        #（2026-09-25 审计：旧版这里直接 exit 0，在跑的 battop 一直用旧 inode）
        echo "✋ verity 激活：/usr 里此前装的 $UNIT 没法更新，沿用它" ;;
    *) exit 1 ;;
esac

# 清掉旧 oneshot+timer 模型遗留（老设备上可能还有 battop.timer；它的 enable 链接在 /etc tmpfs，重启本就清）。
# verity 激活时 rootfs 不可写，不去碰（它不会被启用，无碍）
if [ "$VERITY" = "0" ] && [ -f "$CJ_SYSD/battop.timer" ]; then
    systemctl disable --now battop.timer >/dev/null 2>&1 || true
    rm_timer() { rm -f "$CJ_SYSD/battop.timer"; }
    cj_with_rootfs_rw rm_timer || echo "⚠ 删旧 battop.timer 失败（无碍，它不会被启用）"
    systemctl daemon-reload
fi

START=0
case "$MODE" in
    start) START=1 ;;
    no) START=0 ;;
    auto)
        if [ "$UNIT_EXISTED" = "0" ]; then START=1
        elif [ "$WAS_ACTIVE" = "active" ] && { [ "$BIN_CHANGED" = "1" ] || [ "$CJ_UNIT_CHANGED" = "1" ]; }; then START=1
        fi ;;
esac
if [ "$START" = "1" ]; then
    echo "[*] 启动常驻服务（启动即首采建 baseline，之后每 ~10min；不 enable，重启后需手动/网页再开）"
    systemctl restart "$UNIT" || { echo "!! systemctl restart $UNIT 失败"; exit 1; }
else
    echo "[*] 未启动 battop.service（原本就停着；要开：网页「管理→电池刺客」或 systemctl start battop）"
fi
echo "[OK] battop 已装。服务状态: $(systemctl is-active "$UNIT" 2>/dev/null || echo inactive)"
