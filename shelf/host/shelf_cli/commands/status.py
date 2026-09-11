"""`shelf status`：网关在线 + 每个活着的服务打一次 `/health`。URL 段↔服务名映射来自网关 `/api/manage`
（单一事实源 `manage::MODULES`），本地不再硬编码一份。"""
NAME = "status"
HELP = "网关与各服务健康状态"


def add_args(p):
    pass


def run(args, ctx) -> int:
    t = ctx.transport
    try:
        svcs = t.get("/api/services").get("services", [])
    except Exception as e:  # noqa: BLE001
        print(f"网关 {ctx.config.base_url}: 不可达（{e}）")
        return 1
    print(f"网关 {ctx.config.base_url}: 在线")
    try:
        seg = {m["service"]: m["seg"] for m in t.get("/api/manage").get("modules", [])}
    except Exception:  # noqa: BLE001
        seg = {}
    ok = True
    for s in svcs:
        if s["name"] not in seg:
            continue
        try:
            h = t.get(f"/api/{seg[s['name']]}/health")
            extra = ""
            if s["name"] == "book-serve":
                try:
                    sp = t.get("/api/books/status").get("spool", {})
                    extra = f"  队列 待{sp.get('pending', 0)}/失败{sp.get('failed', 0)}"
                except Exception:  # noqa: BLE001
                    pass
            print(f"  {s['name']:<16} {'ok' if h.get('ok') else 'bad'}  v{h.get('version', '?')}{extra}")
        except Exception as e:  # noqa: BLE001
            ok = False
            print(f"  {s['name']:<16} 异常：{e}")
    if any(s["name"] == "book-serve" for s in svcs):
        print("  （失败的书：shelf inbox 查看/重投/删除）")
    return 0 if ok else 1
