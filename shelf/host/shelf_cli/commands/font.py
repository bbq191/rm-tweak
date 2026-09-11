"""`shelf font add|ls|rm`：只装原生阅读器（fontconfig 用户字体目录）；KOReader 用 `shelf koreader font`。"""
from pathlib import Path

from ..receipts import upload_each

NAME = "font"
HELP = "字体：add <ttf/otf...> | ls | rm <家族名>（删该家族全部文件）"


def add_args(p):
    sub = p.add_subparsers(dest="op", required=True)
    a = sub.add_parser("add", help="上传并安装")
    a.add_argument("files", nargs="+", type=Path)
    sub.add_parser("ls", help="列出")
    r = sub.add_parser("rm", help="按家族名删除（全部字重文件）")
    r.add_argument("file", metavar="family")


def run(args, ctx) -> int:
    t = ctx.transport
    if args.op == "ls":
        d = t.get("/api/fonts")
        for it in d.get("items", []):
            ex = it.get("extra") or {}
            cn = (ex.get("names") or {}).get("cn", "")
            print(f"{it['name']:<34} {cn if cn != it['name'] else '':<20} {len(ex.get('files') or []):>2} 文件 {'⚠界面回退' if ex.get('fontconfigRef') else ''}")
        return 0
    if args.op == "rm":
        t.delete_named("/api/fonts", args.file)
        print(f"已删除 {args.file}")
        return 0
    rc = upload_each(t, "/api/fonts", args.files, extra=lambda it: f"  家族={((it.get('item') or {}).get('extra') or {}).get('family')}")
    if rc == 0:
        print("原生阅读器「文字与布局」菜单重开即可选，无需重启；KOReader 请用 shelf koreader font add")
    return rc
