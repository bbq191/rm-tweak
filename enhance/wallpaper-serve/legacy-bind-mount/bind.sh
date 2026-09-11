#!/bin/sh
# 建立/保持休眠壁纸的全部 bind(幂等,可反复调):
#   1) current.png  -> suspended.png        满幅背景壁纸
#   2) blank776.png -> carousel 三张插画     全透明,消掉中央"休眠插画卡"
# 真身文件从不被改;OTA 冲掉 rootfs 后由 install.sh 重跑本脚本恢复。
DIR=/home/root/wallpaper

ensure() {   # $1=源  $2=目标
  [ -f "$1" ] || return 0
  grep -qF " $2 " /proc/mounts || mount --bind "$1" "$2"
}

ensure "$DIR/current.png" /usr/share/remarkable/suspended.png
for n in 01 02 03; do
  ensure "$DIR/blank776.png" /usr/share/remarkable/carousel/sleep_Illustration_$n.png
done
