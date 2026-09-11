"""`shelf events`：订阅网关 `/api/events`（SSE），逐条打印设备上发生的变更——host 侧的"事件通知"。
用途：另开一个终端挂着，`shelf push` / 网页 / scp 进 inbox / 唤醒轮换……设备一有动静就打一行；`--once` 收到第一条就退出
（脚本里当"等它落库"用）。断线自动重连（设备休眠醒来常见）。"""
from __future__ import annotations

import json
import time

NAME = "events"
HELP = "订阅设备事件流（SSE）：母版库/inbox/字体/壁纸/KOReader/服务启停一有变更就打印"


def add_args(p):
    p.add_argument("--once", action="store_true", help="收到第一条事件就退出（可配合 --area 过滤）")
    p.add_argument("--area", help="只看某个区域：books|koreader|fonts|wallpapers|manage")
    p.add_argument("--raw", action="store_true", help="原样打印 JSON")


def fmt(ev: dict) -> str:
    at = time.strftime("%H:%M:%S", time.localtime(int(ev.get("at", time.time()))))
    line = f"{at}  {ev.get('svc', ev.get('area', '?')):<10} {ev.get('area', '?')}/{ev.get('kind', '?')}"
    # 带载荷的事件（如 books/render：name/status/pages/expected）把字段追加在后面
    extra = " ".join(f"{k}={ev[k]}" for k in ("name", "status", "pages", "expected") if k in ev)
    return f"{line}  {extra}" if extra else line


def run(args, ctx) -> int:
    t = ctx.transport
    if not hasattr(t, "stream_lines"):
        print("当前 transport 不支持流式（测试桩）")
        return 2
    backoff = 2
    while True:
        try:
            print(f"订阅 {ctx.config.base_url}/api/events …（Ctrl-C 退出）", flush=True)
            for line in t.stream_lines("/api/events"):
                backoff = 2
                if not line.startswith("data:"):
                    continue
                try:
                    ev = json.loads(line[5:].strip())
                except ValueError:
                    continue
                if args.area and ev.get("area") != args.area:
                    continue
                print(json.dumps(ev, ensure_ascii=False) if args.raw else fmt(ev), flush=True)
                if args.once:
                    return 0
            print("连接结束，重连…", flush=True)
        except KeyboardInterrupt:
            return 0
        except Exception as e:  # noqa: BLE001
            print(f"断开（{e}），{backoff}s 后重连", flush=True)
        time.sleep(backoff)
        backoff = min(backoff * 2, 30)
