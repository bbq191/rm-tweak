#!/bin/sh
echo "=== A. systemd 定时器(后台活动节奏,谁周期性醒来干活) ==="
systemctl list-timers --all --no-legend --no-pager 2>/dev/null | awk '{print $NF, $(NF-1), $1}' | sed 's/[^ ]* //' 2>/dev/null
systemctl list-timers --all --no-pager 2>/dev/null | tail -n +1 | head -n 30
echo ""
echo "=== B. WiFi 省电态 + 关联(WiFi常连=大头) ==="
iw dev wlan0 get power_save 2>/dev/null || echo "(无 iw)"
iw dev wlan0 link 2>/dev/null | head -n 3 || cat /proc/net/wireless 2>/dev/null
echo ""
echo "=== C. memfault/metrics 是不是周期唤醒 daemon(看 unit 触发方式) ==="
for u in memfaultd crashuploader remarkable-counter-metrics slumber-metrics nm-metrics mdm-agent rm-sync; do
  echo "-- $u: $(systemctl show $u.service -p Type,ExecMainStartTimestamp -p TriggeredBy --value 2>/dev/null | tr "\n" " ")"
done
echo ""
echo "=== D. cang-jie 自家 daemon 的休眠礼貌(fswatch 是否轮询自旋) ==="
echo "cj-stars(2727) 状态: $(cat /proc/2727/stat 2>/dev/null | awk "{print \$3}")  (S=睡眠好, R=运行)"
echo "wr-serve(6255) 状态: $(cat /proc/6255/stat 2>/dev/null | awk "{print \$3}")"
