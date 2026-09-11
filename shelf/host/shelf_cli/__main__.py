"""`shelf` 入口：argparse 子命令分发。"""
from __future__ import annotations

import argparse
import getpass
import os
import sys
from dataclasses import dataclass
from pathlib import Path

if __package__ in (None, ""):
    sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
    from shelf_cli import __version__, commands, config as cfgmod, paths as pathsmod, transport as tr  # type: ignore
else:
    from . import __version__, commands, config as cfgmod, paths as pathsmod, transport as tr


@dataclass
class Context:
    paths: pathsmod.Paths
    config: cfgmod.Config
    transport: object


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="shelf", description="reMarkable 书架 host CLI（与设备网页共用同一 HTTP API）")
    p.add_argument("--version", action="version", version=f"shelf {__version__}")
    p.add_argument("--host", help="设备 IP（缺省 config.toml 或 10.11.99.1）")
    p.add_argument("--port", type=int, help="网关端口（缺省 443）")
    p.add_argument("--http", action="store_true", help="用明文 HTTP（网关关了 https 时）")
    p.add_argument("--password", "-p", help="网关密码（缺省 config.toml / $SHELF_PASSWORD / 交互输入）")
    sub = p.add_subparsers(dest="cmd", required=True)
    for m in commands.ALL:
        sp = sub.add_parser(m.NAME, help=m.HELP)
        m.add_args(sp)
        sp.set_defaults(_run=m.run)
    return p


def main(argv: list[str] | None = None, transport_factory=None) -> int:
    args = build_parser().parse_args(argv)
    paths = pathsmod.Paths()
    pw = args.password or os.environ.get("SHELF_PASSWORD") or None
    cfg = cfgmod.load(paths, {"host": args.host, "port": args.port, "scheme": "http" if args.http else None, "password": pw})
    if not cfg.password and transport_factory is None and sys.stdin.isatty():
        cfg.password = getpass.getpass(f"书架密码（{cfg.host}；首次默认 shelf）: ")
    transport = (transport_factory or (lambda c: tr.HttpTransport(c.base_url, password=c.password, verify_tls=c.verify_tls)))(cfg)
    ctx = Context(paths=paths, config=cfg, transport=transport)
    try:
        return int(args._run(args, ctx) or 0)
    except tr.TransportError as e:
        print(f"错误：{e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
