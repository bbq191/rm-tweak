#!/bin/sh
# cangjie: 把 10.11.99.1 别名挂到 loopback，让设备端注入回传（星标卡片/生词本/笔记本
# → xochitl web POST /upload，该 web 只绑 USB gadget IP 10.11.99.1:80）在**不插 USB**
# 时也本机可达。不插 USB 时 usb 网卡 down、10.11.99.1 从所有接口消失 → POST 报
# Network unreachable(os error 101) → 全部回传失败（2026-08-28 真机根因）。
# 给 lo 加 /32 别名后该地址常驻、xochitl :80 socket 仍可本机 accept
# （真机验证：断 USB 画星→卡片笔记本更新成功）。幂等；不影响真插 USB 时
# host(10.11.99.2) 经 usb 网段访问设备 web。
if ip addr add 10.11.99.1/32 dev lo 2>/dev/null; then
    echo "[lo-alias] 已加 10.11.99.1/32 到 lo"
else
    echo "[lo-alias] 已存在或跳过"
fi

# usb1 静态 IP：治本「无 USB 冷启动 :80 不绑」——xochitl 启动选接口时 usb0 无 carrier
# 就落 usb1 且不再查 carrier、绑到该接口 addressEntries 的 IP（反编译 0x71e070 坐实 +
# 2026-08-31 真机端到端验证：usb0/usb1 carrier=0 冷启动 → HttpListener 绑 10.11.99.1:80、
# GET /documents/ 返回 200）。lo 别名只保住「已绑后」可达、不触发冷启动绑定；usb1 这段才是
# 让冷启动真绑起来的关键。pre-start 跑在 xochitl 前、IP 早已就位。幂等；不阻塞 xochitl 启动。
# 注意：设备 xochitl 冷启动奇慢（重启到 HttpListener ~3min），开机头几分钟 :80 缺失属正常，
# 依赖它的 daemon（★待办/cardhw/回传）本就失败即重试、不标 done，扛得住这段时序。
if ip link show usb1 >/dev/null 2>&1; then
    ip link set usb1 up 2>/dev/null || true
    if ip addr add 10.11.99.1/32 dev usb1 2>/dev/null; then
        echo "[lo-alias] 已加 10.11.99.1/32 到 usb1（无 USB 冷启动 :80 绑定）"
    else
        echo "[lo-alias] usb1 别名已存在或跳过"
    fi
else
    echo "[lo-alias] usb1 接口暂不存在，跳过（不影响 lo 别名）"
fi
