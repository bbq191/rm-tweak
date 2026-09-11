#!/bin/sh
# 书架卸载（设备端，root）：停服务、删单元/.wants、删 ~/.local/bin 二进制；**保留用户数据**
# （~/.config/shelf、~/.local/share/shelf、~/.local/state/shelf、用户字体/壁纸）。
# 用法：./uninstall.sh [--only font,wallpaper] [--purge]   --purge 连数据一起删
set -eu
HOME_DIR="${HOME:-/home/root}"
BIN_DIR="$HOME_DIR/.local/bin"
SYSD=/usr/lib/systemd/system
ONLY=""; PURGE=0; _prev=""
for a in "$@"; do
    case "$a" in
        --only=*) ONLY="${a#--only=}" ;;
        --only) _prev=only; continue ;;
        --purge) PURGE=1 ;;
        *) [ "$_prev" = only ] && ONLY="$a" || { echo "!! 未知参数：$a"; exit 2; } ;;
    esac
    _prev=""
done
ALL="gateway book koreader font wallpaper ink transcribe mind note"   # 笔记线四服务同载荷同卸载；其条目库（~/.local/state/notes）不在 --purge 范围，绝不删用户笔记
[ -n "$ONLY" ] && SEL="$(echo "$ONLY" | tr ',' ' ')" || SEL="$ALL"
svc_of() { case "$1" in gateway) echo gateway ;; *) echo "$1-serve" ;; esac; }

for s in $SEL; do systemctl disable --now "$(svc_of "$s").service" 2>/dev/null || true; done
[ -z "$ONLY" ] && systemctl disable --now shelf.target 2>/dev/null || true

if ! dmsetup ls --target verity 2>/dev/null | grep -q .; then
    mount -o remount,rw / || true
    for s in $SEL; do
        u="$(svc_of "$s").service"
        rm -f "$SYSD/$u" "$SYSD/shelf.target.wants/$u"
    done
    if [ -z "$ONLY" ]; then
        rm -f "$SYSD/shelf.target" "$SYSD/multi-user.target.wants/shelf.target"
        rmdir "$SYSD/shelf.target.wants" 2>/dev/null || true
    fi
    sync; mount -o remount,ro / || true
    systemctl daemon-reload
fi
case " $SEL " in *" wallpaper "*)
    # 还原原生休眠屏（删 xochitl.conf SleepScreenPath；xochitl 重启后生效）。旧 bind-mount 残留清理块已于 2026-09-06 删（真机零残留，白皮书 §03ab）
    [ -x "$BIN_DIR/wallpaper-serve" ] && "$BIN_DIR/wallpaper-serve" disable 2>/dev/null || true ;;
esac
case " $SEL " in *" font "*) rm -f "$HOME_DIR/xovi/exthome/qt-resource-rebuilder/font-menu-dynamic.qmd" ;; esac
case " $SEL " in *" book "*) rm -f "$HOME_DIR/xovi/exthome/qt-resource-rebuilder/shelf-trash-agent.qmd" ;; esac
for s in $SEL; do rm -f "$BIN_DIR/$(svc_of "$s")"; done
if [ "$PURGE" = "1" ]; then
    rm -rf "${XDG_CONFIG_HOME:-$HOME_DIR/.config}/shelf" "${XDG_DATA_HOME:-$HOME_DIR/.local/share}/shelf" "${XDG_STATE_HOME:-$HOME_DIR/.local/state}/shelf"
    echo "-- 已连同数据清除"
fi
echo "✅ 书架已卸载（$(echo "$SEL" | tr ' ' ',')）；用户字体/壁纸文件未动。"
