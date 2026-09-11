import getpass
import sys

NAME = "passwd"
HELP = "改网关密码（首次登录必改；也可在网页 /password 改）"


def add_args(p):
    p.add_argument("--new", help="新密码（缺省交互输入两次）")


def run(args, ctx) -> int:
    new = args.new
    if not new:
        new = getpass.getpass("新密码（至少 6 位）: ")
        if new != getpass.getpass("再输一次: "):
            print("两次输入不一致", file=sys.stderr)
            return 1
    r = ctx.transport.post_json("/password", {"new": new})
    if r.get("ok"):
        print("密码已更新。config.toml 里的 password 请同步改，其它已登录的浏览器需重新登录。")
        return 0
    print(f"失败：{r.get('message', r)}", file=sys.stderr)
    return 1
