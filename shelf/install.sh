#!/bin/sh
# ═══════════════════════════════════════════════════════════════════════════
# 书架（shelf）设备端安装器 —— reMarkable Paper Pro Move，root 运行。可独立于中文化套件安装。
#
# 装什么（按服务可插拔，--only 挑选；清单见 manifest.sh，与 uninstall.sh 共用）：
#   · 二进制 → ~/.local/bin/（XDG 用户可执行目录）；lo-alias.sh 同目录（网关 ExecStartPre 用，
#     让 10.11.99.1 常驻可达以便 /upload 注入；脚本源在 enhance/lo-alias/）
#   · 库 manifest.sh + devlib.sh → ~/.local/lib/shelf/（shelf-uninstall 运行时 source）
#   · XDG 目录：~/.config/shelf  ~/.local/share/shelf  ~/.local/state/shelf
#   · systemd：shelf.target + 各服务单元 → /usr/lib/systemd/system（rootfs，普通重启不丢；OTA 冲掉后重跑本脚本）
#     写 /usr 前实检 dm-verity，激活即跳过（安全红线）；绝不给 xochitl 加依赖。
#   · qt-resource-rebuilder qmd：font（字体菜单）/ book（回收站代理、建夹代理、漫画页边距代理、阅读器翻页），qrr 目录在才装
#
# 用法：./install.sh [--only gateway,book,...] [--no-systemd] [--src DIR] [--password PW | --password-file FILE]
#   --only          只装/更新列出的服务（网关总会装）；缺省全装
#   --password      直接设网关密码（缺省首次默认 shelf、网页登录后强制改；之后可 gateway passwd <新密码>）
#   --password-file 从文件读密码（读完即删）——host 侧 deploy.sh 用它，密码不上命令行/不经远端 shell 展开
#   --no-systemd    只落二进制与目录，不碰 /usr（重启后需手动 systemctl start）
#   --src DIR       载荷目录（含 bin/ systemd/ lo-alias/ xovi/ manifest.sh devlib.sh），缺省=本脚本所在目录
# xovi 持久化（开机自动补 xovi）是**基石/xovi 层**的事，不属 shelf——用 packaging/deploy-xovi-persist.sh
# 装 xovi-reenable.service。
#
# 幂等、失败不留半成品（2026-09-20 改）：
#   1. 先校验载荷（所选服务的二进制、单元、shelf.target 全在）——缺任何一个，一字节都不写就退出；
#   2. 旧二进制/单元/qmd 备份进 ~/cangjie-backups/shelf-<时间>/（保留最近 5 份，见 devlib.sh）；
#   3. 二进制/qmd 一律 cp→暂存→rename 原子替换（不在运行中进程的 inode 上原地写）；
#   4. /usr 写入在带 trap 的 rw 窗口里，失败/中断也恢复 ro；
#   5. 只重启"内容有变化或没在跑"的服务，重复跑不打扰正在用的服务。
# ═══════════════════════════════════════════════════════════════════════════
set -eu

usage() {
    cat <<'USAGE_EOF'
用法：install.sh [--only gateway,book,...] [--no-systemd] [--src DIR] [--password PW | --password-file FILE]
  --only          只装/更新列出的服务（网关总会装）；缺省全装
  --no-systemd    只落二进制与目录，不碰 /usr
  --src DIR       载荷目录，缺省 = 本脚本所在目录
  --password / --password-file   设网关密码（file 读完即删，host 侧 deploy.sh 用它）
USAGE_EOF
}

HERE="$(cd "$(dirname "$0")" && pwd)"
SRC="$HERE"
ONLY=""
PASSWORD=""
PASSWORD_FILE=""
DO_SYSTEMD=1
VERITY_SKIPPED=0
_prev=""
for a in "$@"; do
    case "$_prev" in
        --only) ONLY="$a"; _prev=""; continue ;;
        --src) SRC="$a"; _prev=""; continue ;;
        --password) PASSWORD="$a"; _prev=""; continue ;;
        --password-file) PASSWORD_FILE="$a"; _prev=""; continue ;;
    esac
    case "$a" in
        --only=*) ONLY="${a#--only=}" ;;
        --src=*) SRC="${a#--src=}" ;;
        --password=*) PASSWORD="${a#--password=}" ;;
        --password-file=*) PASSWORD_FILE="${a#--password-file=}" ;;
        --only|--src|--password|--password-file) _prev="$a" ;;
        --no-systemd) DO_SYSTEMD=0 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "!! 未知参数：$a（-h 看用法）"; exit 2 ;;
    esac
done
[ -z "$_prev" ] || { echo "!! $_prev 缺参数"; exit 2; }

# 找库：优先载荷目录，其次已装的 ~/.local/lib/shelf
_lib() {
    for d in "$SRC" "$HERE" "${HOME:-/home/root}/.local/lib/shelf"; do
        [ -f "$d/$1" ] && { echo "$d/$1"; return 0; }
    done
    echo "!! 找不到 $1（载荷缺库文件）" >&2; return 1
}
# shellcheck disable=SC1090
. "$(_lib devlib.sh)"
# shellcheck disable=SC1090
. "$(_lib manifest.sh)"

HOME_DIR="$CJ_HOME"
BIN_DIR="$HOME_DIR/.local/bin"
LIB_DIR="$HOME_DIR/.local/lib/$SHELF_LIB_DIRNAME"
XDG_CONFIG_HOME="${XDG_CONFIG_HOME:-$HOME_DIR/.config}"
XDG_DATA_HOME="${XDG_DATA_HOME:-$HOME_DIR/.local/share}"
XDG_STATE_HOME="${XDG_STATE_HOME:-$HOME_DIR/.local/state}"
SYSD="$CJ_SYSD"
QRR="$HOME_DIR/xovi/exthome/qt-resource-rebuilder"

# 选中的服务：--only 给的（去重）+ 网关（总会装）；缺省全装（规则在 manifest.sh 的 shelf_select，与 host 侧 deploy.sh 共用）
SEL="$(shelf_select "$ONLY")" || exit 2
sel_has() { case " $SEL " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# 密码文件：一进来就读并删（后面任何校验失败退出也不会把它留在磁盘上）
if [ -n "$PASSWORD_FILE" ]; then
    [ -f "$PASSWORD_FILE" ] || { echo "!! 密码文件不存在：$PASSWORD_FILE"; exit 1; }
    PASSWORD="$(cat "$PASSWORD_FILE")"
    rm -f "$PASSWORD_FILE"
fi

echo "═══ 书架 shelf 安装（$(echo "$SEL" | tr ' ' ',')）═══"
cj_require_root || exit 1
[ -d "$SRC/bin" ] || { echo "!! 载荷缺 $SRC/bin/"; exit 1; }

# ── 0. 先校验载荷：缺任何东西就在写第一个字节之前退出（不留半成品）──
for s in $SEL; do
    b="$(shelf_svc_of "$s")"
    [ -f "$SRC/bin/$b" ] || { echo "!! 载荷缺 bin/$b —— 未做任何改动"; exit 1; }
done
if [ "$DO_SYSTEMD" = "1" ] && [ -d "$SRC/systemd" ]; then
    [ -f "$SRC/systemd/shelf.target" ] || { echo "!! 载荷缺 systemd/shelf.target —— 未做任何改动"; exit 1; }
    for s in $SEL; do
        u="$(shelf_svc_of "$s").service"
        [ -f "$SRC/systemd/$u" ] || { echo "!! 载荷缺 systemd/$u —— 未做任何改动"; exit 1; }
    done
fi

# ── 1. 备份（进 cangjie-backups/shelf-<时间>/，保留最近 5 份）：只在"将被改动/删除"时才备份、才建目录，
#     重复安装无变化不留空目录、不堆重复的 20MB 备份 ──
mkdir -p "$BIN_DIR" "$XDG_CONFIG_HOME/shelf" "$XDG_DATA_HOME/shelf" "$XDG_STATE_HOME/shelf"
BK=""
bk_keep() {   # FILE：备份到本次备份目录（首次调用才创建）
    [ -f "$1" ] || return 0
    if [ -z "$BK" ]; then cj_backup_dir_new shelf; BK="$CJ_BK"; fi
    cp "$1" "$BK/"
}
bk_keep_if_differs() {   # SRC DST：DST 存在且与 SRC 不同才备份 DST
    [ -f "$2" ] || return 0
    cmp -s "$1" "$2" || bk_keep "$2"
}
for s in $SEL; do
    b="$(shelf_svc_of "$s")"
    bk_keep_if_differs "$SRC/bin/$b" "$BIN_DIR/$b"
done

# ── 2. 二进制 + 辅助脚本 + 库 → ~/.local/bin、~/.local/lib（原子替换）──
CHANGED=""   # 二进制有变化的服务（决定重启谁）
for s in $SEL; do
    b="$(shelf_svc_of "$s")"
    cj_safe_replace "$SRC/bin/$b" "$BIN_DIR/$b" "$BIN_DIR" 755 || { echo "!! 写 $BIN_DIR/$b 失败"; exit 1; }
    [ "$CJ_REPLACED" = "1" ] && CHANGED="$CHANGED $s"
    for h in $(shelf_svc_helpers "$s"); do
        for hs in "$SRC/lo-alias/$h" "$SRC/$h"; do
            [ -f "$hs" ] || continue
            cj_safe_replace "$hs" "$BIN_DIR/$h" "$BIN_DIR" 755 || { echo "!! 写 $BIN_DIR/$h 失败"; exit 1; }
            break
        done
    done
done
# 整包才装 shelf-uninstall 与它 source 的库（--only 装单个服务不动共享件）
if [ -z "$ONLY" ]; then
    for f in $SHELF_LIB_FILES; do
        cj_safe_replace "$(_lib "$f")" "$LIB_DIR/$f" "$LIB_DIR" 644 || { echo "!! 写 $LIB_DIR/$f 失败"; exit 1; }
    done
    if [ -f "$SRC/uninstall.sh" ]; then
        cj_safe_replace "$SRC/uninstall.sh" "$BIN_DIR/$SHELF_UNINSTALL_BIN" "$BIN_DIR" 755 || { echo "!! 写 shelf-uninstall 失败"; exit 1; }
    fi
fi
echo "-- 二进制已落 $BIN_DIR"
if [ -n "$PASSWORD" ]; then "$BIN_DIR/gateway" passwd "$PASSWORD"; fi

# ── 3. systemd（写 /usr rootfs；dm-verity 门；带 trap 的 rw 窗口）──
UNITS_CHANGED=""
write_units() {   # 在 rw 窗口里执行；显式 || return 1，不依赖 set -e
    cp "$SRC/systemd/shelf.target" "$SYSD/.shelf.target.new" && chmod 644 "$SYSD/.shelf.target.new" \
        && mv -f "$SYSD/.shelf.target.new" "$SYSD/shelf.target" || return 1
    mkdir -p "$SYSD/multi-user.target.wants" "$SYSD/shelf.target.wants" || return 1
    ln -sf ../shelf.target "$SYSD/multi-user.target.wants/shelf.target" || return 1
    for s in $SEL; do
        u="$(shelf_svc_of "$s").service"
        cp "$SRC/systemd/$u" "$SYSD/.$u.new" && chmod 644 "$SYSD/.$u.new" && mv -f "$SYSD/.$u.new" "$SYSD/$u" || return 1
        ln -sf "../$u" "$SYSD/shelf.target.wants/$u" || return 1
    done
    # 旧命名遗留（一次性迁移）：旧单元 + 它在 wants 里的链接
    for lu in $SHELF_LEGACY_UNITS; do
        rm -f "$SYSD/$lu" "$SYSD/shelf.target.wants/$lu" "$SYSD/multi-user.target.wants/$lu"
    done
    return 0
}
if [ "$DO_SYSTEMD" = "0" ]; then
    echo "-- --no-systemd：跳过单元。手动：$BIN_DIR/gateway serve"
elif cj_verity_active; then
    echo "✋ dm-verity 激活 —— 跳过写 /usr（不装开机持久，避免变砖）。已有的单元（若之前装过）仍会重启以载入新二进制；"
    echo "   没有单元的服务不会被 systemd 管理，需手动跑：$BIN_DIR/gateway serve（其余服务同理）。"
    VERITY_SKIPPED=1
elif [ ! -d "$SRC/systemd" ]; then
    echo "-- 载荷无 systemd/，跳过"
else
    # 是否需要写：任一单元/target 内容与已装不同，或有旧单元遗留，或缺 wants 链接
    need=0
    cmp -s "$SRC/systemd/shelf.target" "$SYSD/shelf.target" 2>/dev/null || need=1
    [ -L "$SYSD/multi-user.target.wants/shelf.target" ] || need=1
    for s in $SEL; do
        u="$(shelf_svc_of "$s").service"
        cmp -s "$SRC/systemd/$u" "$SYSD/$u" 2>/dev/null || { need=1; UNITS_CHANGED="$UNITS_CHANGED $s"; bk_keep "$SYSD/$u"; }
        [ -L "$SYSD/shelf.target.wants/$u" ] || need=1
    done
    for lu in $SHELF_LEGACY_UNITS; do
        if [ -e "$SYSD/$lu" ]; then
            need=1
            systemctl disable --now "$lu" 2>/dev/null || true
            bk_keep "$SYSD/$lu"
        fi
    done
    if [ "$need" = "1" ]; then
        cj_with_rootfs_rw write_units || { echo "!! 写 /usr 单元失败（rootfs 已恢复 ro；二进制已更新，单元未完成）。备份在 ${BK:-（本次无需备份）}"; exit 1; }
        systemctl daemon-reload
        echo "-- 单元已写入 /usr（shelf.target；单个服务可 systemctl disable --now <svc>）"
    else
        echo "-- /usr 单元已是最新，未 remount"
    fi
    # 旧命名遗留的二进制/脚本（单元已清才删）
    for lb in $SHELF_LEGACY_BINS; do
        if [ -f "$BIN_DIR/$lb" ]; then bk_keep "$BIN_DIR/$lb"; rm -f "$BIN_DIR/$lb"; echo "-- 已清旧命名遗留 $lb"; fi
    done
fi
# 只重启"二进制或单元变了，或当前没在跑"的服务——且该服务的单元文件确实在 /usr 里（verity 跳过写单元时，
# 之前装过的单元照样要重启才能载入新二进制；根本没有单元的服务 systemctl restart 也只会报错，不去碰）
if [ "$DO_SYSTEMD" = "1" ] && [ -d "$SRC/systemd" ]; then
    for s in $SEL; do
        svc="$(shelf_svc_of "$s")"
        [ -f "$SYSD/$svc.service" ] || continue
        st="$(systemctl is-active "$svc" 2>/dev/null || true)"
        if [ "$st" != "active" ] || case " $CHANGED $UNITS_CHANGED " in *" $s "*) true ;; *) false ;; esac; then
            systemctl restart "$svc.service" 2>/dev/null || true
        fi
    done
    if [ -f "$SYSD/shelf.target" ]; then systemctl start shelf.target 2>/dev/null || true; fi
fi

# ── 3b. 壁纸：建池 + 写原生 SleepScreenPath（选了 wallpaper 才做）──
# 历史迁移块已于 2026-09-06 删除：真机确认零残留（白皮书 §03x/§03ab）。
if sel_has wallpaper; then
    mkdir -p "$XDG_DATA_HOME/shelf/wallpapers/pool"
    # 原生休眠屏：xochitl.conf SleepScreenPath → current.png（有当前图才写；首次写入需重启 xochitl 一次生效）
    if [ -f "$XDG_DATA_HOME/shelf/wallpapers/current.png" ]; then
        "$BIN_DIR/wallpaper-serve" enable || true
    else
        echo "-- 壁纸池还没有当前图：上传并激活首张时自动写 SleepScreenPath"
    fi
fi

# ── 3c. qt-resource-rebuilder qmd（qrr 目录在才装；被替换的旧文件备份进 $BK，不在 qrr 目录里留 .bak）──
QMD_CHANGED=0   # qmd 真的被改动（新装/内容变化/清旧遗留）——只有这时才需要重启 xochitl 才生效
if [ -d "$QRR" ] && [ -d "$SRC/xovi" ]; then
    if sel_has font; then
        # 固件按 os-release 的 IMG_VERSION 主次号挑 qmd（3.27 与 3.28 的 FormatFont.qml 结构不同）。
        # ⚠ /etc/version 是 build 号（如 20260612085811）、不含语义版本——真机踩过选错版本；
        #   语义版本在 /usr/lib/os-release（rootfs）与 /etc/os-release 的 IMG_VERSION="3.27.3.0"。
        FWV="$(sed -n 's/^IMG_VERSION="\{0,1\}\([0-9]*\.[0-9]*\).*/\1/p' /usr/lib/os-release /etc/os-release 2>/dev/null | head -n 1)"
        [ -n "$FWV" ] || FWV=3.28
        echo "-- 固件 $FWV（IMG_VERSION）"
        Q="font-menu-dynamic.qmd"; [ "$FWV" = "3.27" ] && Q="font-menu-dynamic-3.27.qmd"
        if [ -f "$SRC/xovi/$Q" ]; then
            bk_keep_if_differs "$SRC/xovi/$Q" "$QRR/font-menu-dynamic.qmd"
            cj_safe_replace "$SRC/xovi/$Q" "$QRR/font-menu-dynamic.qmd" "$CJ_STAGE_DIR" 644 || { echo "!! 写字体菜单 qmd 失败"; exit 1; }
            if [ "$CJ_REPLACED" = "1" ]; then QMD_CHANGED=1; fi
            echo "-- 字体菜单 qmd（$Q）已放 $QRR/"
        else
            echo "-- 载荷无 xovi/$Q，跳过字体菜单 qmd"
        fi
    fi
    if sel_has book; then
        # 原生回收站代理（note-serve 旧版本回收走 book-serve /trash/*，Sidebar 注入）
        # 与原生建文件夹代理（网页母版库「加入 xochitl → 文件夹」不存在时靠 /mkdir/* 真建出来，MainView 注入；2026-09-19 复活）
        for q in $(shelf_svc_qmds book); do
            if [ -f "$SRC/xovi/$q" ]; then
                bk_keep_if_differs "$SRC/xovi/$q" "$QRR/$q"
                cj_safe_replace "$SRC/xovi/$q" "$QRR/$q" "$CJ_STAGE_DIR" 644 || { echo "!! 写 $q 失败"; exit 1; }
                if [ "$CJ_REPLACED" = "1" ]; then QMD_CHANGED=1; fi
                echo "-- qmd $q 已放 $QRR/（3.28 锚点）"
            fi
        done
    fi
    # 旧版本遗留的变体名
    for lq in $SHELF_LEGACY_QMDS; do
        if [ -e "$QRR/$lq" ]; then rm -f "$QRR/$lq"; QMD_CHANGED=1; fi
    done
    cj_stage_cleanup
else
    echo "-- （无 qt-resource-rebuilder 目录或载荷无 xovi/，跳过字体菜单/回收站/建夹/漫画页边距/阅读器翻页 qmd；字体仍可用 fontconfig 装入）"
fi
if [ "$QMD_CHANGED" = "1" ]; then
    cj_pending_mark shelf-qmd || true   # 让 packaging/deploy-xovi-apply.sh 知道有 qmd 待生效
    # ⚠ qmd 只落盘，要 xochitl 重新启动才注入。2026-09-25 起统一靠整机重启生效（packaging/deploy-xovi-apply.sh，
    #   内部 devlib.sh 的 cj_xochitl_apply）：单独 restart xochitl 有概率在它退出时崩溃并触发整机重启（见 devlib.sh 头注 H3）。
    #   ⚠ 绝不在 xovi 已生效时跑 xovi/start：它会让运行中的 xochitl SEGV → 整机自动重启（2026-09-20 真机事故）。
    #   本脚本不自动重启（会打断阅读）。
    echo "-- ⚠ qmd 生效需整机重启（会打断阅读）：电脑上跑 packaging/deploy-xovi-apply.sh <设备>，或在设备上 reboot"
fi

# ── 3d. xovi 持久化诊断（不引用/不安装外层单元——那是 xovi 层的事）──
if [ -x "$HOME_DIR/xovi/start" ] && [ ! -f "$SYSD/xovi-reenable.service" ] && [ ! -f "$SYSD/cangjie-xovi-reenable.service" ]; then
    echo "═══════════════════════════════════════════════════"
    echo "⚠ 本机没装 xovi-reenable.service：真机重启后 xovi 会丢（字体菜单 / KOReader 入口 / 中文化一起没）。"
    echo "  想开机自动恢复 xovi：那是 xovi 层的持久化，跑 packaging/deploy-xovi-persist.sh <host>"
    echo "  （或整包 packaging/install-all.sh）；shelf 单独装不管 xovi 持久化。"
    echo "═══════════════════════════════════════════════════"
fi

# ── 4. 健康检查（轮询，不是固定 sleep 1——网关首次启动要签发私有 CA/自签证书，
#     真机实测在这台设备上 1 秒不够，会把"只是还没起完"误报成"起不来"；最多等 10 秒）──
# --no-systemd 时服务本来就不会被拉起，不空等 10 秒，直接看一次现状
ALL_OK=0
_tries="1 2 3 4 5 6 7 8 9 10"; [ "$DO_SYSTEMD" = "1" ] || _tries="1"
for _try in $_tries; do
    [ "$DO_SYSTEMD" = "1" ] && sleep 1
    ALL_OK=1
    for s in $SEL; do
        st="$(systemctl is-active "$(shelf_svc_of "$s")" 2>/dev/null || echo '?')"
        [ "$st" = "active" ] || ALL_OK=0
    done
    [ "$ALL_OK" = "1" ] && break
done
MUST_CHANGE="$(grep -c '"mustChangePassword": true' "$XDG_CONFIG_HOME/shelf/gateway.json" 2>/dev/null || true)"
# 设备端 busybox wget 不认 --user/自签证书，HTTPS 探测交给 host 侧 deploy.sh（curl -k）；这里只看 systemd + 注册表。
echo "═══════════════════════════════════════════════════"
for s in $SEL; do
    st="$(systemctl is-active "$(shelf_svc_of "$s")" 2>/dev/null || echo '?')"
    printf '  %-16s %s\n' "$(shelf_svc_of "$s")" "$st"
done
REG="$(ls "${SHELF_REG_DIR:-/tmp/shelf-0/shelf/services}/" 2>/dev/null | sed 's/\.json$//' | tr '\n' ' ')"
echo "  注册表        : ${REG:-（空）}"
[ -z "$BK" ] || echo "-- 备份：$BK"
cj_bk_prune shelf- d
if [ "$ALL_OK" = "1" ] && [ -n "$REG" ]; then
    echo "✅ 书架在线：https://<设备IP>/  或 https://shelf.local/（mDNS；安卓不支持 .local）——标准 443 端口，不用带端口号"
    if [ "${MUST_CHANGE:-0}" != "0" ]; then
        echo "   登录：密码 shelf（首次默认），登录后必须改；忘记密码：gateway reset-password"
    else
        echo "   登录：已设置的密码（改：网页右上「改密码」/ 设备上 gateway passwd <新密码>）"
    fi
    echo "   ⚠ 自签证书：登录页「下载 CA 证书」装进手机/电脑信任库一次即不再提示，否则点「高级 → 继续访问」"
else
    if [ "$VERITY_SKIPPED" = "1" ]; then
        echo "⚠️  dm-verity 激活、没有装 /usr 单元，服务没被 systemd 拉起——二进制已就位，手动跑：$BIN_DIR/gateway serve（其余服务同理）。备份在 ${BK:-（本次无需备份）}。"
    else
        echo "⚠️  有服务未起（journalctl -u gateway 等）。备份在 ${BK:-（本次无需备份）}。"
    fi
    [ "$DO_SYSTEMD" = "0" ] || exit 1
fi
echo "═══════════════════════════════════════════════════"
