#!/bin/sh
# shellcheck shell=sh
# shellcheck disable=SC2034  # 本文件只定义变量/函数，由 install.sh / uninstall.sh source 后使用
# ═══════════════════════════════════════════════════════════════════════════
# shelf/manifest.sh —— 书架安装/卸载共用的"清单"（单一事实源，2026-09-20 脚本审计 H4）。
#
# 被 shelf/install.sh 与 shelf/uninstall.sh（设备上装成 ~/.local/bin/shelf-uninstall）同时 source：
# install 装了什么、uninstall 就删什么，两边不会再各写各的清单而漏项（旧版 uninstall 漏删
# shelf-mkdir-agent.qmd、lo-alias.sh、shelf-uninstall，且没有旧命名遗留清理）。
# 新增一个服务/qmd/辅助脚本 = 只改这个文件。
# ═══════════════════════════════════════════════════════════════════════════

# 服务令牌：gateway 的单元/二进制叫 gateway，其余叫 <令牌>-serve（笔记线 ink/transcribe/mind/note 同规则）
SHELF_ALL="gateway book koreader font wallpaper ink transcribe mind note"
shelf_svc_of() { case "$1" in gateway) echo gateway ;; *) echo "$1-serve" ;; esac; }

# shelf_select ONLY：安装时 --only 的值（逗号分隔的服务令牌）→ 要装的服务清单（去重；网关总会装、排最前）；
# 空 = 全部。未知令牌：报错到 stderr、返回 2。host 侧 deploy.sh 与设备端 install.sh 共用（两边算出的清单必须一致）。
shelf_select() {
    [ -n "$1" ] || { echo "$SHELF_ALL"; return 0; }
    ss_sel="gateway"
    for ss_s in $(echo "$1" | tr ',' ' '); do
        case " $SHELF_ALL " in *" $ss_s "*) ;; *) echo "!! 未知服务令牌：$ss_s（可选：$SHELF_ALL）" >&2; return 2 ;; esac
        case " $ss_sel " in *" $ss_s "*) ;; *) ss_sel="$ss_sel $ss_s" ;; esac
    done
    echo "$ss_sel"
}

# 服务→随它装的 qt-resource-rebuilder qmd（放 $HOME/xovi/exthome/qt-resource-rebuilder/）
#   font 的 qmd 载荷里按固件版本二选一（font-menu-dynamic.qmd / -3.27.qmd），设备上统一叫 font-menu-dynamic.qmd
shelf_svc_qmds() {
    case "$1" in
        font) echo "font-menu-dynamic.qmd" ;;
        book) echo "shelf-trash-agent.qmd shelf-mkdir-agent.qmd shelf-comic-margins.qmd reader-page-turn.qmd" ;;
        *) echo "" ;;
    esac
}

# 服务→随它装的 ~/.local/bin 辅助脚本（网关 ExecStartPre 用 lo-alias.sh 让 10.11.99.1 常驻可达）
shelf_svc_helpers() {
    case "$1" in
        gateway) echo "lo-alias.sh" ;;
        *) echo "" ;;
    esac
}

# 整包（非 --only）才装/删的共享件：shelf-uninstall（网关「管理台」网页卸载调它）与它 source 的库
SHELF_LIB_DIRNAME="shelf"                      # ~/.local/lib/shelf/{manifest.sh,devlib.sh}
SHELF_LIB_FILES="manifest.sh devlib.sh"
SHELF_UNINSTALL_BIN="shelf-uninstall"

# 旧命名遗留（一次性迁移：install 时清掉、uninstall 也清）。2026-09-10/11 改名前的产物：
#   shelf-gateway.service / shelf-gateway（网关旧名）、cangjie-lo-alias.sh（lo-alias.sh 旧名，2026-09-11 起不再带 cangjie- 前缀）
SHELF_LEGACY_UNITS="shelf-gateway.service"
SHELF_LEGACY_BINS="shelf-gateway cangjie-lo-alias.sh"
SHELF_LEGACY_QMDS="font-menu-dynamic-3.27.qmd"   # 早期版本可能把 3.27 变体也按原名放进 qrr

# 用户数据目录（--purge 才删；绝不含笔记线 ~/.local/state/notes 等其它线的数据）
shelf_data_dirs() {
    # $1=config-home $2=data-home $3=state-home
    echo "$1/shelf $2/shelf $3/shelf"
}
