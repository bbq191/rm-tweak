#!/bin/sh
# hl-snap 安装脚本（vellum-xovi 结构，只碰 /home，不碰 /usr）——独立于
# chinese-ime/langhook 之外的最小 xovi 扩展，只做荧光笔精确吸附一件事。
#
# 装的是什么：`extensions.d/` 下的一个 xovi 扩展（hl-snap.so），跟
# qt-resource-rebuilder/appload/cangjie-langhook 并列，由 xovi 自己扫描加载。
# 不装拼音输入法、不装词典、不改 UI 语言——只有这一个 hook。
#
# 前置（本脚本不装，跟 chinese-ime/langhook 共用同一份基石）：
#   vellum add xovi
#
# 【重启后持久化】/etc tmpfs 重启即清、无开机自动服务 → 重启后需手动恢复：
#   /home/root/xovi/start        （或 vellum reenable）
#
# 用法：./install.sh [--no-restart]
#   --no-restart  只落盘 hl-snap.so，不跑 xovi/start——xovi/start 是全量重启 xochitl 重注入
#   **全部**扩展（没有"只重载单个扩展"的机制，见系统增强线白皮书），多个扩展各自跑一次等于
#   短时间内重启 xochitl 多次，xochitl 有 watchdog+StartLimit，真机验证过这样容易撞
#   StartLimitAction 触发整机重启（2026-09-11 packaging/install-all.sh 连续装 hl-snap+
#   handwriting-stroke 真机踩过）。外部编排方（如 packaging/install-all.sh）用这个选项让多个
#   xovi 扩展只落盘、最后统一跑一次 xovi/start；脱离编排单独跑本脚本不传这个参数，行为不变。
set -eu

NO_RESTART=0
for a in "$@"; do
    case "$a" in
        --no-restart) NO_RESTART=1 ;;
        *) echo "!! 未知参数：$a"; exit 2 ;;
    esac
done

HERE="$(cd "$(dirname "$0")" && pwd)"
PAYLOAD="$(dirname "$HERE")"   # deploy/ 的上一级，hl-snap.so 编译产物在这
ROOT=/home/root
XOVI="$ROOT/xovi"
EXTDIR="$XOVI/extensions.d"
DATADIR="$ROOT/.local/share/cangjie-ime"   # 复用同一个 reading-qol.json（hlSnapCjk 键）

echo "== hl-snap 安装（独立最小扩展，只做荧光笔精确吸附）=="

[ "$(id -u)" = "0" ] || { echo "!! 需要 root 运行"; exit 1; }
[ -f "$XOVI/xovi.so" ] || { echo "!! 没找到 $XOVI/xovi.so —— 先跑：vellum add xovi"; exit 1; }
[ -f "$PAYLOAD/hl-snap.so" ] || { echo "!! 没找到 $PAYLOAD/hl-snap.so，先在这边跑 make aarch64"; exit 1; }

BACKUP_DIR="$ROOT/cangjie-backups"
if [ -f "$EXTDIR/hl-snap.so" ]; then
    # 备份绝不能留在 $EXTDIR（extensions.d/）里——xovi 把这个目录下任意文件都当扩展加载
    # （不看后缀），备份文件会被当成另一个扩展重复注册，是致命错误。落进专门的
    # cangjie-backups/ 目录，2026-09-15 全量代码审查补（deploy-sidebar-
    # entry.sh 已经这么做，这几个 xovi 扩展的安装脚本当时漏了）。
    mkdir -p "$BACKUP_DIR"
    cp "$EXTDIR/hl-snap.so" "$BACKUP_DIR/hl-snap.so.bak.pre-$(date +%Y%m%d-%H%M%S)"
    echo "-- 已备份旧版本 -> $BACKUP_DIR/"
fi

echo "-- 拷 hl-snap.so -> $EXTDIR/"
mkdir -p "$EXTDIR"
cp "$PAYLOAD/hl-snap.so" "$EXTDIR/hl-snap.so"
rm -f "$EXTDIR/hl-snap.so.crashed"   # 清旧崩溃标记（xovi 把 extensions.d 里任意文件当扩展加载，重复注册是致命错）

# reading-qol.json 首次装才建（不覆盖已有设置）；只关心 hlSnapCjk 这一个键，
# 其它键留给别的功能（笔记增强等）各自维护，这里不动。
RQOL="$DATADIR/reading-qol.json"
if [ ! -s "$RQOL" ]; then
    echo "-- 建最小配置（hlSnapCjk 默认开）-> $RQOL"
    mkdir -p "$DATADIR"
    printf '%s' '{"hlSnapCjk":true}' > "$RQOL"
fi

if [ "$NO_RESTART" = "1" ]; then
    echo "-- --no-restart：hl-snap.so 已落盘，未跑 xovi/start（由外部编排方稍后统一执行一次）"
    echo "✅ 已就位，尚未生效——外部编排方跑完这轮 xovi/start 后再确认"
    exit 0
fi

echo "-- 应用 xovi/start（/etc tmpfs 引导，不碰 /usr）"
OLD_PID="$(systemctl show xochitl -p MainPID --value 2>/dev/null || echo 0)"
"$XOVI/start"
sleep 5

STATE="$(systemctl is-active xochitl 2>/dev/null || true)"
NEW_PID="$(systemctl show xochitl -p MainPID --value 2>/dev/null || echo 0)"
NREST="$(systemctl show xochitl -p NRestarts --value 2>/dev/null || echo '?')"
HL="$(grep -c hl-snap /proc/"$NEW_PID"/maps 2>/dev/null || echo 0)"
echo "=================================================="
echo "  is-active : $STATE   (期望 active)"
echo "  MainPID   : $OLD_PID -> $NEW_PID   (期望有变化)"
echo "  NRestarts : $NREST   (期望 0/不增)"
echo "  hl-snap 加载 : $HL 段   (期望 >0)"
echo "=================================================="
if [ "$STATE" = "active" ] && [ "$NEW_PID" != "0" ] && [ "${HL:-0}" -gt 0 ]; then
    echo "✅ 安装完成。划中文即精确吸附（划哪吸哪）。"
    echo "⚠️  重启后需手动恢复：/home/root/xovi/start（或 vellum reenable）"
else
    echo "⚠️  健康检查未达预期。查 journalctl -u xochitl | grep hl-snap"
    exit 1
fi
