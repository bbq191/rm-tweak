#!/bin/sh
# battop 采集器 安装/OTA 重建(设备端跑,root)。
# 二进制+数据在 /home/root/battop(OTA 不丢);rootfs 只放 service+timer。
# OTA 后重跑:  sh /home/root/battop/install.sh
set -e
DIR=/home/root/battop
SVC=/usr/lib/systemd/system/battop.service
TMR=/usr/lib/systemd/system/battop.timer

[ -x "$DIR/battop" ] || { echo "缺 $DIR/battop(先 scp 二进制)"; exit 1; }
mkdir -p "$DIR/data"

echo "[*] remount rootfs rw"
mount -o remount,rw /

# 常驻采样服务（Type=simple）：进程内部循环每 ~10min 采一次。**不再用 timer 反复拉起 oneshot**——
# 反复 service-start 的 cgroup 迁移曾撞内核 RCU stall 冻死整机（2026-08-29 事故，见 FINDINGS）。
cat > "$SVC" <<'UNIT'
[Unit]
Description=battop battery/usage sampler (resident, ~10min interval)

[Service]
Type=simple
Nice=19
IOSchedulingClass=idle
ExecStart=/home/root/battop/battop
Restart=on-failure
RestartSec=60

[Install]
WantedBy=multi-user.target
UNIT

# 清掉旧 oneshot+timer 模型（停用+删 timer 文件；enable 符号链接在 /etc tmpfs，重启本就清）
systemctl disable --now battop.timer >/dev/null 2>&1 || true
rm -f "$TMR"
# 注：不再在此前台跑 "$DIR/battop"——常驻进程不退出，前台跑会永久阻塞 install。服务启动即首采建 baseline。

echo "[*] enable + start 常驻服务（启动即首采，之后每 ~10min）"
systemctl daemon-reload
systemctl enable --now battop.service >/dev/null 2>&1 || true

echo "[*] remount rootfs ro"
mount -o remount,ro / || echo "  (remount ro 失败,重启回 ro,无碍)"

echo "[OK] battop 已装（常驻）。服务状态:"
systemctl is-active battop.service
systemctl status battop.service --no-pager 2>/dev/null | grep -E "Active|Main PID" | head -n 2
