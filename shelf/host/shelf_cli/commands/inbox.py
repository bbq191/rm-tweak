"""book-serve 的 spool 队列：列出待处理/失败的书，重试或删除失败项（对齐网页 /inbox）。"""
from __future__ import annotations

import sys

NAME = "inbox"
HELP = "scp 追平队列（inbox/）：列出 / 重试 / 删除入库失败的书（--retry / --delete 名字）"


def add_args(p):
    p.add_argument("--retry", metavar="名字", help="把某个失败项移回队列重投")
    p.add_argument("--delete", metavar="名字", help="删除某个失败项")


def run(args, ctx) -> int:
    t = ctx.transport
    if args.retry:
        r = t.post_json("/api/books/inbox/retry", {"name": args.retry})
        ok = r.get("ok", False)
        print(("已重投：" if ok else "重投失败：") + str((r.get("items") or [{}])[0].get("message", r.get("message", ""))))
        return 0 if ok else 1
    if args.delete:
        r = t.post_json("/api/books/inbox/delete", {"name": args.delete})
        ok = r.get("ok", False)
        print("已删除" if ok else f"删除失败：{r.get('message', r)}")
        return 0 if ok else 1
    items = t.get("/api/books/inbox").get("items", [])
    if not items:
        print("队列为空")
        return 0
    for it in items:
        line = f"  [{it['state']:<7}] {it['name']}  ({it.get('bytes', 0)} B)"
        if it.get("reason"):
            line += f"\n            原因：{it['reason']}"
        print(line)
    failed = [i for i in items if i["state"] == "failed"]
    if failed:
        print(f"\n{len(failed)} 项失败。重投：shelf inbox --retry <名字>；删除：shelf inbox --delete <名字>", file=sys.stderr)
    return 0
