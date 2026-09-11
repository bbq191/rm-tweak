"""子命令注册表（Command 模式）：每个模块暴露 `NAME`、`HELP`、`add_args(parser)`、`run(args, ctx) -> int`。"""
from . import doctor, events, font, inbox, koreader, passwd, push, services, status, wallpaper

ALL = [push, font, wallpaper, koreader, inbox, services, status, events, passwd, doctor]
