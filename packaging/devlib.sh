#!/bin/sh
# shellcheck shell=sh
# ═══════════════════════════════════════════════════════════════════════════
# devlib.sh —— 设备侧共享函数库（POSIX sh，兼容 busybox：无 curl/pkill/getconf，head 不认 -N）。
#
# 只被 source，不直接执行。三种进入方式（都保证库与被跑脚本同源同版）：
#   · host 侧 packaging/lib.sh 的 dev_script：把本文件拼在 heredoc 脚本前面，经 `ssh sh -s` 送上设备；
#   · 各 deploy 脚本把它随载荷推到设备（shelf 载荷 shelf/devlib.sh、xovi 扩展 deploy/devlib.sh、
#     battop 目录 devlib.sh），设备端 install.sh 再 `. "$HERE/devlib.sh"`；
#   · 设备上残留的 ~/.local/lib/shelf/devlib.sh（shelf-uninstall 用）。
#
# 集中解决的、原先在各脚本里各写一遍且各有缺陷的几件事（2026-09-20 脚本审计）：
#   H1 xovi 已生效的 xochitl 上再跑 xovi/start 会 SEGV→整机自动重启（2026-09-20 真机事故）
#      → cj_xochitl_apply：先判 LD_PRELOAD，已生效（或装了 xovi-reenable）就整机重启（2026-09-25 起，见 H3），
#        没生效才用 xovi/start，且之前先打印"会打断阅读"并留一段宽限。
#   H2 remount rw 之后脚本中途失败会把 rootfs 留在 rw → cj_with_rootfs_rw：失败/信号/正常都恢复 ro。
#   M4 原地 cp 覆盖 xochitl 已映射的 .so / 运行中的二进制 → cj_safe_replace：先写暂存再 rename。
#   M6 备份散落 + 无限增长 → cj_backup_file / cj_bk_prune：统一进 cangjie-backups，
#      只按"脚本自己生成的、严格时间戳命名"轮转，保留最近 N 份，绝不 rm -rf。
#   H3 换了运行中 xochitl 已映射的扩展 .so 再 restart xochitl → 旧进程退出时 SEGV → OnFailure=emergency →
#      整机重启（2026-09-21 appload、2026-09-24 hw-stroke 两次真机；先写暂存再 rename 换新 inode 也照样复现）
#      → cj_so_stage / cj_so_commit：xochitl 正映射着目标 .so 时不当场换，先放进待换入区，由 cj_xochitl_apply
#        在整机重启前换入（09-24～25 曾是 stop → 换入 → start）。
#      2026-09-25 修正：崩的是**停止 xochitl 本身**，跟换没换 .so 无关（同日一次只换了 qmd 的普通 restart 也崩了，
#        memfault 栈与 09-21 那次同一处）→ cj_xochitl_apply 改为一律"换入待换入区 → 主动整机重启"。
#   A1 "内容没变也重启 xochitl"（重跑 install-all 每次都闪屏）→ cj_pending_mark / cj_pending_list / cj_pending_clear：
#      各"只落盘"的步骤在**真的改了文件**时记一个待生效标记（/run tmpfs，重启设备即清——重启后一切都是新载入的），
#      xovi-apply 只在有标记（或待换入区有 .so）、或 xovi 还没在 xochitl 里生效时才让它生效（整机重启 / xovi/start）。
#
# 环境变量（测试与特殊部署可覆盖；设备上一般不用设）：
#   CJ_HOME CJ_SYSD CJ_XOVI CJ_PROC CJ_BACKUP_DIR CJ_BACKUP_KEEP CJ_BACKUP_MAXBYTES CJ_STAGE_DIR
#   CJ_APPLY_GRACE（整机重启 / xovi/start 前的宽限秒数，默认 5）  CJ_HEALTH_SLEEP（xovi/start 后等多久再查，默认 5）
#   CJ_PENDING_DIR（待生效标记目录，默认 /run/cangjie-pending-apply）
#   CJ_SO_PENDING_DIR（待换入的扩展 .so，默认 $CJ_STAGE_DIR/so-pending；与 extensions.d 同分区、绝不在其中）
# ═══════════════════════════════════════════════════════════════════════════

CJ_HOME="${CJ_HOME:-${HOME:-/home/root}}"
CJ_SYSD="${CJ_SYSD:-/usr/lib/systemd/system}"
CJ_XOVI="${CJ_XOVI:-$CJ_HOME/xovi}"
CJ_PROC="${CJ_PROC:-/proc}"
CJ_BACKUP_DIR="${CJ_BACKUP_DIR:-$CJ_HOME/cangjie-backups}"
CJ_BACKUP_KEEP="${CJ_BACKUP_KEEP:-5}"
CJ_BACKUP_MAXBYTES="${CJ_BACKUP_MAXBYTES:-67108864}"   # 单文件超过 64MB 的备份一律不自动删（可能是手工大备份）
CJ_STAGE_DIR="${CJ_STAGE_DIR:-$CJ_HOME/.cangjie-stage}"  # 暂存目录：与 /home/root 同分区（rename 原子），且绝不是 extensions.d

CJ_PENDING_DIR="${CJ_PENDING_DIR:-/run/cangjie-pending-apply}"
CJ_PENDING_FALLBACK="${CJ_PENDING_FALLBACK:-$CJ_STAGE_DIR/pending-apply}"   # /run 写不了时的退路：宁可多重启也不能漏
CJ_SO_PENDING_DIR="${CJ_SO_PENDING_DIR:-$CJ_STAGE_DIR/so-pending}"

cj_require_root() {
    [ "$(id -u)" = "0" ] || { echo "!! 需要 root 运行（ssh root@设备；本脚本没有 sudo 通道）"; return 1; }
}

# ── 待生效标记（A1）────────────────────────────────────────────────────────
# cj_pending_mark NAME：记"NAME 落盘了新内容，需要重启 xochitl 才生效"。失败时换退路目录，都失败才报警告（返回 1）。
cj_pending_mark() {
    for cj_pd in "$CJ_PENDING_DIR" "$CJ_PENDING_FALLBACK"; do
        if mkdir -p "$cj_pd" 2>/dev/null && : > "$cj_pd/$1" 2>/dev/null; then return 0; fi
    done
    echo "⚠ 写不了待生效标记（$CJ_PENDING_DIR）——xovi-apply 可能误判\"无需重启\"；请手动整机重启（reboot）让改动生效"
    return 1
}
# cj_pending_list：列出待生效的标记名（一行一个；没有则无输出）。待换入区里的 .so 也算（记成 so-pending:<文件名>）：
# 标记在 /run、设备重启即清，而待换入区在 /home 不清——只看标记的话，"重启过设备、.so 还没换入"时 xovi-apply
# 会误判"无需重启"，新版永远换不进去（2026-09-24 审计发现）。
cj_pending_list() {
    for cj_pd in "$CJ_PENDING_DIR" "$CJ_PENDING_FALLBACK"; do
        [ -d "$cj_pd" ] || continue
        for cj_pf in "$cj_pd"/*; do
            [ -f "$cj_pf" ] && basename "$cj_pf"
        done
    done
    for cj_pf in $(cj_so_pending_list); do echo "so-pending:$cj_pf"; done
    return 0
}
# cj_apply_needed：要不要重启 xochitl 才能让落盘内容生效——有待生效标记/待换入 .so，或 xovi 还没在 xochitl 里生效。
# deploy-xovi-apply 与各"单独跑"的落盘步骤共用这一个判据（没东西要生效就不重启，重复跑不闪屏）。
cj_apply_needed() {
    [ -n "$(cj_pending_list)" ] || ! cj_xochitl_has_xovi
}
# cj_pending_clear：xochitl 重启成功后清空标记（只删目录里的常规文件，再 rmdir）
cj_pending_clear() {
    for cj_pd in "$CJ_PENDING_DIR" "$CJ_PENDING_FALLBACK"; do
        [ -d "$cj_pd" ] || continue
        for cj_pf in "$cj_pd"/*; do
            [ -f "$cj_pf" ] && rm -f "$cj_pf"
        done
        rmdir "$cj_pd" 2>/dev/null || true
    done
    return 0
}

cj_verity_active() {
    dmsetup ls --target verity 2>/dev/null | grep -q .
}

# ── rootfs 读写窗口（H2）──────────────────────────────────────────────────
CJ_RW_ACTIVE=0

cj_rootfs_restore() {
    [ "$CJ_RW_ACTIVE" = "1" ] || return 0
    cj_i=0
    while ! mount -o remount,ro / 2>/dev/null; do
        cj_i=$((cj_i + 1))
        if [ "$cj_i" -ge 5 ]; then
            echo "⚠ remount ro / 一直 busy，rootfs 暂留 rw（重启恢复 ro）"
            break
        fi
        sleep "${CJ_RETRY_SLEEP:-2}"
    done
    CJ_RW_ACTIVE=0
}

# cj_with_rootfs_rw CMD [ARGS…]：remount rw → 在子 shell 里跑 CMD（子 shell 内 set -e）→ 无论成败/被信号打断
# 都恢复 ro。返回 CMD 的退出码。
# 约束：① 会清掉调用方的 EXIT/INT/TERM/HUP trap（设备端脚本本来不设）；② 不要把它放进 `if`/`||` 上下文里
# 指望 set -e 生效——CMD 里请用显式 `|| return 1` 链；③ 写 /usr 之前先自行过 cj_verity_active 门。
cj_with_rootfs_rw() {
    mount -o remount,rw / || { echo "!! remount rw / 失败"; return 1; }
    CJ_RW_ACTIVE=1
    trap 'cj_rootfs_restore' EXIT
    # PIPE 也要接住：经 ssh 跑时连接断了，下一次输出就是 SIGPIPE，默认处置直接杀 shell、EXIT trap 不会执行
    trap 'cj_rootfs_restore; exit 143' INT TERM HUP PIPE
    ( set -e; "$@" )
    cj_rc=$?
    sync
    cj_rootfs_restore
    trap - EXIT INT TERM HUP PIPE
    return "$cj_rc"
}

# ── 原子替换（M4）─────────────────────────────────────────────────────────
# cj_safe_replace SRC DST [STAGE_DIR] [MODE]：cp 到暂存文件 → chmod → rename 覆盖 DST。
# STAGE_DIR 缺省 DST 同目录；DST 在 extensions.d 一类"目录下任何文件都会被当扩展加载"的地方时，
# 必须传一个 extensions.d 之外、同分区的暂存目录（如 $CJ_STAGE_DIR），否则中途崩溃会在里面留半成品。
# 成功后 CJ_REPLACED=1（内容有变化）或 0（与现有逐字节相同，未动）。
# shellcheck disable=SC2034  # 供调用方读取
CJ_REPLACED=0
cj_safe_replace() {
    cj_src=$1; cj_dst=$2; cj_stage=${3:-$(dirname "$cj_dst")}; cj_mode=${4:-755}
    CJ_REPLACED=0
    [ -f "$cj_src" ] || { echo "!! cj_safe_replace：源不存在 $cj_src"; return 1; }
    if [ -f "$cj_dst" ] && cmp -s "$cj_src" "$cj_dst"; then
        chmod "$cj_mode" "$cj_dst" 2>/dev/null || true
        return 0
    fi
    mkdir -p "$cj_stage" "$(dirname "$cj_dst")" || return 1
    cj_tmp="$cj_stage/.$(basename "$cj_dst").new.$$"
    if cp "$cj_src" "$cj_tmp" && chmod "$cj_mode" "$cj_tmp" && mv -f "$cj_tmp" "$cj_dst"; then
        # shellcheck disable=SC2034
        CJ_REPLACED=1
        return 0
    fi
    rm -f "$cj_tmp"
    return 1
}

# ── 备份与轮转（M6）───────────────────────────────────────────────────────
# 备份统一进 $CJ_BACKUP_DIR（绝不留在 extensions.d / qrr 目录内）。命名：
#   单文件  <basename>.bak.pre-YYYYmmdd-HHMMSS
#   目录    <prefix>-YYYYmmdd-HHMMSS/（如 shelf-20260920-101500/，里面只放常规文件）
# 轮转只认上述严格命名、只删常规文件（目录型只在里面全是常规文件时才 rm -f 逐个删再 rmdir），
# 单文件超过 CJ_BACKUP_MAXBYTES 的不动；手工命名的备份、任何用户数据、大文件都不会被碰。全程无 rm -rf。

# cj_bk_list PREFIX：列出 $CJ_BACKUP_DIR 下形如 "PREFIX<8位日期>-<6位时间>" 的名字（时间序）
cj_bk_list() {
    for cj_n in $(ls -1 "$CJ_BACKUP_DIR" 2>/dev/null | sort); do
        case "$cj_n" in
            "$1"[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]-[0-9][0-9][0-9][0-9][0-9][0-9]) echo "$cj_n" ;;
        esac
    done
}

# cj_bk_prune PREFIX KIND（f=单文件备份 d=目录备份）：只保留最近 CJ_BACKUP_KEEP 份
cj_bk_prune() {
    cj_list="$(cj_bk_list "$1")"
    cj_count=$(printf '%s\n' "$cj_list" | grep -c . || true)
    cj_del=$((cj_count - CJ_BACKUP_KEEP))
    [ "$cj_del" -gt 0 ] || return 0
    printf '%s\n' "$cj_list" | sed -n "1,${cj_del}p" | while read -r cj_n; do
        cj_p="$CJ_BACKUP_DIR/$cj_n"
        [ -L "$cj_p" ] && continue
        if [ "$2" = "f" ]; then
            [ -f "$cj_p" ] || continue
            cj_sz=$(wc -c < "$cj_p" | tr -d ' ')
            [ "$cj_sz" -le "$CJ_BACKUP_MAXBYTES" ] || continue
            rm -f "$cj_p"
        else
            [ -d "$cj_p" ] || continue
            cj_ok=1
            for cj_e in "$cj_p"/* "$cj_p"/.[!.]*; do
                { [ -e "$cj_e" ] || [ -L "$cj_e" ]; } || continue
                if [ -f "$cj_e" ] && [ ! -L "$cj_e" ]; then
                    cj_sz=$(wc -c < "$cj_e" | tr -d ' ')
                    [ "$cj_sz" -le "$CJ_BACKUP_MAXBYTES" ] || cj_ok=0
                else
                    cj_ok=0
                fi
            done
            [ "$cj_ok" = "1" ] || continue
            for cj_e in "$cj_p"/* "$cj_p"/.[!.]*; do
                [ -f "$cj_e" ] && rm -f "$cj_e"
            done
            rmdir "$cj_p" 2>/dev/null || true
        fi
    done
    return 0
}

# cj_backup_file FILE：备份到 cangjie-backups/<basename>.bak.pre-<ts> 并轮转；文件不存在则无事可做
cj_backup_file() {
    [ -f "$1" ] || return 0
    mkdir -p "$CJ_BACKUP_DIR" || return 1
    cj_b=$(basename "$1")
    cj_dst="$CJ_BACKUP_DIR/$cj_b.bak.pre-$(date +%Y%m%d-%H%M%S)"
    cp "$1" "$cj_dst" || return 1
    echo "-- 已备份 $1 -> $cj_dst"
    cj_bk_prune "$cj_b.bak.pre-" f
}

# cj_backup_if_differs SRC DST：DST 存在且内容与 SRC 不同才备份 DST（内容没变就不堆重复备份——
# 否则重复部署 5 次就会把真正有价值的旧版本从"保留最近 5 份"里挤掉）
cj_backup_if_differs() {
    [ -f "$2" ] || return 0
    cmp -s "$1" "$2" && return 0
    cj_backup_file "$2"
}

# cj_stage_cleanup：暂存目录空了就删（有内容——别的步骤的暂存/待生效退路标记——就留着）
cj_stage_cleanup() {
    [ -d "$CJ_STAGE_DIR" ] || return 0
    rmdir "$CJ_STAGE_DIR" 2>/dev/null || true
    return 0
}

# cj_rm_payload DIR FILE…：卸载时清"推送载荷目录"（deploy-* 推上来的 .so/脚本/单元源）：
# 只 rm -f 列出的已知文件再 rmdir（目录里有别的东西就留着，不会误删）；DIR 必须在 $CJ_HOME 下、不是符号链接。
cj_rm_payload() {
    cj_pd=$1; shift
    case "$cj_pd" in "$CJ_HOME"/?*) ;; *) echo "!! 拒绝清理 $CJ_HOME 之外的路径：$cj_pd"; return 1 ;; esac
    [ -L "$cj_pd" ] && { echo "!! $cj_pd 是符号链接，拒绝清理"; return 1; }
    [ -d "$cj_pd" ] || return 0
    for cj_pn in "$@"; do
        [ -L "$cj_pd/$cj_pn" ] && continue
        [ -f "$cj_pd/$cj_pn" ] && rm -f "$cj_pd/$cj_pn"
        [ -d "$cj_pd/$cj_pn" ] && rmdir "$cj_pd/$cj_pn" 2>/dev/null
    done
    rmdir "$cj_pd" 2>/dev/null || true
    return 0
}

# cj_backup_dir_new PREFIX：新建目录备份 $CJ_BACKUP_DIR/PREFIX-<ts>，路径写入 CJ_BK（不做轮转，装完调 cj_bk_prune）
cj_backup_dir_new() {
    CJ_BK="$CJ_BACKUP_DIR/$1-$(date +%Y%m%d-%H%M%S)"
    mkdir -p "$CJ_BK"
}

# ── /usr 下的 systemd 单元（dm-verity 门 + rw 窗口 + 幂等）──────────────────
cj_usr_put_body() { # SRC UNIT WANTS
    cp "$1" "$CJ_SYSD/.$2.new" || return 1
    chmod 644 "$CJ_SYSD/.$2.new" || return 1
    mv -f "$CJ_SYSD/.$2.new" "$CJ_SYSD/$2" || return 1
    if [ -n "$3" ] && [ "$3" != "-" ]; then
        mkdir -p "$CJ_SYSD/$3" || return 1
        ln -sf "../$2" "$CJ_SYSD/$3/$2" || return 1
    fi
    return 0
}

# cj_install_usr_unit UNIT SRC [WANTS_DIR]：WANTS_DIR 缺省 multi-user.target.wants，"-" 表示不建 wants 链接。
# 返回 0=已装/已是最新，1=失败（写入前的校验失败则未动任何东西），3=dm-verity 激活、跳过（调用方按"非失败"处理）
# shellcheck disable=SC2034  # 供调用方读取：本次是否真的写了 /usr（决定要不要重启对应服务）
CJ_UNIT_CHANGED=0
cj_install_usr_unit() {
    cj_u=$1; cj_s=$2; cj_w=${3-multi-user.target.wants}
    CJ_UNIT_CHANGED=0
    [ -f "$cj_s" ] || { echo "!! 缺单元源 $cj_s"; return 1; }
    if cj_verity_active; then
        echo "✋ dm-verity 激活 —— 跳过写 /usr（写 /usr + 重启 → root hash 变 → A/B 回滚变砖，2026-08-16 真机踩过）。"
        return 3
    fi
    if [ -f "$CJ_SYSD/$cj_u" ] && cmp -s "$cj_s" "$CJ_SYSD/$cj_u" && { [ "$cj_w" = "-" ] || [ -L "$CJ_SYSD/$cj_w/$cj_u" ]; }; then
        echo "-- $cj_u 已是最新，未动 /usr"
        return 0
    fi
    cj_backup_file "$CJ_SYSD/$cj_u" || return 1
    cj_with_rootfs_rw cj_usr_put_body "$cj_s" "$cj_u" "$cj_w" || return 1
    systemctl daemon-reload
    # shellcheck disable=SC2034
    CJ_UNIT_CHANGED=1
    echo "-- $cj_u 已装入 $CJ_SYSD（普通重启不丢；OTA 冲掉后重跑）"
    return 0
}

cj_usr_rm_body() { # UNIT WANTS…
    cj_ub=$1; shift
    rm -f "$CJ_SYSD/$cj_ub"
    for cj_wd in "$@"; do rm -f "$CJ_SYSD/$cj_wd/$cj_ub"; done
    return 0
}

# cj_remove_usr_unit UNIT [WANTS_DIR…]：停用 + 删单元与 wants 链接。返回 0/1/3(verity)
cj_remove_usr_unit() {
    cj_u=$1; shift
    [ $# -gt 0 ] || set -- multi-user.target.wants
    systemctl disable --now "$cj_u" 2>/dev/null || true
    cj_present=0
    [ -e "$CJ_SYSD/$cj_u" ] && cj_present=1
    for cj_wd in "$@"; do [ -L "$CJ_SYSD/$cj_wd/$cj_u" ] && cj_present=1; done
    if [ "$cj_present" = "0" ]; then
        echo "-- $CJ_SYSD/$cj_u 本来就不存在，跳过"
        return 0
    fi
    if cj_verity_active; then
        echo "✋ dm-verity 激活，rootfs 不可写——$cj_u 已 stop/disable，但 /usr 里的单元文件与 wants 开机链接删不掉，"
        echo "   重启设备后它可能被 wants 链接重新拉起，直到下次固件 OTA 冲掉 rootfs；要彻底移除需先解除 dm-verity（不建议）。"
        return 3
    fi
    cj_with_rootfs_rw cj_usr_rm_body "$cj_u" "$@" || return 1
    systemctl daemon-reload
    echo "-- 已删 $CJ_SYSD/$cj_u 及其 wants 软链"
    return 0
}

# cj_uninstall_usr_unit UNIT [WANTS_DIR…]：卸载步骤用的宽松版——dm-verity 跳过（3）不算失败，其它失败才返回 1
cj_uninstall_usr_unit() {
    cj_rc=0
    cj_remove_usr_unit "$@" || cj_rc=$?
    [ "$cj_rc" = 0 ] || [ "$cj_rc" = 3 ]
}

# ── 扩展 .so 延后换入（H3）──────────────────────────────────────────────────
# 待换入区里的文件一律换进 $CJ_XOVI/extensions.d/<同名>。放在 /home（持久）：设备中途重启也不丢，下一次
# cj_xochitl_apply 照样换入；在那之前 xochitl 仍用旧版（安装脚本会如实提示"尚未生效"）。
# cj_so_stage SRC：把 SRC 放进待换入区（同名覆盖旧的待换入版本）
cj_so_stage() {
    mkdir -p "$CJ_SO_PENDING_DIR" || return 1
    cj_tmp="$CJ_SO_PENDING_DIR/.$(basename "$1").new.$$"
    if cp "$1" "$cj_tmp" && mv -f "$cj_tmp" "$CJ_SO_PENDING_DIR/$(basename "$1")"; then return 0; fi
    rm -f "$cj_tmp"
    return 1
}
# cj_so_unstage NAME：撤掉 NAME 的待换入版本（新部署的与已装的相同时，旧的待换入版本已过时）
cj_so_unstage() {
    rm -f "$CJ_SO_PENDING_DIR/$1"
    rmdir "$CJ_SO_PENDING_DIR" 2>/dev/null || true
    return 0
}
# cj_so_pending_list：列出待换入的文件名（一行一个；没有则无输出）
cj_so_pending_list() {
    [ -d "$CJ_SO_PENDING_DIR" ] || return 0
    for cj_sf in "$CJ_SO_PENDING_DIR"/*; do
        [ -f "$cj_sf" ] && basename "$cj_sf"
    done
    return 0
}
# cj_so_commit：把待换入区全部换进 extensions.d（原子 rename）。只许在 xochitl 没映射这些 .so 时调用
# （已 stop，或运行中的 xochitl 没带 xovi）。任何一个失败返回 1，已换好的不回滚。
cj_so_commit() {
    cj_rc=0
    for cj_sn in $(cj_so_pending_list); do
        if cj_safe_replace "$CJ_SO_PENDING_DIR/$cj_sn" "$CJ_XOVI/extensions.d/$cj_sn" "$CJ_STAGE_DIR" 755; then
            rm -f "$CJ_SO_PENDING_DIR/$cj_sn"
            echo "-- 换入 $cj_sn -> $CJ_XOVI/extensions.d/"
        else
            echo "!! 换入 $cj_sn 失败"; cj_rc=1
        fi
    done
    rmdir "$CJ_SO_PENDING_DIR" 2>/dev/null || true
    return "$cj_rc"
}

# ── xochitl 重启与健康检查（H1）───────────────────────────────────────────
cj_xochitl_pid() {
    cj_p=$(systemctl show xochitl -p MainPID --value 2>/dev/null) || cj_p=0
    echo "${cj_p:-0}"
}

# 运行中的 xochitl 进程里是否已带 xovi（LD_PRELOAD 含 xovi.so）
cj_xochitl_has_xovi() {
    cj_p=$(cj_xochitl_pid)
    [ "$cj_p" != "0" ] && [ -r "$CJ_PROC/$cj_p/environ" ] || return 1
    tr '\0' '\n' < "$CJ_PROC/$cj_p/environ" | grep -q '^LD_PRELOAD=.*xovi\.so'
}

# cj_count_maps TAG PID：该进程 maps 里含 TAG 的行数（永远只输出一个整数）
cj_count_maps() {
    cj_c=$(grep -c "$1" "$CJ_PROC/$2/maps" 2>/dev/null) || cj_c=0
    echo "${cj_c:-0}"
}

# cj_xochitl_apply：让已落盘的 xovi 扩展/qmd 生效。**2026-09-25 起一律主动整机重启**（用户拍板），不再停止/重启
# xochitl——xochitl 自己退出时有竞态：memfault 栈 5 份，3 份崩在 xochitl 自己的线程池（调用方 xochitl+0x64809b，
# 崩点 0x6467b8 / 0x649fc8：09-21 换 appload 后、09-25 卸载后重装、09-25 只换了 qmd 的普通 restart），2 份崩在
# libQt6Gui+0x495098（09-20 在已生效的 xochitl 上跑 xovi/start、09-24 rename 换 hw-stroke 后 restart）。停止它
# 有相当概率 SEGV → OnFailure → rm-emergency → 整机重启，跟扩展文件动没动过无关（09-25 早先"只有映射的扩展被
# 删/换才崩"的归因被同日 10:37 那次推翻）。与其走一趟崩溃 + 应急路径，不如直接干净地重启：
#   xovi 已生效，或装了 xovi-reenable（开机自动恢复 xovi）→ 换入待换入区 → `systemctl reboot`（约 20–60 秒回来）；
#   都没有（没装 xovi-persist，重启后 xovi 不会自己回来）→ 仍走 $CJ_XOVI/start（xochitl 此时不带 xovi）。
# 打印 CJ-APPLY-REBOOTING 让 host 侧（lib.sh 的 run_apply）认出"连接断开是因为设备在重启"；置 CJ_APPLY_REBOOTED=1
# 让同一段设备端脚本跳过重启后的健康检查（交给设备回来后的 verify-on-device.sh）。
# ⚠ 绝不在 xovi 已生效时跑 xovi/start（它 umount 再重挂 drop-in 目录，xochitl SEGV，2026-09-20 真机事故）。
# 会打断阅读/书写，所以先打印提示并留 CJ_APPLY_GRACE 秒宽限（ssh 断开/Ctrl-C 可在此期间取消）。
# shellcheck disable=SC2034  # 由各调用方的设备端脚本读
CJ_APPLY_REBOOTED=0
cj_xochitl_apply() {
    if cj_xochitl_has_xovi || [ -f "$CJ_SYSD/xovi-reenable.service" ]; then
        echo "⚠ 设备将在 ${CJ_APPLY_GRACE:-5} 秒后整机重启让改动生效（约 1 分钟回来），会打断当前的阅读/书写。"
        echo "   （不再单独重启 xochitl：它退出时有概率崩溃并触发整机重启，见 devlib.sh 头注 H3）"
        sleep "${CJ_APPLY_GRACE:-5}"
        trap '' HUP PIPE INT TERM   # 关键区：换入到一半断线也要把重启排上（写已关闭的管道只让 echo 失败）
        cj_ap_rc=0
        cj_so_commit || cj_ap_rc=1
        # 重启前就清待生效标记：/run 那份重启自然清，但退路目录在 /home、重启不清——留着的话设备回来后
        # xovi-apply 又判"有待生效"再重启一次，成了重启循环。换入失败（cj_ap_rc=1）时保留标记，下次还会再试。
        [ "$cj_ap_rc" = 1 ] || cj_pending_clear
        sync
        # shellcheck disable=SC2034  # 调用方的设备端脚本读
        CJ_APPLY_REBOOTED=1
        echo "CJ-APPLY-REBOOTING"
        if ! systemctl reboot --no-block; then
            # 重启没排上：待换入区已换入、标记已清，xochitl 却还在用旧的——把标记补回去，下次 xovi-apply 还会再试；
            # 另打一行让 host 侧 run_apply 别去空等设备重启（2026-09-25 审计）
            cj_ap_rc=1
            cj_pending_mark apply-reboot-failed || true
            echo "!! systemctl reboot 失败——改动已落盘但没生效；请在设备上手动 reboot"
            echo "CJ-APPLY-REBOOT-FAILED"
        fi
        trap - HUP PIPE INT TERM
        return "$cj_ap_rc"
    fi
    [ -x "$CJ_XOVI/start" ] || { echo "!! 没找到 $CJ_XOVI/start —— 先在设备上跑：vellum add xovi"; return 1; }
    echo "⚠ 即将重启 xochitl —— 屏幕会闪烁，并打断当前的阅读/书写（请勿操作设备）；${CJ_APPLY_GRACE:-5} 秒后开始。"
    sleep "${CJ_APPLY_GRACE:-5}"
    cj_so_commit || return 1   # 运行中的 xochitl 没带 xovi，没映射扩展，可以直接换
    echo "-- xochitl 里还没有 xovi、也没装 xovi-reenable（整机重启后不会自动恢复）→ $CJ_XOVI/start"
    "$CJ_XOVI/start" || return 1
    cj_pending_clear   # 重启成功：此前所有"待生效"的落盘内容现在都已被新进程载入
    return 0
}

# cj_xochitl_health OLD_PID [MAP_TAG…]：重启后核对 is-active / MainPID / NRestarts，并列出各扩展的 maps 段数
cj_xochitl_health() {
    cj_old=${1:-0}
    [ $# -gt 0 ] && shift
    sleep "${CJ_HEALTH_SLEEP:-5}"
    cj_st=$(systemctl is-active xochitl 2>/dev/null || true)
    cj_new=$(cj_xochitl_pid)
    cj_nr=$(systemctl show xochitl -p NRestarts --value 2>/dev/null || echo '?')
    echo "=================================================="
    echo "  is-active : $cj_st   (期望 active)"
    echo "  MainPID   : $cj_old -> $cj_new   (期望有变化)"
    echo "  NRestarts : $cj_nr   (期望 0/不增)"
    for cj_t in "$@"; do
        echo "  $cj_t 加载 : $(cj_count_maps "$cj_t" "$cj_new") 段   (期望 >0)"
    done
    echo "  xovi.so 总段数: $(cj_count_maps 'xovi\.so' "$cj_new")（期望 >0）"
    echo "=================================================="
    [ "$cj_st" = "active" ] && [ "$cj_new" != "0" ]
}
