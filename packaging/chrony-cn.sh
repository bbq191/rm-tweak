#!/bin/sh
# ═══════════════════════════════════════════════════════════════════════════
# chrony-cn.sh —— reMarkable 设备时钟同步修复：把 chrony 默认的 time{1-4}.google.com（国内不通，
# 设备从未同步过）换成国内可达的 NTP，写进 **rootfs 底层 /etc/chrony.conf**（普通重启不丢；
# OTA 冲掉 rootfs 后重跑本脚本即可）。
#
# 用法（root，设备上跑）：  ssh root@10.11.99.1 sh -s < packaging/chrony-cn.sh
#   （host 侧也可以用 packaging/deploy-chrony-cn.sh <host> 跑这条命令，供 install-all.sh 编排调用）
# 幂等：底层已是国内配置就不再写 rootfs；overlay 当前视图与底层不一致才额外拷贝一份让本次开机
# 立即生效；chronyd 只在有改动或未同步时才重启。
#
# 【为什么这么绕】/etc 是 overlay（lower=rootfs /etc 只读，upper=/var/volatile tmpfs）：直接写
#   当前看到的 /etc 重启即丢。改底层要：
#   ① 先 `mount -o remount,rw /` 再 `mount --bind /`（bind 继承绑定时刻的 ro 标志，顺序反了
#      拿到的还是只读视图）；
#   ② overlay 缓存 lower，改完底层当场从 /etc 读到的仍是旧内容——要立即生效就再拷一份进 overlay
#      当前视图（落 upper tmpfs，下次重启自然消失、届时 upper 为空、底层新值接管）；
#   ③ `remount,ro /` 偶发 busy，重试几次即可。
# 设备没有 chronyc、busybox 没有 ntpd，验收看 timedatectl + journal。
# ═══════════════════════════════════════════════════════════════════════════
set -u

SERVERS="ntp.aliyun.com ntp.tencent.com cn.pool.ntp.org time.cloudflare.com"
# 下面几个路径只为本机模拟测试（packaging/tests）可覆盖，设备上一律用默认值
CONF="${CJ_CHRONY_CONF:-/etc/chrony.conf}"
BK_DIR="${CJ_BACKUP_DIR:-/home/root/cangjie-backups}"
MOUNTS="${CJ_MOUNTS:-/proc/mounts}"
TMPD="${CJ_TMPDIR:-/tmp}"
BIND="$TMPD/chrony-cn.rootbind"

[ "$(id -u)" = "0" ] || { echo "!! 需 root"; exit 1; }
[ -f "$CONF" ] || { echo "!! 没有 $CONF"; exit 1; }

is_cn() { # $1=文件：不含 google 且含全部 4 个国内服务器
    ! grep -q "^server .*google" "$1" && for s in $SERVERS; do grep -q "^server $s " "$1" || return 1; done
}

rewrite() { # $1=源 $2=目标：第一条 server 行处换成 4 条国内，其余 server 行删掉；源里一条 server 都没有就追加到末尾
    awk -v servers="$SERVERS" '
        BEGIN { n = split(servers, S, " ") }
        /^server / { if (!done) { for (i = 1; i <= n; i++) print "server " S[i] " iburst minpoll 7"; done = 1 }; next }
        { print }
        END { if (!done) for (i = 1; i <= n; i++) print "server " S[i] " iburst minpoll 7" }' "$1" > "$2"
}

changed=0
RW_OPEN=0
if grep -q " /etc overlay " "$MOUNTS"; then
    # ── overlay：改 rootfs 底层 ──
    if dmsetup ls --target verity 2>/dev/null | grep -q .; then
        echo "✋ dm-verity 激活，rootfs 不可写：只改本次开机的 overlay 视图（重启会丢）"
    else
        mount -o remount,rw / || { echo "!! remount rw / 失败"; exit 1; }
        # 2026-09-20：rw 窗口内被 kill/ssh 断开时也要恢复 ro（先卸 bind，否则 remount ro 会 busy）
        RW_OPEN=1
        trap 'if [ "$RW_OPEN" = "1" ]; then umount "$BIND" 2>/dev/null; mount -o remount,ro / 2>/dev/null; fi' EXIT
        trap 'exit 143' INT TERM HUP PIPE   # PIPE：ssh 断开后写输出会收到它，不接住就不走 EXIT trap、rootfs 留在 rw
        mkdir -p "$BIND" && mount --bind / "$BIND" || { mount -o remount,ro / 2>/dev/null; echo "!! bind / 失败"; exit 1; }
        LOWER="$BIND/etc/chrony.conf"
        if is_cn "$LOWER"; then
            echo "-- rootfs 底层已是国内 NTP，跳过"
        else
            mkdir -p "$BK_DIR"
            cp "$LOWER" "$BK_DIR/chrony.conf.bak.$(date +%Y%m%d-%H%M%S)" || { echo "!! 备份 chrony.conf 失败，不改底层"; exit 1; }
            # 失败要如实报错退出（EXIT trap 卸 bind、恢复 ro）——旧版不看返回值，写失败也打印"已改"（2026-09-25 审计）
            if ! { rewrite "$LOWER" "$LOWER.new" && mv "$LOWER.new" "$LOWER" && sync; }; then
                rm -f "$LOWER.new"; echo "!! 改 rootfs 底层 chrony.conf 失败（底层未动）"; exit 1
            fi
            echo "-- rootfs 底层已改（备份在 $BK_DIR）"
            changed=1
        fi
        cp "$LOWER" "$TMPD/chrony-cn.lower"
        umount "$BIND"; rmdir "$BIND" 2>/dev/null
        i=0
        while ! mount -o remount,ro / 2>/dev/null; do
            i=$((i + 1)); [ "$i" -ge 5 ] && { echo "⚠ remount ro / 一直 busy，rootfs 暂留 rw（重启恢复 ro）"; break; }
            sleep 2
        done
        [ "$i" -lt 5 ] && RW_OPEN=0
        # overlay 视图与底层不一致（overlay 缓存）→ 拷进 upper 让本次开机立即生效
        if ! cmp -s "$TMPD/chrony-cn.lower" "$CONF"; then
            cp "$TMPD/chrony-cn.lower" "$CONF" && echo "-- 已同步进 overlay（本次开机立即生效）" && changed=1
        fi
        rm -f "$TMPD/chrony-cn.lower"
    fi
fi
# 非 overlay 或 verity：直接改 /etc 视图（幂等）
if ! is_cn "$CONF"; then
    rewrite "$CONF" "$CONF.new" && mv "$CONF.new" "$CONF" && changed=1 && echo "-- /etc 视图已改"
fi

synced() { timedatectl 2>/dev/null | grep -q "synchronized: yes"; }
if [ "$changed" = "1" ] || ! synced; then
    systemctl restart chronyd
    i=0
    while [ "$i" -lt 15 ] && ! synced; do sleep 2; i=$((i + 1)); done
fi
echo "-- servers: $(grep "^server " "$CONF" | awk '{print $2}' | tr '\n' ' ')"
if synced; then
    echo "✅ 时钟已同步：$(journalctl -u chronyd --no-pager -b 2>/dev/null | grep 'Selected source' | tail -n 1 | sed 's/.*Selected source //')  $(date '+%F %T %Z')"
else
    echo "⚠ 未同步（网络不通？）：journalctl -u chronyd 看原因"; exit 2
fi
