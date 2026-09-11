#!/bin/sh
# ═══════════════════════════════════════════════════════════════════════════
# verify-on-device.sh —— 部署后健康核对（host 侧，2026-09-25 新增）。经 ssh **只读**采集设备现状，逐项给 ✓/⚠/✗。
#
# 为什么要有：每次部署后要核的东西（xovi 生没生效、扩展 .so 是不是换了文件没重启、qmd 在不在、各服务
# NRestarts/内存、本次开机有没有 panic/OOM、OTA 有没有冲掉 /usr 单元……）以前靠人记、靠临场敲命令，漏过。
#
# 只读：设备上**不重启、不写、不删**任何东西——不调 systemctl start/stop/restart、不 mount、不建临时文件；
# 只读 /proc、systemctl show/is-active、journalctl、dmesg、ls/stat/df/sha256sum。
#
# 结构：采集与判定分开——
#   · 设备端采集（本文件末尾的 heredoc，经 lib.sh 的 dev_script 一次 ssh 送上去）：只输出结构化文本
#     （每行 `键<TAB>字段…`，空值写成 "-"，因为 IFS=TAB 时连续 TAB 会被合并）；
#   · host 端判定（judge_*）：只读这份文本出结论，不连设备——所以 `--from 文件` 可以离线重判，
#     模拟测试也直接喂构造好的文本（packaging/tests/run_sim_tests.sh）。
#
# 用法：./verify-on-device.sh [host] [--json] [--dump] [--from FILE] [--flight-lines N]
#   host             默认 10.11.99.1（USB）；WiFi 下给设备 IP 或 shelf.local
#   --json           只输出一个 JSON 对象（含 ok/warn/fail 计数与逐项结果）
#   --dump           只打印设备端采集的原始文本（可存档，之后 --from 重判），不判定
#   --from FILE      不连设备，判定之前 --dump 存下的文本
#   --flight-lines N 飞行记录仪取最后 N 行（默认 8）
# 退出码：0 = 没有 ✗（可以有 ⚠）；1 = 有 ✗ 或连不上设备；2 = 参数错误（不连设备）。
# 文本模式最后一行固定是机器可读汇总：VERIFY-SUMMARY host=… ok=N warn=N fail=N result=PASS|WARN|FAIL
# 环境：CJ_SSH_TIMEOUT（lib.sh）· CJ_MIN_FREE_KB / CJ_WARN_FREE_KB（lib.sh，/home 空间阈值，与装前预检同一套）·
#       CJ_VERIFY_HWM_WARN_KB（服务峰值内存 VmHWM 超过它记 ⚠，默认 524288 = 512MB，经验值、非实测）·
#       CJ_VERIFY_UPTIME_WARN（开机不足这么多秒记 ⚠，默认 600）
# ═══════════════════════════════════════════════════════════════════════════
set -eu
ORIG_PWD="$(pwd)"
cd "$(dirname "$0")"
# shellcheck disable=SC1091
. ./lib.sh
# shellcheck disable=SC1091
. ../shelf/manifest.sh   # SHELF_ALL / shelf_svc_of / shelf_svc_qmds / SHELF_LEGACY_QMDS：与安装同一份清单

usage() {
    cat <<'EOF'
用法：./verify-on-device.sh [host] [--json] [--dump] [--from FILE] [--flight-lines N]
  host             默认 10.11.99.1（USB）；WiFi 下给设备 IP 或 shelf.local
  --json           只输出一个 JSON 对象
  --dump           只打印设备端采集的原始文本（可存档），不判定
  --from FILE      不连设备，判定之前 --dump 存下的文本
  --flight-lines N 飞行记录仪取最后 N 行（默认 8）
只读：不重启、不写、不删设备上的任何东西。有 ✗ 退出 1；只有 ⚠ 退出 0。
EOF
}

HOST="10.11.99.1"; HOST_SET=0; JSON=0; DUMP_ONLY=0; FROM=""; FLN=8
while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help) usage; exit 0 ;;
        --json) JSON=1 ;;
        --dump) DUMP_ONLY=1 ;;
        --from=*) FROM="${1#--from=}" ;;
        --from) [ $# -ge 2 ] || { echo "!! --from 需要文件路径"; exit 2; }; FROM="$2"; shift ;;
        --flight-lines=*) FLN="${1#--flight-lines=}" ;;
        --flight-lines) [ $# -ge 2 ] || { echo "!! --flight-lines 需要数字"; exit 2; }; FLN="$2"; shift ;;
        -*) echo "!! 未知参数：$1（-h 看用法）"; exit 2 ;;
        *) [ "$HOST_SET" = 0 ] || { echo "!! 参数太多：$1（host 只能给一个）"; exit 2; }; HOST="$1"; HOST_SET=1 ;;
    esac
    shift
done
case "$FLN" in ''|*[!0-9]*) echo "!! --flight-lines 要正整数：$FLN"; exit 2 ;; esac
[ "$JSON" = 0 ] || [ "$DUMP_ONLY" = 0 ] || { echo "!! --json 与 --dump 不能同时用"; exit 2; }
if [ -n "$FROM" ]; then
    case "$FROM" in /*) ;; *) FROM="$ORIG_PWD/$FROM" ;; esac
    [ -f "$FROM" ] || { echo "!! --from 文件不存在：$FROM"; exit 2; }
fi

CJ_VERIFY_HWM_WARN_KB="${CJ_VERIFY_HWM_WARN_KB:-524288}"
CJ_VERIFY_UPTIME_WARN="${CJ_VERIFY_UPTIME_WARN:-600}"
TAB="$(printf '\t')"
US="$(printf '\037')"   # 条目"详情"多行之间的分隔符（渲染时拆开）

# ── 要核对的 systemd 单元（取自 manifest + 步骤表的载荷位置，不在这里另起一份服务清单）──────────
# 每项 KIND:UNIT:PAYLOAD——KIND：svc=常驻且应 active；opt=常驻但有意不开机自启（battop）；once=oneshot；target。
# PAYLOAD：这个单元对应的设备上载荷（二进制/脚本）；"单元缺失但载荷在" = OTA 冲掉了 /usr，判 ✗。"-" = 无可判载荷。
unit_specs() {
    for us_s in $SHELF_ALL; do
        us_b="$(shelf_svc_of "$us_s")"
        echo "svc:$us_b.service:/home/root/.local/bin/$us_b"
    done
    echo "target:shelf.target:/home/root/.local/bin/gateway"
    echo "svc:wifi-watch.service:/home/root/.local/bin/wifi-watch.sh"
    echo "opt:battop.service:/home/root/battop/battop"
    echo "once:xovi-reenable.service:/home/root/xovi/start"
    echo "once:chrony-boot-wakelock.service:-"
}

# 服务 → 监听端口的兜底表（单元 ExecStart 里有 --bind 就以单元为准；没有的用这张表，与 docs/OVERVIEW.md 端口表一致）
svc_port() {
    case "$1" in
        gateway) echo 443 ;;
        book-serve) echo 8790 ;; koreader-serve) echo 8791 ;; font-serve) echo 8792 ;; wallpaper-serve) echo 8793 ;;
        ink-serve) echo 8795 ;; transcribe-serve) echo 8796 ;; mind-serve) echo 8797 ;; note-serve) echo 8798 ;;
        *) echo "" ;;
    esac
}

# qmd → 它加载后会在 xochitl 日志里打的标记（只作佐证：多数要打开相关界面才打印，没见到不算异常）
qmd_mark() {
    case "$1" in
        reader-page-turn.qmd) echo "CJ-PAGE-TURN: loaded" ;;
        koreader-sidebar-entry.qmd) echo "CJ-SIDEBAR[" ;;
        font-menu-dynamic.qmd) echo "SHELF-FONT:" ;;
        shelf-mkdir-agent.qmd) echo "SHELF-MKDIR:" ;;
        shelf-trash-agent.qmd) echo "SHELF-TRASH:" ;;
        shelf-comic-margins.qmd) echo "CJ-COMIC-MARGIN:" ;;
        *) echo "" ;;
    esac
}
qmd_marks_spec() {   # 传给设备端的 "键=子串" 列表（| 分隔），键 = qmd 文件名
    qm_out=""
    for qm_q in reader-page-turn.qmd koreader-sidebar-entry.qmd font-menu-dynamic.qmd shelf-mkdir-agent.qmd shelf-trash-agent.qmd shelf-comic-margins.qmd; do
        qm_out="$qm_out|mark:$qm_q=$(qmd_mark "$qm_q")"
    done
    echo "${qm_out#|}"
}

# ═════════════════════════════ 设备端采集 ═════════════════════════════
collect() {
    # shellcheck disable=SC2046  # unit_specs 每行一个无空格的 KIND:UNIT:PAYLOAD，有意按词展开成多个参数
    dev_script /usr/bin/xochitl "$FLN" "$(qmd_marks_spec)" $(unit_specs) <<'DEVICE_SCRIPT'
# 只读采集。绝不 start/stop/restart、mount、写文件（连临时文件都不建）。busybox：无 head -N / pkill / curl。
set -u
XBIN="$1"; FLN="$2"; MARKS="$3"; shift 3
T="$(printf '\t')"; CR="$(printf '\r')"; NL="
"
emit() { # KEY V…：一行 KEY<TAB>V…；值里的 TAB/换行压成空格，空值写 "-"
    em_l="$1"; shift
    for em_v in "$@"; do
        case "$em_v" in *"$T"*|*"$NL"*|*"$CR"*) em_v="$(printf '%s' "$em_v" | tr '\t\n\r' '   ')" ;; esac   # 少 fork：多数值不含
        [ -n "$em_v" ] || em_v="-"
        em_l="$em_l$T$em_v"
    done
    printf '%s\n' "$em_l"
}
yn() { if "$@"; then echo 1; else echo 0; fi; }
sprop() { systemctl show "$1" -p "$2" --value 2>/dev/null; }

emit VERSION 1
emit NOW "$(date +%s)"
emit UPTIME "$(cut -d' ' -f1 "$CJ_PROC/uptime" 2>/dev/null)"
emit FW_SHA "$(sha256sum "$XBIN" 2>/dev/null | awk '{print $1}')"
emit FW_VER "$(sed -n 's/^IMG_VERSION="\{0,1\}\([^"]*\)"\{0,1\}$/\1/p' /usr/lib/os-release /etc/os-release 2>/dev/null | head -n 1)"

# ── xochitl ──
XP="$(cj_xochitl_pid)"
emit X_ACTIVE "$(systemctl is-active xochitl 2>/dev/null)"
emit X_PID "$XP"
emit X_NRESTARTS "$(sprop xochitl NRestarts)"
emit X_START_MONO "$(sprop xochitl ExecMainStartTimestampMonotonic)"
XM="$CJ_PROC/$XP/maps"
if [ "$XP" != "0" ] && [ -r "$XM" ]; then
    emit X_XOVI "$(yn grep -q 'xovi\.so' "$XM")"
    # 映射了哪些 extensions.d 下的文件、各几段、有没有 "(deleted)"（= 文件被换掉了但进程还用着旧的）
    awk -v T="$T" '{ for (i = 1; i <= NF; i++) if (index($i, "/extensions.d/")) { p = $i; n[p]++; if ($NF == "(deleted)") d[p] = 1 } }
        END { for (p in n) print "XMAP" T p T n[p] T (d[p] ? 1 : 0) }' "$XM" 2>/dev/null
else
    emit X_XOVI "?"
fi
emit HAS_XOVI "$(yn [ -e "$CJ_XOVI/xovi.so" ])"
EXT="$CJ_XOVI/extensions.d"
HOOKSPEC=""
if [ -d "$EXT" ]; then
    for f in "$EXT"/* "$EXT"/.[!.]*; do
        [ -f "$f" ] || continue
        n="$(basename "$f")"
        emit EXT_FILE "$n" "$(stat -c %Y "$f" 2>/dev/null)"
        case "$n" in *.so) e="${n%.so}"; HOOKSPEC="$HOOKSPEC|hookok:$e=[$e]&安装完成|hookfail:$e=[$e]&hook 未安装" ;; esac
    done
fi
cj_pending_list | while IFS= read -r p; do emit PENDING "$p"; done

# ── qt-resource-rebuilder 目录下的 qmd/rcc ──
Q="$CJ_XOVI/exthome/qt-resource-rebuilder"
if [ -d "$Q" ]; then
    emit HAS_QRR 1
    for f in "$Q"/*; do [ -f "$f" ] && emit QRR_FILE "$(basename "$f")" "$(stat -c %Y "$f" 2>/dev/null)"; done
else
    emit HAS_QRR 0
fi
emit HAS_APPLOAD "$(yn [ -d "$CJ_XOVI/exthome/appload" ])"

# ── systemd 单元 ──
for spec in "$@"; do
    kind="${spec%%:*}"; rest="${spec#*:}"; unit="${rest%%:*}"; payload="${rest#*:}"
    present="$(yn [ -f "$CJ_SYSD/$unit" ])"
    if [ "$payload" = "-" ]; then pl="-"; else pl="$(yn [ -e "$payload" ])"; fi
    act="$(systemctl is-active "$unit" 2>/dev/null)"
    nr=""; pid=""; rss=""; hwm=""; sm=""; port=""
    case "$kind" in svc|opt)
        nr="$(sprop "$unit" NRestarts)"; pid="$(sprop "$unit" MainPID)"; sm="$(sprop "$unit" ExecMainStartTimestampMonotonic)"
        if [ -n "$pid" ] && [ "$pid" != "0" ] && [ -r "$CJ_PROC/$pid/status" ]; then
            rss="$(awk '/^VmRSS:/ { print $2 }' "$CJ_PROC/$pid/status")"
            hwm="$(awk '/^VmHWM:/ { print $2 }' "$CJ_PROC/$pid/status")"
        fi
        [ "$present" = 1 ] && port="$(sed -n 's/.*--bind [^ ]*:\([0-9][0-9]*\).*/\1/p' "$CJ_SYSD/$unit" | head -n 1)" ;;
    esac
    emit UNIT "$kind" "$unit" "$present" "$pl" "$act" "$nr" "$pid" "$rss" "$hwm" "$sm" "$port"
done

# ── 端口监听（/proc/net/tcp{,6}；busybox 的 netstat/ss 参数不一，不依赖它们）──
TCPF=""
for f in "$CJ_PROC/net/tcp" "$CJ_PROC/net/tcp6"; do [ -r "$f" ] && TCPF="$TCPF $f"; done
if [ -n "$TCPF" ]; then
    emit PORTINFO 1
    # shellcheck disable=SC2086
    awk -v T="$T" '
        function h2d(h,  i, v) { v = 0; h = toupper(h); for (i = 1; i <= length(h); i++) v = v * 16 + index("0123456789ABCDEF", substr(h, i, 1)) - 1; return v }
        FNR > 1 && $4 == "0A" {
            split($2, a, ":"); ad = a[1]
            if (length(ad) == 8) addr = h2d(substr(ad, 7, 2)) "." h2d(substr(ad, 5, 2)) "." h2d(substr(ad, 3, 2)) "." h2d(substr(ad, 1, 2))
            else if (ad ~ /^0+$/) addr = "::"
            else if (ad == "00000000000000000000000001000000") addr = "::1"
            else if (ad ~ /^0000000000000000FFFF0000/) { v4 = substr(ad, 25, 8); addr = h2d(substr(v4, 7, 2)) "." h2d(substr(v4, 5, 2)) "." h2d(substr(v4, 3, 2)) "." h2d(substr(v4, 1, 2)) }
            else addr = "v6"
            print "LISTEN" T h2d(a[2]) T addr
        }' $TCPF
else
    emit PORTINFO 0
fi

# ── 日志：本次开机以来的关键告警（一遍扫完）+ 当前 xochitl 进程的加载标记 ──
if command -v journalctl >/dev/null 2>&1; then
    emit JOURNAL 1
    journalctl -b -o cat --no-pager -q 2>/dev/null | awk -v T="$T" -v P="J" '
        function hit(k,  x) { c[k]++; x = $0; gsub(/\t/, " ", x); s[k] = substr(x, 1, 400) }
        index($0, "Kernel command line") { next }   # 内核命令行里的 panic=2 之类不是告警（2026-09-25 真机误报）
        /panicked/ || index($0, "Kernel panic") { hit("panic") }
        index($0, "SHELF-MKDIR: transfer timeout") { hit("mkdir-timeout") }
        index($0, "hook 未安装") { hit("hook-missing") }
        index($0, "processed more than once") { hit("dup-ext") }
        /Out of memory|oom-kill|invoked oom-killer|killed by the OOM killer/ { hit("oom") }
        END { for (k in c) print "ALERT" T P T k T c[k] T s[k] }'
    if [ "$XP" != "0" ]; then
        journalctl -b "_PID=$XP" -o cat --no-pager -q 2>/dev/null | awk -v T="$T" -v SPEC="$MARKS$HOOKSPEC" '
            BEGIN { n = split(SPEC, sp, "|"); for (i = 1; i <= n; i++) if (sp[i] != "") { e = index(sp[i], "="); key[i] = substr(sp[i], 1, e - 1); pat[i] = substr(sp[i], e + 1); c[key[i]] = 0 } }
            { for (i = 1; i <= n; i++) if (i in key) { m = split(pat[i], ps, "&"); ok = 1; for (j = 1; j <= m; j++) if (!index($0, ps[j])) { ok = 0; break }; if (ok) c[key[i]]++ } }
            END { for (k in c) print "XLOG" T k T c[k] }'
    fi
else
    emit JOURNAL 0
fi
if command -v dmesg >/dev/null 2>&1; then
    dmesg 2>/dev/null | awk -v T="$T" -v P="K" '
        function hit(k,  x) { c[k]++; x = $0; gsub(/\t/, " ", x); s[k] = substr(x, 1, 400) }
        index($0, "Kernel command line") { next }
        /panicked/ || index($0, "Kernel panic") { hit("panic") }
        /Out of memory|oom-kill|invoked oom-killer/ { hit("oom") }
        END { for (k in c) print "ALERT" T P T k T c[k] T s[k] }'
fi


emit DF_HOME "$(df -kP "$CJ_HOME" 2>/dev/null | awk 'END { print $4 }')"
emit END 1
DEVICE_SCRIPT
}

# ═════════════════════════════ host 端判定（只读 $DUMP，不连设备）═════════════════════════════
dget() { awk -F'\t' -v k="$1" '$1 == k { print $2; exit }' "$DUMP"; }
drows() { awk -F'\t' -v k="$1" '$1 == k' "$DUMP"; }
num() { case "$1" in ''|-|*[!0-9]*) return 1 ;; *) return 0 ;; esac; }
# item LEVEL(ok|warn|fail) SECTION TITLE MSG [DETAIL（多行用 $US 分隔）]
item() {
    printf '%s\t%s\t%s\t%s\t%s\n' "$1" "$2" "$3" "$(printf '%s' "$4" | tr '\t\n' '  ')" "$(printf '%s' "${5:-}" | tr '\t\n' '  ')" >> "$ITEMS"
}
fmt_dur() { awk -v s="$1" 'BEGIN { s = int(s); d = int(s / 86400); h = int(s % 86400 / 3600); m = int(s % 3600 / 60);
    if (d) printf "%d天%d小时", d, h; else if (h) printf "%d小时%d分", h, m; else if (m) printf "%d分%d秒", m, s % 60; else printf "%d秒", s }'; }
fmt_mb() { awk -v k="$1" 'BEGIN { printf "%.1fMB", k / 1024 }'; }
dash() { [ "$1" = "-" ] && echo "" || echo "$1"; }

judge_fw() {
    S="1. 固件与开机"
    up="$(dget UPTIME)"; up="${up%%.*}"
    if num "$up"; then
        if [ "$up" -lt "$CJ_VERIFY_UPTIME_WARN" ]; then
            item warn "$S" "开机时长" "$(fmt_dur "$up")——刚开机不久；若不是你重启的，看下面飞行记录仪与告警是否有意外整机重启"
        else
            item ok "$S" "开机时长" "$(fmt_dur "$up")"
        fi
    else
        item warn "$S" "开机时长" "读不到 /proc/uptime"
    fi
    sha="$(dash "$(dget FW_SHA)")"; ver="$(dash "$(dget FW_VER)")"; ver="${ver:-未知}"
    if [ -z "$sha" ]; then
        item fail "$S" "固件" "读不到 /usr/bin/xochitl 的 sha256（IMG_VERSION=$ver）"
        return
    fi
    line=""; [ -f "$CJ_ALLOWLIST" ] && line="$(grep "^${sha}[[:space:]]" "$CJ_ALLOWLIST" | head -n 1 || true)"
    if [ -n "$line" ]; then
        item ok "$S" "固件" "IMG_VERSION=$ver，sha256 命中白名单：$(echo "$line" | awk '{$1=""; sub(/^ /, ""); print}')"
        return
    fi
    [ -f "$CJ_ALLOWLIST_LOCAL" ] && line="$(grep "^${sha}[[:space:]]" "$CJ_ALLOWLIST_LOCAL" | head -n 1 || true)"
    if [ -n "$line" ]; then
        item warn "$S" "固件" "IMG_VERSION=$ver，sha256 只在本机 firmware-allowlist.local.txt（--force 追加，未验证）"
    else
        item fail "$S" "固件" "IMG_VERSION=$ver，sha256=$sha 不在白名单——固件变了（OTA？），注入定位没验证过"
    fi
}

judge_xochitl() {
    S="2. xochitl 与 xovi 扩展"
    st="$(dget X_ACTIVE)"; pid="$(dget X_PID)"; nr="$(dget X_NRESTARTS)"
    if [ "$st" != "active" ]; then
        item fail "$S" "xochitl" "is-active=$st（期望 active），MainPID=$pid"
    elif num "$nr" && [ "$nr" -gt 0 ]; then
        item warn "$S" "xochitl" "active，MainPID=$pid，NRestarts=$nr（>0：本次开机里崩过、被 systemd 拉起过）"
    else
        item ok "$S" "xochitl" "active，MainPID=$pid，NRestarts=${nr}"
    fi
    xv="$(dget X_XOVI)"; hx="$(dget HAS_XOVI)"
    if [ "$xv" = 1 ]; then
        item ok "$S" "xovi" "已在运行中的 xochitl 里生效（maps 含 xovi.so）"
    elif [ "$hx" != 1 ]; then
        item warn "$S" "xovi" "设备上没有 xovi.so（没 vellum add xovi）——扩展与界面补丁都不会生效"
    elif [ "$xv" = 0 ]; then
        item fail "$S" "xovi" "xochitl 没带 xovi（maps 无 xovi.so）——重启后没恢复？看 xovi-reenable.service；恢复用 sh deploy-xovi-apply.sh"
    else
        item fail "$S" "xovi" "读不到 xochitl 进程的 maps（MainPID=$pid）"
    fi

    # 扩展：extensions.d 里的每个文件 × maps 里映射的路径
    drows EXT_FILE | while IFS="$TAB" read -r _k name _mt; do
        case "$name" in
            *.so.crashed) item warn "$S" "扩展 $name" "有 xovi 崩溃标记——对应扩展上次加载时崩过" ; continue ;;
            *.so) ;;
            *) item fail "$S" "扩展目录异物 $name" "extensions.d 里不是 .so 的文件——xovi 会把目录下任意文件当扩展加载，同名重复注册致 xochitl 崩溃循环（备份绝不能放这里）"; continue ;;
        esac
        m="$(awk -F'\t' -v n="/extensions.d/$name" '$1 == "XMAP" && substr($2, length($2) - length(n) + 1) == n { print $3 "\t" $4; exit }' "$DUMP")"
        e="${name%.so}"
        ok_n="$(awk -F'\t' -v k="hookok:$e" '$1 == "XLOG" && $2 == k { print $3; exit }' "$DUMP")"
        bad_n="$(awk -F'\t' -v k="hookfail:$e" '$1 == "XLOG" && $2 == k { print $3; exit }' "$DUMP")"
        hooks=""; num "${ok_n:-x}" && [ "$ok_n" -gt 0 ] && hooks="，日志 hook「安装完成」×$ok_n"
        if [ -z "$m" ]; then
            if [ "$xv" = 1 ]; then
                item warn "$S" "扩展 $name" "在 extensions.d 里但没被当前 xochitl 映射（_xovi_shouldLoad 拒绝了，或装在本次 xochitl 启动之后）"
            else
                item warn "$S" "扩展 $name" "在 extensions.d 里；xovi 未生效所以没加载"
            fi
        elif [ "$(echo "$m" | cut -f2)" = 1 ]; then
            item fail "$S" "扩展 $name" "maps 里是 (deleted)：文件换了但 xochitl 还用着旧的。别 restart xochitl（换过已映射的 .so 再 restart 会 SEGV→整机重启），整机重启让它生效"
        elif num "${bad_n:-x}" && [ "$bad_n" -gt 0 ]; then
            item fail "$S" "扩展 $name" "已映射 $(echo "$m" | cut -f1) 段，但当前 xochitl 日志有「hook 未安装」×$bad_n（特征码没命中，功能不生效）"
        else
            item ok "$S" "扩展 $name" "已映射 $(echo "$m" | cut -f1) 段$hooks"
        fi
    done
    # 映射着、但目录里已经没有这个文件（卸了没重启）
    drows XMAP | while IFS="$TAB" read -r _k path _n _d; do
        b="$(basename "$path")"
        awk -F'\t' -v n="$b" '$1 == "EXT_FILE" && $2 == n { f = 1 } END { exit !f }' "$DUMP" \
            || item warn "$S" "扩展 $b" "当前 xochitl 还映射着，但 extensions.d 里已没有这个文件（卸载后没重启，下次重启 xochitl 才停用）"
    done

    so="$(drows PENDING | cut -f2 | grep '^so-pending:' | sed 's/^so-pending://' | tr '\n' ' ' | sed 's/ $//' || true)"
    mk="$(drows PENDING | cut -f2 | grep -v '^so-pending:' | tr '\n' ' ' | sed 's/ $//' || true)"
    if [ -n "$so" ]; then
        item warn "$S" "待换入区 so-pending" "有 $so 等着换入——下次 sh deploy-xovi-apply.sh 会 stop→换入→start"
    else
        item ok "$S" "待换入区 so-pending" "空"
    fi
    [ -z "$mk" ] || item warn "$S" "待生效标记" "$mk——落盘了还没重启 xochitl 生效（sh deploy-xovi-apply.sh）"
}

judge_qmd() {
    S="3. 界面补丁（qmd）"
    if [ "$(dget HAS_QRR)" != 1 ]; then
        item warn "$S" "qt-resource-rebuilder" "设备上没有 exthome/qt-resource-rebuilder——所有 qmd 都被安装脚本跳过（vellum add qt-resource-rebuilder）"
        return
    fi
    now="$(dget NOW)"; up="$(dget UPTIME)"; up="${up%%.*}"; xs="$(dget X_START_MONO)"
    xstart=""   # 当前 xochitl 进程启动的 epoch 秒（= 现在 − 开机时长 + 启动时的单调时钟）
    if num "$now" && num "$up" && num "$xs"; then xstart=$((now - up + xs / 1000000)); fi
    expect=""
    for s in $SHELF_ALL; do
        b="$(shelf_svc_of "$s")"
        # 这个服务装了（二进制在）才期望它的 qmd
        awk -F'\t' -v u="$b.service" '$1 == "UNIT" && $3 == u && $5 == "1" { f = 1 } END { exit !f }' "$DUMP" || continue
        expect="$expect $(shelf_svc_qmds "$s")"
    done
    [ "$(dget HAS_APPLOAD)" = 1 ] && expect="$expect koreader-sidebar-entry.qmd cangjie-icons.rcc"
    for q in $expect; do
        mt="$(awk -F'\t' -v n="$q" '$1 == "QRR_FILE" && $2 == n { print $3; exit }' "$DUMP")"
        mark="$(qmd_mark "$q")"; note=""
        if [ -n "$mark" ]; then
            mc="$(awk -F'\t' -v k="mark:$q" '$1 == "XLOG" && $2 == k { print $3; exit }' "$DUMP")"
            if num "${mc:-x}" && [ "$mc" -gt 0 ]; then note="；当前 xochitl 日志见「$mark」×$mc"
            else note="；当前 xochitl 日志未见「$mark」（要打开相关界面才打印，不算异常）"; fi
        fi
        if [ -z "$mt" ]; then
            item fail "$S" "$q" "缺失（所属服务已装、qt-resource-rebuilder 在）——重跑对应部署（shelf：sh deploy.sh；侧栏：sh deploy-sidebar-entry.sh）"
        elif [ -n "$xstart" ] && num "$mt" && [ "$mt" -gt $((xstart + 2)) ]; then
            item warn "$S" "$q" "文件比当前 xochitl 进程新——待重启生效（sh deploy-xovi-apply.sh）$note"
        else
            item ok "$S" "$q" "在$note"
        fi
    done
    for q in $SHELF_LEGACY_QMDS; do
        awk -F'\t' -v n="$q" '$1 == "QRR_FILE" && $2 == n { f = 1 } END { exit !f }' "$DUMP" \
            && item warn "$S" "$q" "旧命名遗留（重跑 sh deploy.sh 会清掉）"
    done
    [ -n "$expect" ] || item warn "$S" "qmd" "没有期望的 qmd（书架服务都没装？）"
}

judge_services() {
    S="4. 常驻服务"
    drows UNIT | while IFS="$TAB" read -r _k kind unit present _pl act nr pid rss hwm sm _port; do
        case "$kind" in svc|opt) ;; *) continue ;; esac
        [ "$present" = 1 ] || continue   # 单元缺失在第 8 节报
        mem=""; num "$rss" && mem="，RSS $(fmt_mb "$rss")"; num "$hwm" && mem="$mem / 峰值 $(fmt_mb "$hwm")"
        started=""
        if num "$sm" && [ "$sm" -gt 0 ]; then
            started="，开机后 $(awk -v u="$sm" 'BEGIN { printf "%.1f", u / 1000000 }')s 启动"
            [ $((sm / 1000000)) -gt 300 ] && started="，开机后 $(fmt_dur $((sm / 1000000))) 才启动（开机后被重启过）"
        fi
        info="PID $(dash "$pid")，NRestarts $(dash "$nr")$mem$started"
        if [ "$kind" = opt ]; then
            item ok "$S" "$unit" "$act（有意不开机自启，active/inactive 都正常）；$info"
            continue
        fi
        if [ "$act" != "active" ]; then
            item fail "$S" "$unit" "is-active=$act（期望 active）——journalctl -u $unit -b 看原因"
        elif num "$nr" && [ "$nr" -gt 0 ]; then
            item warn "$S" "$unit" "active，但 NRestarts=$nr（崩过、被 systemd 拉起过）；$info"
        elif num "$hwm" && [ "$hwm" -gt "$CJ_VERIFY_HWM_WARN_KB" ]; then
            item warn "$S" "$unit" "active，峰值内存偏高（> $(fmt_mb "$CJ_VERIFY_HWM_WARN_KB")）；$info"
        else
            item ok "$S" "$unit" "active；$info"
        fi
    done
    awk -F'\t' '$1 == "UNIT" && ($2 == "svc") && $4 == "1" { f = 1 } END { exit !f }' "$DUMP" \
        || item warn "$S" "常驻服务" "一个常驻服务单元都没有（没装书架？或 OTA 冲掉了——见第 8 节）"
}

judge_alerts() {
    S="5. 本次开机以来的关键告警"
    if [ "$(dget JOURNAL)" != 1 ]; then item warn "$S" "journal" "设备上没有 journalctl，只看了 dmesg"; fi
    for key in panic oom hook-missing dup-ext mkdir-timeout; do
        # journal 与 dmesg 可能同一事件各一份：取较大计数，样例优先 journal
        r="$(awk -F'\t' -v k="$key" '$1 == "ALERT" && $3 == k { if ($4 + 0 > c) c = $4 + 0; if (s == "" || $2 == "J") s = $5 } END { if (c) print c "\t" s }' "$DUMP")"
        case "$key" in
            panic) t="panic"; lvl=fail; zero="无 panic（已排除内核命令行里的 panic=N 参数）" ;;
            oom) t="OOM"; lvl=fail; zero="无 Out of memory / oom-kill" ;;
            hook-missing) t="hook 未安装"; lvl=fail; zero="无「hook 未安装」" ;;
            dup-ext) t="扩展重复注册"; lvl=fail; zero="无「processed more than once」" ;;
            mkdir-timeout) t="SHELF-MKDIR 超时"; lvl=warn; zero="无「SHELF-MKDIR: transfer timeout」" ;;
        esac
        if [ -n "$r" ]; then
            item "$lvl" "$S" "$t" "$(echo "$r" | cut -f1) 条相关日志；最近一条：$(echo "$r" | cut -f2-)"
        else
            item ok "$S" "$t" "$zero"
        fi
    done
}

judge_flight() {
    S="6. 飞行记录仪"
    if [ "$(dget FLIGHT_ABSENT)" = 1 ]; then
        item ok "$S" "flight.log" "本机没有 ~/.local/state/cang-jie-flight/flight.log（飞行记录仪跑在宿主机上，不由本仓库安装；没开就没有）"
        return
    fi
    mt="$(dget FLIGHT_MTIME)"; now="$(dget NOW)"; age=""
    num "$mt" && num "$now" && age="，最后写入于 $(fmt_dur $((now - mt)))前"
    lines="$(drows FLIGHT | cut -f2- | tr '\n' "$US" | sed "s/$US\$//")"
    item ok "$S" "flight.log" "最后 $(drows FLIGHT | wc -l | tr -d ' ') 行$age" "$lines"
}

judge_ports() {
    S="7. 端口监听"
    if [ "$(dget PORTINFO)" != 1 ]; then item warn "$S" "端口" "读不到 /proc/net/tcp，跳过"; return; fi
    drows UNIT | while IFS="$TAB" read -r _k kind unit present pl _act _nr _pid _rss _hwm _sm port; do
        [ "$kind" = svc ] || continue
        b="${unit%.service}"
        port="$(dash "$port")"; [ -n "$port" ] || port="$(svc_port "$b")"
        [ -n "$port" ] || continue
        [ "$pl" = 1 ] && [ "$present" = 1 ] || continue   # 没装这个服务（或单元缺失，第 8 节报）就不期望它的端口
        addrs="$(awk -F'\t' -v p="$port" '$1 == "LISTEN" && $2 == p { print $3 }' "$DUMP" | sort -u | tr '\n' ' ' | sed 's/ $//')"
        if [ -z "$addrs" ]; then
            item fail "$S" "$port（$b）" "没有进程在监听"
        elif [ "$b" = gateway ]; then
            case " $addrs " in
                *" 0.0.0.0 "*|*" :: "*) item ok "$S" "$port（$b）" "监听 $addrs" ;;
                *) item warn "$S" "$port（$b）" "只监听 $addrs——网关应对外（0.0.0.0），USB/WiFi 上可能访问不到" ;;
            esac
        else
            case " $addrs " in
                *" 0.0.0.0 "*|*" :: "*) item warn "$S" "$port（$b）" "监听 $addrs——领域服务应只听 127.0.0.1（经网关转发）" ;;
                *) item ok "$S" "$port（$b）" "监听 $addrs" ;;
            esac
        fi
    done
}

judge_units() {
    S="8. /usr 下的 systemd 单元（OTA 会冲掉）"
    total=0; okc=0
    for spec in $(drows UNIT | awk -F'\t' '{ print $3 ":" $4 ":" $5 }'); do
        total=$((total + 1))
        unit="${spec%%:*}"; r="${spec#*:}"; present="${r%%:*}"; pl="${r#*:}"
        if [ "$present" = 1 ]; then okc=$((okc + 1)); continue; fi
        if [ "$pl" = 1 ]; then
            item fail "$S" "$unit" "单元文件不在 /usr/lib/systemd/system，但它的载荷还在 /home——OTA 冲掉了？重跑 sh install-all.sh"
        elif [ "$pl" = "-" ]; then
            item warn "$S" "$unit" "没装（装时 --skip 过可忽略；OTA 冲掉的话重跑 sh install-all.sh）"
        else
            item warn "$S" "$unit" "没装（单元与载荷都不在；装时 --only/--skip 过可忽略）"
        fi
    done
    [ "$total" -gt 0 ] || { item fail "$S" "单元" "采集里没有单元信息"; return; }
    [ "$okc" -eq 0 ] || item ok "$S" "单元" "$okc/$total 个在位"
}

judge_disk() {
    S="9. 磁盘"
    fr="$(dget DF_HOME)"
    if ! num "$fr"; then item warn "$S" "/home" "读不到剩余空间"; return; fi
    if [ "$fr" -lt "$CJ_MIN_FREE_KB" ]; then
        item fail "$S" "/home" "剩余 $(fmt_mb "$fr")（< $(fmt_mb "$CJ_MIN_FREE_KB")，连重装都放不下）"
    elif [ "$fr" -lt "$CJ_WARN_FREE_KB" ]; then
        item warn "$S" "/home" "剩余 $(fmt_mb "$fr")（< $(fmt_mb "$CJ_WARN_FREE_KB")，偏紧）"
    else
        item ok "$S" "/home" "剩余 $(fmt_mb "$fr")"
    fi
}

judge_all() {
    if [ "$(dget VERSION)" != 1 ]; then
        item fail "0. 采集" "采集结果" "不是可识别的采集文本（缺 VERSION 行）"
        return
    fi
    [ "$(dget END)" = 1 ] || item fail "0. 采集" "采集结果" "不完整（缺 END 行：ssh 中途断了或设备端脚本出错）——以下结论可能缺项"
    judge_fw; judge_xochitl; judge_qmd; judge_services; judge_alerts; judge_flight; judge_ports; judge_units; judge_disk
}

render_text() {
    awk -F'\t' -v US="$US" '
        { if ($2 != sec) { sec = $2; print ""; print "═══ " sec " ═══" }
          sym = ($1 == "ok") ? "✓" : ($1 == "warn") ? "⚠" : "✗"
          print "  " sym " " $3 "：" $4
          if ($5 != "") { n = split($5, d, US); for (i = 1; i <= n; i++) print "      │ " d[i] } }' "$ITEMS"
}
render_json() { # $1=host
    awk -F'\t' -v US="$US" -v host="$1" '
        function esc(s) { gsub(/\\/, "\\\\", s); gsub(/"/, "\\\"", s); gsub(/[\001-\037]/, " ", s); return s }
        BEGIN { printf "{\"host\":\"%s\",\"items\":[", esc(host) }
        { if (NR > 1) printf ","; c[$1]++
          printf "{\"level\":\"%s\",\"section\":\"%s\",\"title\":\"%s\",\"msg\":\"%s\",\"detail\":[", $1, esc($2), esc($3), esc($4)
          if ($5 != "") { n = split($5, d, US); for (i = 1; i <= n; i++) printf "%s\"%s\"", (i > 1 ? "," : ""), esc(d[i]) }
          printf "]}" }
        END { r = c["fail"] ? "FAIL" : (c["warn"] ? "WARN" : "PASS")
              printf "],\"ok\":%d,\"warn\":%d,\"fail\":%d,\"result\":\"%s\"}\n", c["ok"], c["warn"], c["fail"], r }' "$ITEMS"
}

# 飞行记录仪在**宿主机**上（`~/.local/state/cang-jie-flight/flight.log`：宿主机循环 ssh 设备流式抓 journal +
# 每 2 秒一行负载/内存），不在设备上——所以在本机读，拼进同一份采集文本，--from 离线重判照样有。
collect_host_flight() {
    fl="${CJ_FLIGHT_LOG:-$HOME/.local/state/cang-jie-flight/flight.log}"
    if [ -f "$fl" ]; then
        printf 'FLIGHT_MTIME\t%s\n' "$(stat -c %Y "$fl" 2>/dev/null)"
        tail -n "$FLN" "$fl" 2>/dev/null | tr '\t' ' ' | while IFS= read -r l; do printf 'FLIGHT\t%s\n' "$l"; done
    else
        printf 'FLIGHT_ABSENT\t1\n'
    fi
}

# ═════════════════════════════ 主流程 ═════════════════════════════
WORK="$(mktemp -d)"   # 本机临时目录，只放下面三个文件；收尾逐个删再 rmdir（不递归删目录）
DUMP="$WORK/dump.txt"; ITEMS="$WORK/items.txt"; : > "$ITEMS"
trap 'rm -f "$DUMP" "$ITEMS" "$WORK/err.txt"; rmdir "$WORK" 2>/dev/null || true' EXIT
if [ -n "$FROM" ]; then
    cp "$FROM" "$DUMP"
    dh="$(awk -F'\t' '$1 == "HOST" { print $2; exit }' "$DUMP")"   # 采集时记下的真实主机；旧 dump 没有就用参数
    LABEL="${dh:-$HOST}（离线：$(basename "$FROM")）"
else
    require_device
    collect > "$DUMP" 2>"$WORK/err.txt" || true   # 采集脚本自身出错也继续判定（缺 END 行会被判 ✗）
    collect_host_flight >> "$DUMP"
    printf 'HOST\t%s\n' "$HOST" >> "$DUMP"
    LABEL="$HOST"
fi
if [ "$DUMP_ONLY" = 1 ]; then cat "$DUMP"; exit 0; fi

judge_all
okn="$(grep -c '^ok' "$ITEMS" || true)"; wn="$(grep -c '^warn' "$ITEMS" || true)"; fn="$(grep -c '^fail' "$ITEMS" || true)"
if [ "$fn" -gt 0 ]; then RES=FAIL; elif [ "$wn" -gt 0 ]; then RES=WARN; else RES=PASS; fi
if [ "$JSON" = 1 ]; then
    render_json "$LABEL"
else
    echo "═══ 设备健康核对（root@$LABEL，只读）═══"
    render_text
    if [ -s "$WORK/err.txt" ] && [ -z "$FROM" ]; then
        echo; echo "-- 设备端采集的 stderr（前 10 行）："; sed -n '1,10p' "$WORK/err.txt" | sed 's/^/     /'
    fi
    echo
    echo "═══ 汇总：✓ $okn   ⚠ $wn   ✗ $fn ═══"
    echo "VERIFY-SUMMARY host=$HOST ok=$okn warn=$wn fail=$fn result=$RES"
fi
[ "$fn" -eq 0 ]
