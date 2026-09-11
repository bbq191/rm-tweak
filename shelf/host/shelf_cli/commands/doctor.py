"""host 环境体检：系统 python3、uv/venv 劫持、Calibre、pymupdf；设备侧：网关可达。
`--render`：洗书排版的**真机回归探针**——造探针 EPUB → 投原生 → 等渲染自检 → 取回 xochitl 渲染缓存 PDF → pymupdf
量首行缩进/顶格 → 与期望比（书架白皮书 §03y 的八轮手工诊断固化成一条命令；固件 OTA 后跑一次即知 CSS 引擎有没有变）。"""
import json
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

from .. import calibre_bridge as cb

NAME = "doctor"
HELP = "检查 host 依赖（Calibre/pymupdf/venv 劫持）与设备可达性；--render 跑真机排版回归探针"
RENDER_FOLDER = "书架自检"
RENDER_WAIT_SECS = 600


def add_args(p):
    p.add_argument("--render", action="store_true", help="投探针书到原生书库，量 xochitl 渲染缓存核对首行缩进/顶格（探针留在原生书库「书架自检」文件夹，可手动删）")
    p.add_argument("--keep", action="store_true", help="--render 时保留探针在母版库与本地工作目录（默认量完即删）")


def venv_hijack(env: dict) -> bool:
    """在 uv/venv 里跑会劫持 ebook-convert 的 `#!/usr/bin/env python3`。"""
    return bool(env.get("VIRTUAL_ENV")) or any(".venv/bin" in p for p in env.get("PATH", "").split(os.pathsep))


def run(args, ctx) -> int:
    if getattr(args, "render", False):
        return run_render(args, ctx)
    rc = 0
    print(f"python3       : {sys.executable} ({sys.version.split()[0]})")
    if venv_hijack(dict(os.environ)):
        print("venv 劫持     : ⚠ 处于 VIRTUAL_ENV/.venv PATH 中——调 Calibre 前会自动清洗（calibre_bridge）")
    else:
        print("venv 劫持     : 无")
    for tool, why in [("ebook-convert", "host 高质量路（洗书/定稿/漫画转 CBZ）"), ("k2pdfopt", "可选：扫描件 PDF 重排兜底")]:
        p = shutil.which(tool)
        print(f"{tool:<14}: {p or '缺（' + why + '）'}")
        if tool == "ebook-convert" and not p:
            print("                → 无 Calibre 时 `shelf push` 原样落母版库，优化在网页母版库里点")
    py = cb.py_with_pymupdf()
    ok = subprocess.run([*py, "-c", "import pymupdf"], env=cb.clean_env(), capture_output=True).returncode == 0
    print(f"pymupdf       : {'有' if ok else '缺'}（经 {' '.join(py[:2])}；体检/裁边/大 PDF 分卷{'可用' if ok else '不可用'}）")
    print(f"认证          : 密码{'已提供' if ctx.config.password else '未提供（config.toml password / $SHELF_PASSWORD / 交互输入）'}；TLS 校验 {'开' if ctx.config.verify_tls else '关（自签）'}")
    try:
        n = len(ctx.transport.get("/api/services").get("services", []))
        print(f"设备网关      : {ctx.config.base_url} 在线，{n} 个服务")
    except Exception as e:  # noqa: BLE001
        rc = 1
        print(f"设备网关      : {ctx.config.base_url} 不可达（{e}）")
    return rc


def _render_state(t, name: str) -> dict | None:
    """母版库条目的渲染自检结果（`delivered.render`）；条目不在 → None。"""
    for it in t.get("/api/books/staging").get("items", []):
        if it.get("name") == name:
            return (it.get("delivered") or {}).get("render") or {}
    return None


def wait_render(t, name: str, timeout: float = RENDER_WAIT_SECS, sleep=time.sleep) -> dict:
    """先查一次；还在 pending 就挂事件流等 books/render（不轮询），到点为止。返回 render 记录（可能 pending/timeout）。"""
    st = _render_state(t, name) or {}
    if st.get("status") not in (None, "", "pending"):
        return st
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            for line in t.stream_lines("/api/events"):
                if time.monotonic() >= deadline:
                    break
                if not line.startswith("data:"):
                    continue
                try:
                    ev = json.loads(line[5:].strip())
                except ValueError:
                    continue
                if ev.get("kind") == "render" and ev.get("name") == name and ev.get("status") not in (None, "pending"):
                    return _render_state(t, name) or {}
        except Exception:  # noqa: BLE001  设备睡了/断线：稍等重连
            sleep(3)
        st = _render_state(t, name) or {}
        if st.get("status") not in (None, "", "pending"):
            return st
    return st


def run_render(args, ctx) -> int:
    t = ctx.transport
    work = cb.workdir("shelf-doctor-")
    title = "书架自检探针 " + time.strftime("%Y%m%d-%H%M%S")
    probe = cb.render_probe(work, title)
    print(f"探针          : {probe.name}（拉丁 h1 后首段/场景切换/续段 + 中文段，自带 cangjie-wash.css）")
    d = t.post_files("/api/books/staging", [probe])
    items = d.get("items") or []
    if not items or not items[0].get("ok"):
        print(f"入母版库失败  : {d}")
        return 1
    name = items[0].get("name") or probe.name
    r = t.post_json("/api/books/staging/deliver", {"name": name, "keep": True, "folder": RENDER_FOLDER})
    print(f"投原生        : {r.get('message', r)}")
    st = wait_render(t, name)
    status = st.get("status")
    trashed = False
    print(f"渲染自检      : {status} pages={st.get('pages')} expected={st.get('expected')} uuid={st.get('uuid') or '-'}")
    rc = 1
    if status in ("ok", "warn") and st.get("uuid"):
        pdf = work / "render.pdf"
        pdf.write_bytes(t.get_bytes(f"/api/books/staging/render/{st['uuid']}"))
        if not args.keep:
            # 探针不在原生书库里累积：排进回收站队列，Sidebar 代理在书库视图下次有动静（本次 /upload 的 4s 防抖内即可）移进回收站
            try:
                t.post_json("/api/books/trash/add", {"uuid": st["uuid"], "name": title})
                trashed = True
            except Exception as e:  # noqa: BLE001
                trashed = False
                print(f"回收站        : 排队失败（{e}），探针留在原生书库「{RENDER_FOLDER}」")
        res = cb.render_measure(pdf)
        for row in res.get("rows", []):
            print(f"  {row['sentinel']:<9} p{row['page']:<3} 首行 {row['indent_pt']:>7} pt  字号 {row['size_pt']:>5} pt  = {row['em']:>6} em")
        if res.get("ok"):
            print("结论          : PASS —— 顶格 / 首行缩进与 §03y 配方一致")
            rc = 0
        else:
            print("结论          : FAIL")
            for p in res.get("problems", []):
                print(f"  ✗ {p}")
    elif status == "timeout":
        print("结论          : 10 分钟内没等到 xochitl 渲染；设备上打开探针书一次再跑")
    else:
        print("结论          : 无法量测（渲染自检没给出 uuid）")
    if not args.keep:
        t.post_json("/api/books/staging/delete", {"name": name})
        shutil.rmtree(work, ignore_errors=True)
        if trashed:
            print("提示          : 探针已从母版库删除并排队进原生回收站（设备回到书库视图即执行；回收站里可恢复）")
    else:
        print(f"提示          : --keep：探针留在母版库与原生书库「{RENDER_FOLDER}」文件夹")
    return rc
