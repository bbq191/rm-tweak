#!/bin/sh
# lo-alias：让 10.11.99.1 在不插 USB 时也常驻可达。
# xochitl 的网页上传口（:80，POST /upload）只绑 USB 网卡的 10.11.99.1。不插 USB 时 USB 网卡 down、
# 这个地址从所有接口消失，本机服务（网关传书 / 笔记推送等）往 /upload 发请求就报
# Network unreachable (os error 101)（2026-08-28 真机根因）。给 lo 加 /32 别名后地址常驻，
# xochitl 已绑好的 :80 socket 仍能在本机 accept。幂等；不影响真插 USB 时电脑经 USB 网段访问设备。
# 调用方：网关单元 gateway.service 的 ExecStartPre（失败不阻塞网关启动）。
if ip addr add 10.11.99.1/32 dev lo 2>/dev/null; then
    echo "[lo-alias] 已加 10.11.99.1/32 到 lo"
else
    echo "[lo-alias] 已存在或跳过"
fi

# usb1 静态 IP：治「无 USB 冷启动时 :80 根本没绑起来」——xochitl 启动选接口时 usb0 无 carrier
# 就落到 usb1 且不再查 carrier，绑该接口上的地址（反编译 0x71e070 坐实；2026-08-31 真机端到端验证：
# usb0/usb1 都无 carrier 冷启动 → HttpListener 绑 10.11.99.1:80、GET /documents/ 返回 200）。
# lo 别名只保住「已绑之后」可达、不触发冷启动绑定；usb1 这段才让冷启动真绑起来。
# 注意时序：本脚本现在由网关 ExecStartPre 调用，并不保证先于 xochitl 运行；xochitl 冷启动奇慢
# （重启到 HttpListener 约 3 分钟），地址通常赶在它绑 :80 之前挂上，但现行接法下无 USB 冷启动
# 这一场景没有重新真机验证过。开机头几分钟 :80 缺失属正常，依赖它的服务应失败即重试。幂等。
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
