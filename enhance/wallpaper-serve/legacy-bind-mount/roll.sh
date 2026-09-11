#!/bin/sh
# 交替休眠壁纸:current.png <- 另一张。原地覆盖(cat 重定向)保持 inode,
# 让 bind-mount 到 suspended.png 的绑定自动跟随新内容。
DIR=/home/root/wallpaper
cur=$(cat "$DIR/state" 2>/dev/null)
if [ "$cur" = "1" ]; then next=2; else next=1; fi
cat "$DIR/$next.png" > "$DIR/current.png"
echo "$next" > "$DIR/state"
