"""`shelf koreader pull|diff|sync`：配置即代码。Lua 处理全在设备端（koreader-serve 内嵌 merge.lua）。"""
from __future__ import annotations

import datetime as _dt
from pathlib import Path

from ..receipts import print_receipts, upload_each

NAME = "koreader"
HELP = "KOReader：pull | diff | sync [--dry-run] [--fonts] [--dicts] | font add|ls|rm（只装 KOReader）"

FILES = {"settings": "settings.reader.patch.lua", "defaults": "defaults.custom.lua", "gestures": "gestures.patch.lua"}
SNAP_NAMES = {"settings": "settings.reader.lua", "defaults": "defaults.custom.lua", "gestures": "gestures.lua"}
PROFILE_DIR = Path(__file__).resolve().parents[3] / "koreader" / "profile"
REPO_ROOT = Path(__file__).resolve().parents[4]
DICT_EXT = ("ifo", "idx", "dict", "dz", "syn", "oft")


def add_args(p):
    sub = p.add_subparsers(dest="op", required=True)
    pl = sub.add_parser("pull", help="拉三份配置到本机快照目录")
    pl.add_argument("--out", type=Path, help="缺省 $XDG_CACHE_HOME/shelf/snapshots/<时间>/")
    df = sub.add_parser("diff", help="profile 对设备 dry-run，列出将改的键")
    sy = sub.add_parser("sync", help="应用 profile（KOReader 须已退出）")
    sy.add_argument("--dry-run", "-n", action="store_true")
    sy.add_argument("--fonts", action="store_true", help="按 profile/fonts.txt 同步字体")
    sy.add_argument("--dicts", action="store_true", help="按 profile/dicts.txt 同步词典")
    for x in (df, sy):
        x.add_argument("--profile", type=Path, default=PROFILE_DIR)
    fo = sub.add_parser("font", help="给 KOReader 装字体：add <ttf...> | ls | rm <file>（原生阅读器用 `shelf font`）")
    fsub = fo.add_subparsers(dest="fop", required=True)
    fa = fsub.add_parser("add")
    fa.add_argument("files", nargs="+", type=Path)
    fsub.add_parser("ls")
    fr = fsub.add_parser("rm")
    fr.add_argument("file")


def _print_changes(res: dict) -> int:
    ch = res.get("changes") or []
    for c in ch:
        print(f"    {c['path']}: {c.get('old')!r} → {c.get('new')!r}")
    if not ch:
        print("    （无变化）")
    return len(ch)


def _apply(ctx, profile: Path, dry: bool) -> int:
    rc = 0
    for key, fname in FILES.items():
        f = profile / fname
        if not f.is_file():
            print(f"- {key}: profile 缺 {fname}，跳过")
            continue
        print(f"- {key} ({fname}){' [dry-run]' if dry else ''}")
        try:
            res = ctx.transport.post_text(f"/api/koreader/config/{key}", f.read_bytes(), {"dry_run": "1"} if dry else None)
        except Exception as e:  # noqa: BLE001
            print(f"    ✗ {e}")
            rc = 1
            continue
        n = _print_changes(res)
        if res.get("written"):
            print(f"    已写入（备份 {res.get('backup')}）")
        elif not dry and n:
            print("    ⚠ 有差异但未写入")
    return rc


def _read_list(path: Path) -> list[list[str]]:
    if not path.is_file():
        return []
    rows = []
    for line in path.read_text(encoding="utf-8").splitlines():
        line = line.strip()
        if line and not line.startswith("#"):
            rows.append(line.split())
    return rows


def run(args, ctx) -> int:
    t = ctx.transport
    if args.op == "pull":
        out = args.out or (ctx.paths.snapshots_dir / _dt.datetime.now().strftime("%Y%m%d-%H%M%S"))
        out.mkdir(parents=True, exist_ok=True)
        for key, snap in SNAP_NAMES.items():
            try:
                text = t.get_text(f"/api/koreader/config/{key}")
            except Exception as e:  # noqa: BLE001
                print(f"✗ {key}: {e}")
                continue
            (out / snap).write_text(text, encoding="utf-8")
            print(f"✓ {key}: {len(text)} 字节")
        print(f"快照：{out}")
        return 0
    if args.op == "font":
        if args.fop == "ls":
            for it in t.get("/api/koreader/fonts").get("items", []):
                print(f"{it['name']:<40} {it.get('bytes', 0) // 1024} KB")
            return 0
        if args.fop == "rm":
            t.delete_named("/api/koreader/fonts", args.file)
            print(f"已从 KOReader 删除 {args.file}（KOReader 运行中需重启它）")
            return 0
        return upload_each(t, "/api/koreader/fonts", args.files)
    if args.op == "diff":
        return _apply(ctx, args.profile, dry=True)
    rc = _apply(ctx, args.profile, dry=args.dry_run)
    if args.dry_run:
        return rc
    if args.fonts:
        for row in _read_list(args.profile / "fonts.txt"):
            p = Path(row[0]) if row[0].startswith("/") else REPO_ROOT / row[0]
            if not p.is_file():
                print(f"✗ 字体缺：{p}")
                rc = 1
                continue
            print_receipts(t.post_files("/api/koreader/fonts", [p]), prefix="字体 ")
    if args.dicts:
        for row in _read_list(args.profile / "dicts.txt"):
            if len(row) < 2:
                continue
            name, d = row[0], Path(row[1]).expanduser()
            files = [f for f in d.iterdir() if f.suffix.lstrip(".") in DICT_EXT] if d.is_dir() else []
            if not files:
                print(f"✗ 词典 {name}: 目录 {d} 无 StarDict 文件")
                rc = 1
                continue
            res = t.post_files("/api/koreader/dicts", files, {"name": name})
            print(f"{'✓' if res.get('ok') else '✗'} 词典 {name}: {len(res.get('items', []))} 个文件")
    return rc
