#!/bin/sh
# cang-jie 休眠壁纸 安装/OTA 重建脚本(设备端跑,需 root)。
# 逻辑+图片全在 /home/root/wallpaper(OTA 不丢);rootfs 只放 1 service + 1 sleep 钩子。
# 固件 OTA 冲掉 rootfs 后,重跑本脚本即可恢复:  sh /home/root/wallpaper/install.sh
set -e
DIR=/home/root/wallpaper
SVC=/usr/lib/systemd/system/cangjie-wallpaper.service
HOOK=/usr/lib/systemd/system-sleep/cangjie-wallpaper.sh

chmod +x "$DIR/roll.sh" "$DIR/bind.sh" "$DIR/unbind.sh"
[ -f "$DIR/current.png" ] || cp "$DIR/1.png" "$DIR/current.png"
[ -f "$DIR/state" ] || echo 1 > "$DIR/state"

echo "[*] remount rootfs rw"
mount -o remount,rw /

echo "[*] 写 boot bind service(开机建立全部 bind)"
cat > "$SVC" <<'UNIT'
[Unit]
Description=cang-jie suspend wallpaper bind-mounts
DefaultDependencies=no
After=local-fs.target
ConditionPathExists=/home/root/wallpaper/current.png

[Service]
Type=oneshot
RemainAfterExit=yes
ExecStart=/home/root/wallpaper/bind.sh
ExecStop=/home/root/wallpaper/unbind.sh

[Install]
WantedBy=multi-user.target
UNIT

echo "[*] 写 sleep 钩子(before 确保 bind;after 滚下一张;两套参数约定都认;每周期防重滚)"
cat > "$HOOK" <<'HK'
#!/bin/sh
DIR=/home/root/wallpaper
FLAG=/tmp/cangjie-wp-rolled
[ -f "$DIR/current.png" ] || exit 0
case "$1" in
  before|pre)
    "$DIR/bind.sh"
    rm -f "$FLAG"
    ;;
  after|post)
    if [ ! -e "$FLAG" ]; then
      "$DIR/roll.sh"
      touch "$FLAG"
    fi
    ;;
esac
exit 0
HK
chmod +x "$HOOK"

echo "[*] 立即建立全部 bind(不等重启)"
"$DIR/bind.sh"

echo "[*] enable + start service"
systemctl daemon-reload
systemctl enable cangjie-wallpaper.service >/dev/null 2>&1 || true
systemctl start  cangjie-wallpaper.service >/dev/null 2>&1 || true

echo "[*] remount rootfs ro(还原)"
mount -o remount,ro / || echo "  (remount ro 失败,重启会自动回 ro,无碍)"

n=$(grep -cE "remarkable/(suspended\.png|carousel/sleep_Illustration)" /proc/mounts || true)
echo "[OK] 安装完成。当前壁纸=$(cat $DIR/state).png,bind 共 $n 条(应=4:suspended + 3 张 carousel)"
