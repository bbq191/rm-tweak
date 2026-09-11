#!/bin/sh
# ═══════════════════════════════════════════════════════════════════════════
# 书架（shelf）设备端安装器 —— reMarkable Paper Pro Move，root 运行。可独立于中文化套件安装。
#
# 装什么（按服务可插拔，--only 挑选）：
#   · 二进制 → ~/.local/bin/（XDG 用户可执行目录）；lo-alias.sh 同目录（网关 ExecStartPre 用，
#     让 10.11.99.1 常驻可达以便 /upload 注入；脚本源在 enhance/lo-alias/，2026-09-11 起独立副本，见该目录
#     README；不再用 cangjie- 前缀，往后新命名一律不带这个前缀）
#   · XDG 目录：~/.config/shelf  ~/.local/share/shelf  ~/.local/state/shelf
#   · systemd：shelf.target + 各服务单元 → /usr/lib/systemd/system（rootfs，普通重启不丢；OTA 冲掉后重跑本脚本）
#     写 /usr 前实检 dm-verity，激活即跳过（安全红线）；绝不给 xochitl 加依赖。
#
# 用法：./install.sh [--only gateway,book,koreader,font,wallpaper,ink,transcribe,mind,note] [--no-systemd] [--src DIR] [--password PW]
#   笔记线服务（ink…）与书架同一载荷、同一 shelf.target，令牌同规则 <令牌>-serve。
#   --only        只装/更新列出的服务（网关总会装）；缺省全装
#   --password    直接设网关密码（缺省首次默认 shelf、网页登录后强制改；之后可 gateway passwd <新密码>）
#   --no-systemd  只落二进制与目录，不碰 /usr（重启后需手动 systemctl start）
#   --src DIR     载荷目录（含 bin/ systemd/ lo-alias/），缺省=本脚本所在目录
# 注：xovi 持久化（开机自动补 xovi）是**基石/xovi 层**的事，不属 shelf——用整包
#     packaging/install-on-device.sh 装 xovi-reenable.service，或手动 xovi/start。shelf 不碰它。
# 幂等，可反复跑；每次先把现有二进制备份到 /home/root/cangjie-backups/shelf-<时间>/。
# ═══════════════════════════════════════════════════════════════════════════
set -eu

HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="$HERE"
ONLY=""
PASSWORD=""
DO_SYSTEMD=1
for a in "$@"; do
    case "$a" in
        --only=*) ONLY="${a#--only=}" ;;
        --only) ;;                                   # 下一个参数是列表
        --no-systemd) DO_SYSTEMD=0 ;;
        --src=*) SRC="${a#--src=}" ;;
        --src) ;;
        --password=*) PASSWORD="${a#--password=}" ;;
        --password) ;;
        *) if [ -n "${_prev:-}" ]; then
               case "$_prev" in --only) ONLY="$a" ;; --src) SRC="$a" ;; --password) PASSWORD="$a" ;; esac
           else
               echo "!! 未知参数：$a"; exit 2
           fi ;;
    esac
    case "$a" in --only|--src|--password) _prev="$a" ;; *) _prev="" ;; esac
done

HOME_DIR="${HOME:-/home/root}"
BIN_DIR="$HOME_DIR/.local/bin"
XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME_DIR/.config}"
XDG_DATA_HOME="${XDG_DATA_HOME:-$HOME_DIR/.local/share}"
XDG_STATE_HOME="${XDG_STATE_HOME:-$HOME_DIR/.local/state}"
SYSD=/usr/lib/systemd/system
BK="$HOME_DIR/cangjie-backups/shelf-$(date +%Y%m%d-%H%M%S)"

ALL="gateway book koreader font wallpaper ink transcribe mind note"
[ -n "$ONLY" ] && SEL="gateway $(echo "$ONLY" | tr ',' ' ' | sed 's/\bgateway\b//g')" || SEL="$ALL"
svc_of() { case "$1" in gateway) echo gateway ;; *) echo "$1-serve" ;; esac; }

echo "═══ 书架 shelf 安装（$(echo "$SEL" | tr ' ' ',')）═══"
[ "$(id -u)" = "0" ] || { echo "!! 需 root"; exit 1; }
[ -d "$SRC/bin" ] || { echo "!! 载荷缺 $SRC/bin/"; exit 1; }

# ── 1. 备份现有二进制 ──
mkdir -p "$BK" "$BIN_DIR" "$XDG_CONFIG_HOME/shelf" "$XDG_DATA_HOME/shelf" "$XDG_STATE_HOME/shelf"
for s in $SEL; do
    b="$(svc_of "$s")"
    [ -f "$BIN_DIR/$b" ] && cp "$BIN_DIR/$b" "$BK/"
done
echo "-- 备份：$BK"

# ── 2. 二进制 + lo 别名脚本 → ~/.local/bin ──
for s in $SEL; do
    b="$(svc_of "$s")"
    [ -f "$SRC/bin/$b" ] || { echo "!! 载荷缺 bin/$b"; exit 1; }
    cp "$SRC/bin/$b" "$BIN_DIR/$b" && chmod 755 "$BIN_DIR/$b"
done
if [ -f "$SRC/lo-alias/lo-alias.sh" ]; then
    cp "$SRC/lo-alias/lo-alias.sh" "$BIN_DIR/lo-alias.sh" && chmod 755 "$BIN_DIR/lo-alias.sh"
fi
# 装 uninstall.sh 为 shelf-uninstall（网关「管理台」网页卸载调它，单一事实源）
if [ -f "$SRC/uninstall.sh" ]; then
    cp "$SRC/uninstall.sh" "$BIN_DIR/shelf-uninstall" && chmod 755 "$BIN_DIR/shelf-uninstall"
fi
echo "-- 二进制已落 $BIN_DIR"
[ -n "$PASSWORD" ] && "$BIN_DIR/gateway" passwd "$PASSWORD"

# ── 3. systemd（写 /usr rootfs；dm-verity 门）──
if [ "$DO_SYSTEMD" = "0" ]; then
    echo "-- --no-systemd：跳过单元。手动：$BIN_DIR/gateway serve"
elif dmsetup ls --target verity 2>/dev/null | grep -q .; then
    echo "✋ dm-verity 激活 —— 跳过写 /usr（不装开机持久，避免变砖）。"
elif [ ! -d "$SRC/systemd" ]; then
    echo "-- 载荷无 systemd/，跳过"
else
    mount -o remount,rw / || { echo "!! remount rw / 失败"; exit 1; }
    (
        set -e
        cd "$SYSD"
        cp "$SRC/systemd/shelf.target" shelf.target && chmod 644 shelf.target
        mkdir -p multi-user.target.wants shelf.target.wants
        ln -sf ../shelf.target multi-user.target.wants/shelf.target
        for s in $SEL; do
            u="$(svc_of "$s").service"
            cp "$SRC/systemd/$u" "$u" && chmod 644 "$u"
            ln -sf "../$u" "shelf.target.wants/$u"
        done
        sync
    )
    mount -o remount,ro / || true
    systemctl daemon-reload
    for s in $SEL; do systemctl restart "$(svc_of "$s").service" 2>/dev/null || true; done
    systemctl start shelf.target 2>/dev/null || true
    echo "-- 单元已写入 /usr 并启动（shelf.target；单个服务可 systemctl disable --now <svc>）"
fi

# ── 3b. 壁纸：建池 + 写原生 SleepScreenPath（选了 wallpaper 才做）──
# 历史迁移块（旧 misc/wallpaper 工具搬池、bind-mount 整套退役清理、blank776.png）已于 2026-09-06 删除：
# 真机确认零残留（白皮书 §03x/§03ab）；万一从更老的备份恢复，按 §03x 手工清。
case " $SEL " in *" wallpaper "*)
    mkdir -p "$XDG_DATA_HOME/shelf/wallpapers/pool"
    # 原生休眠屏：xochitl.conf SleepScreenPath → current.png（有当前图才写；首次写入需 xovi/start 一次生效）
    if [ -f "$XDG_DATA_HOME/shelf/wallpapers/current.png" ]; then
        "$BIN_DIR/wallpaper-serve" enable || true
    else
        echo "-- 壁纸池还没有当前图：上传并激活首张时自动写 SleepScreenPath"
    fi
    ;;
esac

# ── 3c. 字体菜单 qmd（选了 font 才做；qrr 目录在才装）──
case " $SEL " in *" font "*)
    QRR="$HOME_DIR/xovi/exthome/qt-resource-rebuilder"
    if [ -d "$QRR" ] && [ -d "$SRC/xovi" ]; then
        # 固件按 os-release 的 IMG_VERSION 主次号挑 qmd（3.27 与 3.28 的 FormatFont.qml 结构不同）。
        # ⚠ /etc/version 是 build 号（如 20260612085811）、不含语义版本——真机踩过选错版本；
        #   语义版本在 /usr/lib/os-release（rootfs）与 /etc/os-release 的 IMG_VERSION="3.27.3.0"。
        FWV="$(sed -n 's/^IMG_VERSION="\{0,1\}\([0-9]*\.[0-9]*\).*/\1/p' /usr/lib/os-release /etc/os-release 2>/dev/null | head -n1)"
        [ -n "$FWV" ] || FWV=3.28
        echo "-- 固件 $FWV（IMG_VERSION）"
        Q="font-menu-dynamic.qmd"; [ "$FWV" = "3.27" ] && Q="font-menu-dynamic-3.27.qmd"
        rm -f "$QRR/font-menu-dynamic.qmd" "$QRR/font-menu-dynamic-3.27.qmd"
        cp "$SRC/xovi/$Q" "$QRR/font-menu-dynamic.qmd"
        # ⚠ 重启 xochitl 的正确姿势取决于 xovi 怎么持久化：有 xovi-reenable.service（rootfs oneshot helper）
        #   时 `systemctl restart xochitl` 后它不会自动补 xovi——只有开机才跑；没装它（vellum 裸机，xovi 配置在 /etc
        #   tmpfs）时 restart 直接丢 xovi（KOReader 入口/中文化一起没）。两种情况都用 xovi/start：它写 env + bind-mount +
        #   自己 restart xochitl。2026-09-03 真机踩过。
        if [ -x "$HOME_DIR/xovi/start" ]; then
            echo "-- 字体菜单 qmd（$Q）已放 $QRR/ —— ⚠ 生效需重启 xochitl：请跑 $HOME_DIR/xovi/start（不要裸 systemctl restart xochitl，会丢 xovi）"
        else
            echo "-- 字体菜单 qmd（$Q）已放 $QRR/ —— ⚠ 需 systemctl restart xochitl 生效（本脚本不自动重启）"
        fi
    else
        echo "-- （无 qt-resource-rebuilder 目录或载荷无 xovi/，跳过字体菜单 qmd；字体仍可用 fontconfig 装入）"
    fi
    ;;
esac

# ── 3c2. 原生回收站代理 qmd（选了 book 才做；qrr 目录在才装）：shelf doctor --render 探针 + note-serve
#         生成笔记本的旧版本回收都走这条队列（book-serve /trash/*，Sidebar 注入）──
case " $SEL " in *" book "*)
    QRR="$HOME_DIR/xovi/exthome/qt-resource-rebuilder"
    if [ -d "$QRR" ] && [ -f "$SRC/xovi/shelf-trash-agent.qmd" ]; then
        cp "$SRC/xovi/shelf-trash-agent.qmd" "$QRR/shelf-trash-agent.qmd"
        echo "-- 回收站代理 qmd 已放 $QRR/（3.28 锚点）—— 生效同样需 $HOME_DIR/xovi/start 一次"
    fi
    ;;
esac

# ── 3c3. 原生建文件夹代理 qmd（选了 book 才做；qrr 目录在才装；2026-09-19 复活，见 mkdir.rs 模块
#         文档——网页母版库「加入 xochitl → 文件夹」目标文件夹不存在时，靠这条队列（book-serve
#         /mkdir/*，MainView 注入）真建出来）──
case " $SEL " in *" book "*)
    QRR="$HOME_DIR/xovi/exthome/qt-resource-rebuilder"
    if [ -d "$QRR" ] && [ -f "$SRC/xovi/shelf-mkdir-agent.qmd" ]; then
        cp "$SRC/xovi/shelf-mkdir-agent.qmd" "$QRR/shelf-mkdir-agent.qmd"
        echo "-- 建夹代理 qmd 已放 $QRR/（3.28 锚点）—— 生效同样需 $HOME_DIR/xovi/start 一次"
    fi
    ;;
esac

# ── 3d. xovi 持久化诊断（借鉴踩过的坑，不引用/不安装外层单元——那是 xovi 层的事）──
if [ -x "$HOME_DIR/xovi/start" ] && [ ! -f "$SYSD/xovi-reenable.service" ] && [ ! -f "$SYSD/cangjie-xovi-reenable.service" ]; then
    echo "═══════════════════════════════════════════════════"
    echo "⚠ 本机没装 xovi-reenable.service：裸 systemctl restart xochitl 或重启后 xovi 会丢"
    echo "  （字体菜单 / KOReader 入口 / 中文化一起没）。"
    echo "  · 立即重启 xochitl：跑  $HOME_DIR/xovi/start（别裸 restart）"
    echo "  · 想开机自动恢复 xovi：那是 xovi 层的持久化，用整包 packaging/install-on-device.sh"
    echo "    （它装 xovi-reenable.service）；shelf 单独装不管 xovi 持久化。"
    echo "═══════════════════════════════════════════════════"
fi

# ── 4. 健康检查（轮询，不是固定 sleep 1——网关首次启动要签发私有 CA/自签证书，
#     真机实测在这台设备上 1 秒不够，会把"只是还没起完"误报成"起不来"；最多等 10 秒）──
ALL_OK=0
for _try in 1 2 3 4 5 6 7 8 9 10; do
    sleep 1
    ALL_OK=1
    for s in $SEL; do
        st="$(systemctl is-active "$(svc_of "$s")" 2>/dev/null || echo '?')"
        [ "$st" = "active" ] || ALL_OK=0
    done
    [ "$ALL_OK" = "1" ] && break
done
MUST_CHANGE="$(grep -c '"mustChangePassword": true' "${XDG_CONFIG_HOME:-$HOME/.config}/shelf/gateway.json" 2>/dev/null || true)"
# 设备端 busybox wget 不认 --user/自签证书，HTTPS 探测交给 host 侧 deploy.sh（curl -k）；这里只看 systemd + 注册表。
echo "═══════════════════════════════════════════════════"
for s in $SEL; do
    st="$(systemctl is-active "$(svc_of "$s")" 2>/dev/null || echo '?')"
    printf '  %-16s %s\n' "$(svc_of "$s")" "$st"
done
REG="$(ls /tmp/shelf-0/shelf/services/ 2>/dev/null | sed 's/\.json$//' | tr '\n' ' ')"
echo "  注册表        : ${REG:-（空）}"
if [ "$ALL_OK" = "1" ] && [ -n "$REG" ]; then
    echo "✅ 书架在线：https://<设备IP>/  或 https://shelf.local/（mDNS；安卓不支持 .local）——标准 443 端口，不用带端口号"
    if [ "${MUST_CHANGE:-0}" != "0" ]; then
        echo "   登录：密码 shelf（首次默认），登录后必须改；忘记密码：gateway reset-password"
    else
        echo "   登录：已设置的密码（改：网页右上「改密码」/ shelf passwd / 设备上 gateway passwd <新密码>）"
    fi
    echo "   ⚠ 自签证书：登录页「下载 CA 证书」装进手机/电脑信任库一次即不再提示，否则点「高级 → 继续访问」"
else
    echo "⚠️  有服务未起（journalctl -u gateway 等）。备份在 $BK。"
    [ "$DO_SYSTEMD" = "0" ] || exit 1
fi
echo "═══════════════════════════════════════════════════"
