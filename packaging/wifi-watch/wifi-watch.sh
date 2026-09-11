#!/bin/sh
# wifi-watch —— 常驻看护 wlan0 载波（装为 systemd 服务 wifi-watch.service，/usr 单元，OTA 后重装）。
# 定位（2026-09-06）：连上 60 s 必掉的真凶是 cfg80211 regdomain 宽限——精简 regulatory.db 的 CN 不含 5150–5350，路由 5G 信道 36
# 被判非法而断开（书架白皮书 §03w），根治是锁 2.4G 或路由改 149+ 信道。本脚本只做兜底：slumber 醒来后 wlan0 偶发假死
# （NetworkManager 仍标 connected、永不自愈）时 `nmcli con up`。⚠ `nmcli con up` 对已激活连接会先断再连，所以判据必须是真 NO-CARRIER。
# 做法：每 INTERVAL 秒看一次；wlan0 存在、rfkill 未软锁、NM 有 wifi 连接、却连续 STRIKES 次 NO-CARRIER → `nmcli con up`。
# 只在"NM 以为连着但链路死了"时动手；用户关 WiFi（rfkill/NM 断开）不干预。日志 journalctl -u wifi-watch。
# 固化（用户 2026-09-06 拍板）：当前活动的 WiFi 连接缺 `802-11-wireless.band=$BAND`（缺省 bg=2.4G）或
# `powersave=2` 就补上并重新激活一次——新 SSID / 在设置里重连后自动生效。BAND= 置空即不管频段。
#
# 省电（2026-09-20，用户反馈整机耗电）：原版每 15 s 都 fork rfkill/nmcli/grep/head/cut，开机一小时子进程累计 16 s CPU
# （≈0.46%，是 book-serve 空闲开销的 3 倍且不停唤醒 CPU）。而 99% 的时间链路是好的——链路好只需读一个 sysfs 文件
# （shell 内建 read，零 fork）。所以：carrier=1 走快路径不 fork 任何外部命令；只有 carrier≠1（疑似假死）才走原来的
# rfkill/nmcli 慢路径；"固化频段/省电"只在 carrier 0→1 跳变（新连接必然伴随）和每 RECHECK 个周期兜底复查时做。
IFACE=${IFACE:-wlan0}
INTERVAL=${INTERVAL:-15}
STRIKES=${STRIKES:-2}
RECHECK=${RECHECK:-40}   # 快路径下每 RECHECK 个周期兜底复查一次固化（40×15s=10 分钟）
BAND=${BAND-bg}
SYSFS=${SYSFS:-/sys/class/net}
strikes=0
enforced=""
prev=""
tick=0

# 当前 wlan0 的活动连接名（要 fork nmcli，只在需要时调用）
active_con() {
    nmcli -t -f DEVICE,NAME con show --active 2>/dev/null | grep "^$IFACE:" | head -n 1 | cut -d: -f2-
}

# 固化频段/省电（每个连接只查一次，避免反复打 nmcli）
enforce() {
    con="$1"
    [ "$enforced" != "$con" ] || return 0
    changed=""
    if [ -n "$BAND" ] && [ "$(nmcli -g 802-11-wireless.band con show "$con" 2>/dev/null)" != "$BAND" ]; then
        nmcli con modify "$con" 802-11-wireless.band "$BAND" 2>/dev/null && changed="band=$BAND"
    fi
    # ⚠ `nmcli -g` 回的是文字 "disable"（不是数字 2）；旧版拿它跟 "2" 比，永远不等 → 每次启动都白白改一遍并 con up 断线重连。
    case "$(nmcli -g 802-11-wireless.powersave con show "$con" 2>/dev/null)" in
        2|disable|"2 (disable)") ;;
        *) nmcli con modify "$con" 802-11-wireless.powersave 2 2>/dev/null && changed="$changed powersave=2" ;;
    esac
    if [ -n "$changed" ]; then
        out="$(nmcli con up "$con" 2>&1 | tail -n 1)"
        echo "固化 '$con' $changed → 重新激活: $out"
    fi
    enforced="$con"
}

while :; do
    sleep "$INTERVAL"
    [ -e "$SYSFS/$IFACE" ] || continue
    # carrier：1=有载波，0=NO-CARRIER，读失败（接口 admin down）=空。read 是 shell 内建，不 fork。
    carrier=""
    { read -r carrier < "$SYSFS/$IFACE/carrier"; } 2>/dev/null
    # 接口 admin down（用户关了 WiFi）：不可能有 NO-CARRIER，无事可做，连 rfkill/nmcli 都不必问。
    if [ -z "$carrier" ]; then strikes=0; prev=""; tick=0; continue; fi
    if [ "$carrier" = 1 ]; then
        strikes=0
        tick=$((tick + 1))
        if [ "$prev" != 1 ] || [ "$tick" -ge "$RECHECK" ]; then
            tick=0
            if ! rfkill list wifi 2>/dev/null | grep -q "Soft blocked: yes"; then
                con="$(active_con)"
                [ -n "$con" ] && enforce "$con"
            fi
        fi
        prev=1
        continue
    fi
    prev="$carrier"
    tick=0
    # ── 慢路径（链路疑似死了 / 接口没起）：与原版逻辑一致 ──
    if rfkill list wifi 2>/dev/null | grep -q "Soft blocked: yes"; then strikes=0; continue; fi
    con="$(active_con)"
    [ -n "$con" ] || { strikes=0; continue; }
    enforce "$con"
    if [ "$carrier" = 0 ]; then
        strikes=$((strikes + 1))
        if [ "$strikes" -ge "$STRIKES" ]; then
            out="$(nmcli con up "$con" 2>&1 | tail -n 1)"
            echo "$IFACE NO-CARRIER x$strikes → nmcli up '$con': $out"
            strikes=0
        fi
    else
        strikes=0
    fi
done
