NAME = "services"
HELP = "列出设备上活着的书架服务"


def add_args(p):
    pass


def run(args, ctx) -> int:
    j = ctx.transport.get("/api/services")
    svcs = j.get("services", [])
    if not svcs:
        print("（无服务注册）")
        return 1
    for s in svcs:
        tab = s.get("ui", {}).get("title", "-") if s.get("ui") else "-"
        print(f"{s['name']:<16} v{s['version']:<8} :{s['port']:<6} tab={tab}")
    return 0
