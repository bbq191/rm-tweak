#!/bin/sh
# 撤掉全部休眠壁纸 bind(真身文件本就没动,撤挂即完全还原)。
umount /usr/share/remarkable/suspended.png 2>/dev/null
for n in 01 02 03; do
  umount /usr/share/remarkable/carousel/sleep_Illustration_$n.png 2>/dev/null
done
