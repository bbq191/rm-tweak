#!/bin/sh
# 书架卸载（设备端，root）：停服务、删单元/.wants、删 ~/.local/bin 二进制、删随服务装的 qmd 与辅助脚本；
# **保留用户数据**（~/.config/shelf、~/.local/share/shelf、~/.local/state/shelf、用户字体/壁纸）。
# 清单与 install.sh 共用 manifest.sh（2026-09-20：install 装什么这里就删什么，含 shelf-mkdir-agent.qmd、
# lo-alias.sh、shelf-uninstall 本身与 ~/.local/lib/shelf 库，以及旧命名遗留 shelf-gateway 等）。
# 用法：./uninstall.sh [--only font,wallpaper] [--purge] [--dry-run]   --purge 连数据一起删
#   --only    只卸列出的服务（共享件 shelf-uninstall/库/shelf.target 保留；被网页「管理台」这样调用）
#   --purge   只删 shelf 自己的三个 XDG 目录（且要求目录名恰为 shelf、不是符号链接）；笔记线等其它线的数据不碰。
#   --dry-run 只列出"将会删除的、当前存在的"路径，什么都不做（想先核对目标时用）
# 设备上装成 ~/.local/bin/shelf-uninstall（网关网页卸载与 packaging/uninstall-all.sh 都调它）。
#
# 2026-09-22 审计：① /usr 单元删不掉（dm-verity 激活 / rw 窗口失败）时**保留二进制与 shelf-uninstall**——单元与
# wants 开机链接还在，重启后服务会被拉起，删了二进制它们就每 5 秒失败重启一次；保留二进制则重启后服务照常跑，
# 待可写时再跑一次本脚本即可彻底清除；② 没有任何 /usr 单元残留时不再 remount rw（幂等）；③ --dry-run / -h。
set -eu

usage() {
    cat <<'USAGE_EOF'
用法：uninstall.sh [--only font,wallpaper] [--purge] [--dry-run]
  --only    只卸列出的服务（共享件保留）    --purge  连 shelf 的三个 XDG 数据目录一起删
  --dry-run 只列出将会删除的现存路径，不做任何改动
USAGE_EOF
}

HERE="$(cd "$(dirname "$0")" && pwd)"
ONLY=""; PURGE=0; DRY=0; _prev=""
for a in "$@"; do
    case "$_prev" in
        --only) ONLY="$a"; _prev=""; continue ;;
    esac
    case "$a" in
        -h|--help) usage; exit 0 ;;
        --only=*) ONLY="${a#--only=}" ;;
        --only) _prev="$a" ;;
        --purge) PURGE=1 ;;
        --dry-run) DRY=1 ;;
        *) echo "!! 未知参数：$a（-h 看用法）"; exit 2 ;;
    esac
done
[ -z "$_prev" ] || { echo "!! $_prev 缺参数"; exit 2; }

_lib() {
    for d in "$HERE" "${HOME:-/home/root}/.local/lib/shelf"; do
        [ -f "$d/$1" ] && { echo "$d/$1"; return 0; }
    done
    echo "!! 找不到 $1（重装一次 shelf 会装上库；或从载荷目录跑）" >&2; return 1
}
# shellcheck disable=SC1090
. "$(_lib devlib.sh)"
# shellcheck disable=SC1090
. "$(_lib manifest.sh)"

HOME_DIR="$CJ_HOME"
BIN_DIR="$HOME_DIR/.local/bin"
LIB_DIR="$HOME_DIR/.local/lib/$SHELF_LIB_DIRNAME"
SYSD="$CJ_SYSD"
QRR="$HOME_DIR/xovi/exthome/qt-resource-rebuilder"

if [ -n "$ONLY" ]; then
    SEL=""
    for s in $(echo "$ONLY" | tr ',' ' '); do
        case " $SHELF_ALL " in *" $s "*) SEL="$SEL $s" ;; *) echo "!! 未知服务令牌：$s（可选：$SHELF_ALL）"; exit 2 ;; esac
    done
else
    SEL="$SHELF_ALL"
fi
sel_has() { case " $SEL " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# 清 --purge 的数据目录：只删 shelf 自己的三个 XDG 目录（目录名必须恰为 shelf、不能是符号链接、变量不能为空）
purge_dirs() {
    XCH="${XDG_CONFIG_HOME:-$HOME_DIR/.config}"; XDH="${XDG_DATA_HOME:-$HOME_DIR/.local/share}"; XSH="${XDG_STATE_HOME:-$HOME_DIR/.local/state}"
    shelf_data_dirs "$XCH" "$XDH" "$XSH"
}

# 当前存在的 /usr 单元/链接（决定要不要开 rw 窗口）
unit_paths() {
    for s in $SEL; do
        u="$(shelf_svc_of "$s").service"
        echo "$SYSD/$u"; echo "$SYSD/shelf.target.wants/$u"
    done
    if [ -z "$ONLY" ]; then
        echo "$SYSD/shelf.target"; echo "$SYSD/multi-user.target.wants/shelf.target"
        for lu in $SHELF_LEGACY_UNITS; do echo "$SYSD/$lu"; echo "$SYSD/shelf.target.wants/$lu"; done
    fi
}
# 将被删的用户态路径（二进制/辅助/qmd/库…）
file_paths() {
    for s in $SEL; do
        for q in $(shelf_svc_qmds "$s"); do echo "$QRR/$q"; done
        for h in $(shelf_svc_helpers "$s"); do echo "$BIN_DIR/$h"; done
        echo "$BIN_DIR/$(shelf_svc_of "$s")"
    done
    if [ -z "$ONLY" ]; then
        for lb in $SHELF_LEGACY_BINS; do echo "$BIN_DIR/$lb"; done
        for lq in $SHELF_LEGACY_QMDS; do echo "$QRR/$lq"; done
        for f in $SHELF_LIB_FILES; do echo "$LIB_DIR/$f"; done
        echo "$BIN_DIR/$SHELF_UNINSTALL_BIN"
    fi
}
exists_p() { [ -e "$1" ] || [ -L "$1" ]; }

if [ "$DRY" = "1" ]; then
    echo "═══ shelf 卸载 dry-run（不做任何改动）：服务 $(echo "$SEL" | sed 's/^ //')；purge=$PURGE ═══"
    for p in $(unit_paths) $(file_paths); do
        if exists_p "$p"; then echo "  会删：$p"; fi
    done
    if [ "$PURGE" = "1" ]; then
        for d in $(purge_dirs); do
            if [ -d "$d" ]; then echo "  --purge 会删（含用户数据）：$d"; fi
        done
    fi
    exit 0
fi

cj_require_root || exit 1

for s in $SEL; do systemctl disable --now "$(shelf_svc_of "$s").service" 2>/dev/null || true; done
if [ -z "$ONLY" ]; then
    systemctl disable --now shelf.target 2>/dev/null || true
    for lu in $SHELF_LEGACY_UNITS; do systemctl disable --now "$lu" 2>/dev/null || true; done
fi

# ── /usr 单元（dm-verity 门 + 带 trap 的 rw 窗口；没有残留就不 remount）──
rm_units() {
    for p in $(unit_paths); do rm -f "$p"; done
    if [ -z "$ONLY" ]; then rmdir "$SYSD/shelf.target.wants" 2>/dev/null || true; fi
    return 0
}
UNITS_GONE=1
HAVE_UNITS=0
for p in $(unit_paths); do
    if exists_p "$p"; then HAVE_UNITS=1; fi
done
if [ "$HAVE_UNITS" = "0" ]; then
    echo "-- /usr 里没有 shelf 单元残留，未 remount"
elif cj_verity_active; then
    UNITS_GONE=0
    echo "✋ dm-verity 激活，rootfs 不可写——服务已 stop/disable，但 /usr 里的单元文件与开机链接删不掉。"
    echo "   为避免重启后服务因缺二进制而每 5 秒失败重启，本次**保留二进制与 shelf-uninstall**；下次固件 OTA 冲掉后（或解除 verity 后）再跑一次卸载即可清除。"
elif ! cj_with_rootfs_rw rm_units; then
    UNITS_GONE=0
    echo "⚠ 删 /usr 单元失败（rootfs 已恢复 ro）；服务已 stop/disable。为避免重启后服务因缺二进制反复重启，本次保留二进制与 shelf-uninstall；修好后再跑一次。"
else
    systemctl daemon-reload
fi

# ── 随服务装的东西：壁纸还原 / qmd / 辅助脚本 / 二进制 ──
if sel_has wallpaper; then
    # 还原原生休眠屏（删 xochitl.conf SleepScreenPath；xochitl 重启后生效）
    if [ -x "$BIN_DIR/wallpaper-serve" ]; then "$BIN_DIR/wallpaper-serve" disable 2>/dev/null || true; fi
fi
for s in $SEL; do
    for q in $(shelf_svc_qmds "$s"); do rm -f "$QRR/$q"; done
    if [ "$UNITS_GONE" = "1" ]; then
        for h in $(shelf_svc_helpers "$s"); do rm -f "$BIN_DIR/$h"; done
        rm -f "$BIN_DIR/$(shelf_svc_of "$s")"
    fi
done

# ── 整包卸载才做：旧命名遗留、共享件（库、shelf-uninstall 本身，最后删）──
if [ -z "$ONLY" ]; then
    for lq in $SHELF_LEGACY_QMDS; do rm -f "$QRR/$lq"; done
    if [ "$UNITS_GONE" = "1" ]; then
        for lb in $SHELF_LEGACY_BINS; do rm -f "$BIN_DIR/$lb"; done
        for f in $SHELF_LIB_FILES; do
            # 库文件此刻已被 source 进内存，删了不影响后面的执行
            rm -f "$LIB_DIR/$f"
        done
        rmdir "$LIB_DIR" 2>/dev/null || true
        rmdir "$HOME_DIR/.local/lib" 2>/dev/null || true
        rm -f "$BIN_DIR/$SHELF_UNINSTALL_BIN"   # 自己：Linux 上删掉正在跑的脚本文件没问题，放在最后
    fi
fi

if [ "$PURGE" = "1" ]; then
    # 只删 shelf 自己的三个 XDG 目录：目录名必须恰为 shelf、不能是符号链接、变量不能为空
    for d in $(purge_dirs); do
        case "$d" in
            /?*/shelf) ;;
            *) echo "!! 拒绝清除异常路径：$d"; continue ;;
        esac
        [ -L "$d" ] && { echo "!! $d 是符号链接，拒绝清除"; continue; }
        if [ -d "$d" ]; then rm -rf "$d"; fi
    done
    echo "-- 已连同数据清除（~/.config/shelf ~/.local/share/shelf ~/.local/state/shelf；笔记线数据未动）"
fi
if [ "$UNITS_GONE" = "1" ]; then
    echo "✅ 书架已卸载（$(echo "$SEL" | tr ' ' ',' | sed 's/^,//')）；用户字体/壁纸文件未动。"
else
    echo "⚠ 书架服务已停用，但 /usr 单元没删掉、二进制保留（见上）；用户字体/壁纸文件未动。"
fi
[ -z "$ONLY" ] || echo "   （--only：shelf.target、shelf-uninstall 与共享库保留）"
