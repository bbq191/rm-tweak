#!/bin/sh
# shellcheck shell=sh
# ═══════════════════════════════════════════════════════════════════════════
# lib.sh —— packaging/ 下所有 host 侧脚本共用的函数库（2026-09-20 脚本审计后抽出）。
#
# 用法：调用方先 `cd "$(dirname "$0")"`（进 packaging/），再 `. ./lib.sh`，设好全局 HOST。
# 设备侧对应的库是 devlib.sh（本库的 dev_script 会把它拼在 heredoc 脚本前面经 ssh 送上设备）。
#
# 提供：
#   ssh 封装（M11）  rssh（stdin=/dev/null）· rssh_in（透传 stdin）· rscp —— 统一
#                    BatchMode + ConnectTimeout，休眠/断线时快速失败而不是卡死
#   shquote          把任意字符串安全地拼进远端命令行（M9/M10）
#   dev_script       `dev_script ARG… <<'EOF' … EOF`：devlib.sh + 脚本体经 `ssh sh -s` 在设备上执行
#   push_verified    一批文件 scp 到"暂存路径"→ 逐个 md5 对拍；不对就删暂存并失败，绝不落到最终位置（H3）；
#                    一次 ssh 建目录 + 每文件一次 scp + 一次 ssh 取全部 md5
#   步骤表           STEP_ORDER / step_script / STEP_DEFER / STEP_CONFIG_ONLY（install-all 与 uninstall-all 共用，
#                    保证两边清单对称）
#   parse_step_args  install-all / uninstall-all 共用的 [host] --force --purge --force-apply --dry-run --skip -h 解析
#                    （调用方先定义 usage()）
#   run_step / skip_has   （DRY=1 时 run_step 只打印将执行的命令，不连设备；SKIPPED/NOTAPPL/DONE/FAILED 记账）
#   step_skipped     步骤因前置条件不满足而跳过（非失败）：打印原因；在 run_step 编排下另记入汇总的"前置条件不满足"栏
#   host_arg         薄 deploy-*.sh 共用的 [host] 参数解析（-h、多余/未知参数 exit 2）
#   require_device   动手前确认 ssh 通；不通给下一步排查提示并 exit 1
#   fw_gate          固件 sha256 白名单门（install-all）
#   preflight_device 设备只读预检：root/ /home 可写与剩余空间/xovi·qrr·appload·verity 现状（install-all）
# ═══════════════════════════════════════════════════════════════════════════

CJ_PKG_DIR="$(pwd)"
CJ_SSH_TIMEOUT="${CJ_SSH_TIMEOUT:-8}"
CJ_STAGE_REMOTE="${CJ_STAGE_REMOTE:-/home/root/.cangjie-stage}"   # 设备上的暂存目录（与 devlib.sh 的 CJ_STAGE_DIR 同址）

# shellcheck disable=SC2086  # CJ_SSH_OPTS 有意按词展开成多个选项
rssh()    { ssh -n $CJ_SSH_OPTS "root@$HOST" "$@"; }
# shellcheck disable=SC2086
rssh_in() { ssh $CJ_SSH_OPTS "root@$HOST" "$@"; }
# shellcheck disable=SC2086
rscp()    { scp -q $CJ_SSH_OPTS "$@"; }
CJ_SSH_OPTS="-o BatchMode=yes -o ConnectTimeout=$CJ_SSH_TIMEOUT"

# 单引号转义，可安全拼进远端 shell 命令行
shquote() {
    printf "'%s'" "$(printf '%s' "$1" | sed "s/'/'\\\\''/g")"
}

# dev_script [ARG…]  ← stdin 是脚本体。devlib.sh + 脚本体经 ssh 在设备上 `sh -s -- ARG…` 执行。
dev_script() {
    ds_args=""
    for ds_a in "$@"; do ds_args="$ds_args $(shquote "$ds_a")"; done
    { cat "$CJ_PKG_DIR/devlib.sh"; cat; } | rssh_in "sh -s --$ds_args"
}

# run_apply CMD…：跑一段可能让设备主动整机重启的设备端命令（内部调了 devlib.sh 的 cj_xochitl_apply）。输出照常
# 打到终端并留一份；见到设备端打印的 CJ-APPLY-REBOOTING 就按成功处理——ssh 随重启断开（退出码 255）不算失败——
# 并提示设备回来后跑 verify-on-device.sh；见到 CJ-APPLY-REBOOT-FAILED（systemctl reboot 本身失败）则不空等、按失败返回。
# 其余情况原样返回 CMD 的退出码。stdin 原样交给 CMD（可接 heredoc）。
run_apply() {
    ra_log="$(mktemp)"; ra_rcf="$(mktemp)"
    { ra_c=0; "$@" || ra_c=$?; echo "$ra_c" > "$ra_rcf"; } | tee "$ra_log"
    ra_rc="$(cat "$ra_rcf")"
    if grep -q '^CJ-APPLY-REBOOT-FAILED$' "$ra_log"; then
        rm -f "$ra_log" "$ra_rcf"
        echo "!! 设备没能排上整机重启（见上），改动尚未生效；设备上手动 reboot 后跑：sh verify-on-device.sh $HOST"
        [ "$ra_rc" != 0 ] || ra_rc=1
        return "$ra_rc"
    fi
    if grep -q '^CJ-APPLY-REBOOTING$' "$ra_log"; then
        rm -f "$ra_log" "$ra_rcf"
        wait_reboot_and_verify
        return $?
    fi
    rm -f "$ra_log" "$ra_rcf"
    return "$ra_rc"
}

# wait_reboot_and_verify：设备已排上整机重启——先等它断开（免得关机前就连上又判"回来了"），再等它回来，
# 再给 xovi-reenable 与各服务一点时间起齐，然后跑 verify-on-device.sh，退出码即结果（有 ✗ 为 1）。
# CJ_APPLY_VERIFY=0 只提示不等；CJ_REBOOT_DOWN_WAIT / CJ_REBOOT_UP_WAIT / CJ_REBOOT_SETTLE（秒，缺省 60/240/20）。
wait_reboot_and_verify() {
    if [ "${CJ_APPLY_VERIFY:-1}" = 0 ]; then
        echo "✅ 改动已落盘，设备正在整机重启让它生效（约 1 分钟）。回来后核对：sh verify-on-device.sh $HOST"
        return 0
    fi
    echo "-- 改动已落盘，设备正在整机重启；等它回来后自动核对（不想等：Ctrl-C，之后自己跑 sh verify-on-device.sh $HOST）"
    wr_t=0
    while [ "$wr_t" -lt "${CJ_REBOOT_DOWN_WAIT:-60}" ] && rssh true >/dev/null 2>&1; do
        sleep 2; wr_t=$((wr_t + 2))
    done
    wr_t=0
    until rssh true >/dev/null 2>&1; do
        if [ "$wr_t" -ge "${CJ_REBOOT_UP_WAIT:-240}" ]; then
            echo "!! 设备 ${CJ_REBOOT_UP_WAIT:-240} 秒内没回来。检查连接（WiFi/USB）后手动跑：sh verify-on-device.sh $HOST"
            return 1
        fi
        sleep 5; wr_t=$((wr_t + 5))
    done
    sleep "${CJ_REBOOT_SETTLE:-20}"
    echo "-- 设备已回来，核对："
    sh "$CJ_PKG_DIR/verify-on-device.sh" "$HOST"
}

# host_arg USAGE "$@"：薄 deploy-*.sh 共用的参数解析——`[host]`，-h/--help 打印用法，多余/未知参数 exit 2。设 HOST。
host_arg() {
    ha_usage="$1"; shift
    case "${1:-}" in -h|--help) echo "$ha_usage"; exit 0 ;; esac
    [ $# -le 1 ] || { echo "!! 参数太多：$*"; echo "$ha_usage"; exit 2; }
    HOST="${1:-10.11.99.1}"
    case "$HOST" in -*) echo "!! 未知参数：$HOST"; echo "$ha_usage"; exit 2 ;; esac
}

# require_device：动手前先确认 ssh 通（BatchMode，不会卡在密码提示上）；不通给出下一步该查什么，exit 1。
# install-all 自己确认过后导出 CJ_DEVICE_OK=<host>，它编排的各步骤就不再各连一次（整轮省 10 次连接）；
# 之后设备真断了，该步骤的第一条 ssh 会原样报错、记为失败。
require_device() {
    [ "${CJ_DEVICE_OK:-}" != "$HOST" ] || return 0
    rd_err="$(rssh true 2>&1)" || {
        echo "!! 连不上 root@$HOST（${CJ_SSH_TIMEOUT}s 超时，BatchMode）。ssh 报错："
        echo "$rd_err" | sed 's/^/     /'
        echo "   下一步：① 设备是否休眠/没插 USB（接口消失=物理连接或休眠）；② 接口在但 IP 不对：sudo ip addr add 10.11.99.2/24 dev <网卡>；"
        echo "           ③ 提示 host key 变了（OTA/重装后常见）：ssh-keygen -R $HOST；④ 提示 Permission denied：ssh-copy-id root@$HOST"
        exit 1
    }
}

md5_local() { md5sum "$1" | awk '{print $1}'; }

# push_verified LOCAL REMOTE [LOCAL REMOTE …]：把一批文件 scp 到各自的"暂存路径"（调用方保证不在 extensions.d
# 之类的自动加载目录里）并核对 md5。md5 对不上的那个文件从设备上删掉、整体返回 1；最终位置从未被碰过。
# 连接次数：一次 ssh 建好所有目标目录 → 每个文件一次 scp → 一次 ssh 取回全部 md5（2026-09-25 起合批；
# 旧版每个文件 mkdir/scp/md5 各一次，deploy-xovi-ext 光推送就 12 次连接）。
push_verified() {
    { [ $# -ge 2 ] && [ $(($# % 2)) -eq 0 ]; } || { echo "!! push_verified：参数必须成对（LOCAL REMOTE …）"; return 1; }
    pv_dirs=""; pv_rems=""; pv_odd=1
    for pv_a in "$@"; do
        if [ "$pv_odd" = 0 ]; then
            pv_d="$(shquote "$(dirname "$pv_a")")"
            case " $pv_dirs " in *" $pv_d "*) ;; *) pv_dirs="$pv_dirs $pv_d" ;; esac
            pv_rems="$pv_rems $(shquote "$pv_a")"
        fi
        pv_odd=$((1 - pv_odd))
    done
    rssh "mkdir -p$pv_dirs" || return 1
    pv_odd=1
    for pv_a in "$@"; do
        if [ "$pv_odd" = 1 ]; then pv_local=$pv_a
        else rscp "$pv_local" "root@$HOST:$pv_a" || { echo "!! scp $pv_local 失败"; return 1; }
        fi
        pv_odd=$((1 - pv_odd))
    done
    # 每个文件一行 md5（缺失则空行），顺序与参数一致
    pv_sums="$(rssh "for f in$pv_rems; do s=\$(md5sum \"\$f\" 2>/dev/null) || s=; echo \"\${s%% *}\"; done")" || pv_sums=""
    pv_bad=""; pv_odd=1; pv_k=0
    for pv_a in "$@"; do
        if [ "$pv_odd" = 1 ]; then pv_local=$pv_a
        else
            pv_k=$((pv_k + 1))
            pv_l="$(md5_local "$pv_local")"
            pv_r="$(printf '%s\n' "$pv_sums" | sed -n "${pv_k}p")"
            if [ -z "$pv_r" ] || [ "$pv_l" != "$pv_r" ]; then
                echo "!! md5 对不上：$(basename "$pv_local")（本地 $pv_l vs 设备 ${pv_r:-空}），传输可能损坏，不继续（已删设备上的暂存文件 $pv_a，最终位置未动）"
                pv_bad="$pv_bad $(shquote "$pv_a")"
            else
                echo "-- md5 一致：$(basename "$pv_local")"
            fi
        fi
        pv_odd=$((1 - pv_odd))
    done
    [ -z "$pv_bad" ] && return 0
    rssh "rm -f$pv_bad" || true
    return 1
}

# ── 固件安全门（install-all 用）──────────────────────────────────────────
# 白名单 = 仓库里的 firmware-allowlist.txt + 本机 firmware-allowlist.local.txt（--force 追加到后者，已 gitignore，
# 不再改动被 git 跟踪的文件，避免"未验证的哈希"被误提交；可用 CJ_ALLOWLIST_LOCAL 改路径）。
CJ_ALLOWLIST="${CJ_ALLOWLIST:-$CJ_PKG_DIR/firmware-allowlist.txt}"
CJ_ALLOWLIST_LOCAL="${CJ_ALLOWLIST_LOCAL:-$CJ_PKG_DIR/firmware-allowlist.local.txt}"
fw_gate() { # $1=FORCE(0/1)
    echo "═══ 固件安全门（root@$HOST）═══"
    fw_hash="$(rssh 'sha256sum /usr/bin/xochitl' | awk '{print $1}')"
    if [ -z "$fw_hash" ]; then
        echo "!! 没拿到 /usr/bin/xochitl 的 sha256（ssh 连不上，或设备上没有这个文件？）"
        return 1
    fi
    fw_line=""
    for fw_f in "$CJ_ALLOWLIST" "$CJ_ALLOWLIST_LOCAL"; do
        [ -f "$fw_f" ] || continue
        fw_line="$(grep "^${fw_hash}[[:space:]]" "$fw_f" | head -n 1)" && [ -n "$fw_line" ] && break
        fw_line=""
    done
    if [ -n "$fw_line" ]; then
        echo "-- 固件命中白名单：$(echo "$fw_line" | awk '{$1=""; print}')"
    elif [ "$1" = "1" ]; then
        echo "⚠️  固件不在白名单（sha256=$fw_hash），--force 强装——追加进 $CJ_ALLOWLIST_LOCAL（本机文件，不入 git）"
        echo "$fw_hash  (--force 追加，未验证，$(date +%Y-%m-%d))" >> "$CJ_ALLOWLIST_LOCAL"
    else
        echo "!! 固件不在白名单（sha256=$fw_hash）。"
        echo "   这台设备的 xochitl 没有在这套安装脚本上验证过注入定位，qmd/hook 偏移可能对不上。"
        echo "   确认这台设备的固件确实跟已验证过的版本一致，要强装就加 --force（会记录这个哈希到本机文件）。"
        return 1
    fi
}

# ── 步骤表（install-all / uninstall-all 共用；两边清单靠它对称）──────────────
# 顺序：先与 xovi/vellum 无关的独立项，再 battop/wifi-watch，再依赖 xovi 的，shelf 最重，xovi-apply 放最后统一重启一次。
STEP_ORDER="chrony-cn chrony-boot-wakelock timezone-cn battop wifi-watch xovi-persist hl-snap handwriting-stroke sidebar-entry shelf xovi-apply"
# 只落盘、不各自重启 xochitl 的步骤（install-all 给它们传 DEFER_XOVI_START=1，最后由 xovi-apply 统一重启）
# shellcheck disable=SC2034  # 由 install-all.sh 使用
STEP_DEFER="hl-snap handwriting-stroke sidebar-entry"
# 没有"卸载"语义的步骤：配置覆写（chrony-cn/timezone-cn），以及纯动作（xovi-apply）
# shellcheck disable=SC2034  # 由 uninstall-all.sh 使用
STEP_CONFIG_ONLY="chrony-cn timezone-cn xovi-apply"

step_script() {
    case "$1" in
        chrony-cn) echo ./deploy-chrony-cn.sh ;;
        chrony-boot-wakelock) echo ./deploy-chrony-boot-wakelock.sh ;;
        timezone-cn) echo ./deploy-timezone-cn.sh ;;
        battop) echo ./deploy-battop.sh ;;
        wifi-watch) echo ./deploy-wifi-watch.sh ;;
        xovi-persist) echo ./deploy-xovi-persist.sh ;;
        hl-snap) echo ./deploy-hl-snap.sh ;;
        handwriting-stroke) echo ./deploy-handwriting-stroke.sh ;;
        sidebar-entry) echo ./deploy-sidebar-entry.sh ;;
        shelf) echo ./deploy.sh ;;
        xovi-apply) echo ./deploy-xovi-apply.sh ;;
        *) return 1 ;;
    esac
}
word_in() { case " $2 " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

# step_payload STEP：该步骤的 deploy-* 推到设备 $HOME 下的载荷——"目录名 文件…"（文件在前、子目录在后）。
# deploy-usr-unit / deploy-xovi-ext 按它定推送目录，uninstall-all 按它清（cj_rm_payload），两边同一份清单
#（2026-09-25 审计；原先两边各写一遍）。没有独立载荷目录的步骤返回 1。
step_payload() {
    case "$1" in
        chrony-boot-wakelock) echo "pkg-chrony-boot-wakelock chrony-boot-wakelock.service" ;;
        xovi-persist) echo "pkg-xovi-persist xovi-reenable.service" ;;
        wifi-watch) echo "pkg-wifi-watch wifi-watch.service wifi-watch.sh" ;;
        hl-snap) echo "hl-snap hl-snap.so deploy/install.sh deploy/xovi-ext-install.sh deploy/devlib.sh deploy" ;;
        handwriting-stroke) echo "hw-stroke hw-stroke.so deploy/install.sh deploy/xovi-ext-install.sh deploy/devlib.sh deploy" ;;
        *) return 1 ;;
    esac
}
step_payload_dir() { sp_p="$(step_payload "$1")" || return 1; echo "${sp_p%% *}"; }

# ── 参数解析 / 步骤运行 ────────────────────────────────────────────────────
# parse_step_args "$@"  →  设 HOST FORCE PURGE SKIP DRY FORCE_APPLY；未知参数 exit 2。
# 用法：[host] [--force] [--purge] [--force-apply] [--dry-run] [--skip a,b | --skip=a,b] [-h|--help]
#   调用方需先定义 usage()（打印帮助）——-h/--help 调它后 exit 0。
# shellcheck disable=SC2034  # FORCE/PURGE/DRY/FORCE_APPLY 由调用方（install-all/uninstall-all）使用
parse_step_args() {
    HOST="10.11.99.1"; FORCE=0; PURGE=0; SKIP=""; DRY=0; FORCE_APPLY=0
    case "${1:-}" in ""|-*) ;; *) HOST="$1"; shift ;; esac
    while [ $# -gt 0 ]; do
        case "$1" in
            -h|--help) usage; exit 0 ;;
            --force) FORCE=1 ;;
            --purge) PURGE=1 ;;
            --force-apply) FORCE_APPLY=1 ;;
            --dry-run) DRY=1 ;;
            --skip=*) SKIP="${1#--skip=}" ;;
            --skip) [ $# -ge 2 ] || { echo "!! --skip 需要参数（逗号分隔的步骤名）"; exit 2; }; SKIP="$2"; shift ;;
            *) echo "!! 未知参数：$1（-h 看用法）"; exit 2 ;;
        esac
        shift
    done
    for ps_s in $(echo "$SKIP" | tr ',' ' '); do
        word_in "$ps_s" "$STEP_ORDER" || echo "⚠ --skip 里的 '$ps_s' 不是已知步骤名（已知：$STEP_ORDER）"
    done
}

# 该步骤名是否要跑（--skip 没点名）
skip_has() { case ",$SKIP," in *",$1,"*) return 0 ;; *) return 1 ;; esac; }

# ── 设备预检（install-all 用）：只读检查，能提前拦住的全在这拦（磁盘满/非 root/写不了 /home），
#   其余（xovi/qrr/appload 缺失）只报告——对应步骤自己会清楚报错或跳过。返回 1 = 不该继续装。──
CJ_MIN_FREE_KB="${CJ_MIN_FREE_KB:-51200}"     # /home 可用空间低于此值拒装（≈50MB：连 shelf 二进制都放不下）
CJ_WARN_FREE_KB="${CJ_WARN_FREE_KB:-204800}"  # 低于此值只警告（≈200MB：shelf 载荷+备份余量偏紧）
preflight_device() {
    echo "═══ 设备预检（root@$HOST，只读）═══"
    dev_script "$CJ_MIN_FREE_KB" "$CJ_WARN_FREE_KB" <<'DEVICE_SCRIPT'
set -eu
MINF="$1"; WARNF="$2"
cj_require_root || exit 1
[ -d "$CJ_HOME" ] && [ -w "$CJ_HOME" ] || { echo "!! $CJ_HOME 不可写——/home 分区没挂载好？先在设备上 mount | grep home"; exit 1; }
FREE="$(df -kP "$CJ_HOME" 2>/dev/null | awk 'END{print $4}')"
case "$FREE" in ''|*[!0-9]*) echo "⚠ 读不到 $CJ_HOME 的可用空间，跳过空间检查" ;; *)
    if [ "$FREE" -lt "$MINF" ]; then
        echo "!! $CJ_HOME 只剩 $((FREE / 1024)) MB 可用，装不下（至少 $((MINF / 1024)) MB）。先清理：ls -la $CJ_HOME、$CJ_BACKUP_DIR"; exit 1
    fi
    if [ "$FREE" -lt "$WARNF" ]; then echo "⚠ $CJ_HOME 只剩 $((FREE / 1024)) MB 可用（建议 ≥ $((WARNF / 1024)) MB），继续但空间偏紧"
    else echo "-- $CJ_HOME 可用 $((FREE / 1024)) MB"; fi ;;
esac
have() { [ -e "$1" ] && echo "有" || echo "无"; }
echo "-- xovi 本体 : $(have "$CJ_XOVI/xovi.so")   （无 → xovi-persist/hl-snap/handwriting-stroke/xovi-apply 会失败：先 vellum add xovi）"
echo "-- qt-resource-rebuilder : $(have "$CJ_XOVI/exthome/qt-resource-rebuilder")   （无 → sidebar-entry 与 shelf 的 qmd 自动跳过）"
echo "-- appload   : $(have "$CJ_XOVI/exthome/appload")   （无 → sidebar-entry 自动跳过；3.28 固件需 ≥ 0.6.0）"
if cj_verity_active; then echo "-- dm-verity : 激活 → 所有写 /usr 的单元（chrony-boot-wakelock/xovi-persist/wifi-watch/battop/shelf 开机链接）会被跳过"; else echo "-- dm-verity : 未激活"; fi
if cj_xochitl_has_xovi; then echo "-- xochitl 里 xovi 已生效 → 有改动时最后一步换入后整机重启（不跑 xovi/start、不 restart xochitl）"; else echo "-- xochitl 里 xovi 尚未生效 → 最后一步让它生效：装了 xovi-reenable（本轮 xovi-persist 会装，dm-verity 下装不上）就整机重启，否则 xovi/start"; fi
DEVICE_SCRIPT
}

DONE=""
FAILED=""
SKIPPED=""
NOTAPPL=""   # 步骤自己判定前置条件不满足、跳过（退出 0，但不该算进"已安装"）

# step_skipped REASON：步骤因前置条件不满足/不适用而跳过（非失败）时调用，调用后照常 exit 0。
# 单独跑时只打印原因；在 run_step 编排下（它导出 CJ_STEP_SKIP_FILE）另把原因写进该文件，
# 汇总时这一步列进"已跳过（前置条件不满足）"而不是"已安装"——不然"跳过"混在"已安装"里，容易漏看。
step_skipped() {
    echo "-- $1——跳过，非失败"
    if [ -n "${CJ_STEP_SKIP_FILE:-}" ]; then printf '%s\n' "$1" > "$CJ_STEP_SKIP_FILE"; fi
}

# run_step NAME CMD [ARGS…]：跳过判定 + 执行 + 记账（DRY=1 时只打印将执行的命令，不执行、不连设备）
run_step() {
    rs_name="$1"; shift
    if skip_has "$rs_name"; then
        echo; echo "-- 跳过 $rs_name（--skip）"
        SKIPPED="$SKIPPED $rs_name"
        return 0
    fi
    echo; echo "═══ $rs_name ═══"
    if [ "${DRY:-0}" = "1" ]; then
        echo "-- [dry-run] 将执行：$*"
        DONE="$DONE $rs_name"
        return 0
    fi
    rs_skipf="$(mktemp)"
    export CJ_STEP_SKIP_FILE="$rs_skipf"
    if "$@"; then
        if [ -s "$rs_skipf" ]; then
            NOTAPPL="$NOTAPPL
   $rs_name：$(head -n 1 "$rs_skipf")"
        else
            DONE="$DONE $rs_name"
        fi
    else
        echo "!! $rs_name 失败（见上面这一步的原始报错）"
        FAILED="$FAILED $rs_name"
    fi
    unset CJ_STEP_SKIP_FILE
    rm -f "$rs_skipf"
}
