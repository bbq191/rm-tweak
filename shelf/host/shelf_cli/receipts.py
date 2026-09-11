"""上传回执/守卫的单点实现：各命令（font/wallpaper/koreader/push）共用「✗ 不是文件」守卫、
「✓/✗ <名>: <消息>」回执行，以及"逐文件上传并打回执"的循环。

服务端回执统一形状 `{ok, items:[{name, ok, message, item?}]}`（`shelf_core::asset::receipt`）；旧服务用过 `file` 键，
`item_name` 两个都认。成功项的额外尾注（如字体家族）走 `extra` 回调。"""
from __future__ import annotations

from pathlib import Path


def mark(ok: bool) -> str:
    return "✓" if ok else "✗"


def item_name(it: dict, default: str = "?") -> str:
    return it.get("name") or it.get("file") or default


def guard_file(f: Path) -> bool:
    """不是文件→打印 ✗ 并返回 False（调用方据此置 rc 并 continue）。"""
    if not f.is_file():
        print(f"✗ {f}: 不是文件")
        return False
    return True


def print_receipts(d: dict, default_name: str = "?", prefix: str = "", extra=None) -> int:
    """打印一个上传响应里每项的 ✓/✗ 回执，返回 rc（0=全 ok，1=有失败）。响应无 items 时把响应体自身当作单条。
    `extra(it)`：仅对成功项追加的尾注字符串（如 `  家族=…`）。"""
    items = d.get("items") or [d]
    rc = 0
    for it in items:
        ok = bool(it.get("ok"))
        tail = extra(it) if (extra and ok) else ""
        print(f"{mark(ok)} {prefix}{item_name(it, default_name)}: {it.get('message', '')}{tail}")
        if not ok:
            rc = 1
    return rc


def upload_each(transport, api: str, files: list[Path], query=None, extra=None, on_response=None) -> int:
    """逐文件 POST 到 `api`（每本独立成败），守卫非文件，打回执；返回 rc。
    `query(i, f)` 可按序号/文件给查询参数；`on_response(d)` 拿到每次响应（如壁纸的 activated）。"""
    rc = 0
    for i, f in enumerate(files):
        if not guard_file(f):
            rc = 1
            continue
        try:
            d = transport.post_files(api, [f], query(i, f) if query else None)
        except Exception as e:  # noqa: BLE001
            print(f"✗ {f.name}: {e}")
            rc = 1
            continue
        rc |= print_receipts(d, default_name=f.name, extra=extra)
        if on_response:
            on_response(d)
    return rc
