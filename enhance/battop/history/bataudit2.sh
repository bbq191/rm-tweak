#!/bin/sh
echo "=== A. 显要进程的应用归属(systemd unit / cgroup) ==="
for pid in 18817 550 6255 2727 236; do
  [ -d /proc/$pid ] || continue
  cm=$(cat /proc/$pid/comm 2>/dev/null)
  cg=$(sed -n 's/.*:://p' /proc/$pid/cgroup 2>/dev/null | head -1)
  exe=$(readlink /proc/$pid/exe 2>/dev/null)
  echo "pid $pid [$cm]  unit/cgroup: $cg"
  echo "      exe: $exe"
done
echo ""
echo "=== B. 正在运行的 service(谁常驻) ==="
systemctl list-units --type=service --state=running --no-legend --no-pager 2>/dev/null | awk '{print $1}'
echo ""
echo "=== C. 唤醒源计数(active_count>0,谁在打断/触发唤醒) ==="
for f in /sys/devices/*/power/wakeup_count /sys/devices/*/*/power/wakeup_count /sys/devices/*/*/*/power/wakeup_count; do
  [ -f "$f" ] || continue
  c=$(cat "$f" 2>/dev/null); [ "$c" -gt 0 ] 2>/dev/null || continue
  d=$(dirname "$(dirname "$f")"); nm=$(basename "$d")
  ac=$(cat "$d/power/wakeup_active_count" 2>/dev/null)
  echo "$c  active=$ac  $nm"
done | sort -rn | head -n 15
echo ""
echo "=== D. 一天内休眠/唤醒统计 ==="
echo "suspend 进入次数: $(journalctl -b 2>/dev/null | grep -c "PM: suspend entry")"
echo "suspend 退出次数: $(journalctl -b 2>/dev/null | grep -c "PM: suspend exit")"
echo "--- 被推迟/拒绝休眠(Deferring/Can't suspend) ---"
journalctl -b 2>/dev/null | grep -iE "Deferring suspend|Can.t suspend" | sed 's/.*]: //' | sort | uniq -c | sort -rn | head
echo "--- 唤醒原因(wakeup source 触发) ---"
journalctl -b 2>/dev/null | grep -iE "PM: Wakeup|wakeup source|Resume caused by" | tail -n 10
