#!/bin/sh
# reMarkable Paper Pro Move 电池/后台进程审计(设备端跑,root)。
# 用功耗代理量排查:累计CPU / 生命期CPU占比(自旋者高) / 唤醒源 / 休眠健康 / 应用归属。
# 注意:插 USB 时 current_now=0 测不到实时放电;要真实 mA 需断 USB 后本地读。
sh "$(dirname "$0")/bataudit.sh"
echo; echo "############################################"; echo
sh "$(dirname "$0")/bataudit2.sh"
echo; echo "############################################"; echo
sh "$(dirname "$0")/bataudit3.sh"
