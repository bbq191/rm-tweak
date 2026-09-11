"""量 xochitl 渲染缓存 `<uuid>.pdf` 里探针段落的首行缩进（pymupdf；`uv run --group calibre` 路，见 calibre_bridge）。

方法＝书架白皮书 §03y：按哨兵词找到段首行，取它与紧随其后一视觉行（同页、y 序）的 x0 差 = 首行缩进（pt），
再除以该行字号得 em。真机坑：同一视觉行会被 pymupdf 拆成多段（拉丁哨兵 EBGaramond 与 CJK 回退字体 KingHwa 各成
一"line"，y0 差 2pt）——先按 y 把碎片合并成视觉行（`visual_lines`，纯函数），再量。
判定（`judge`，纯函数）：flush 段 |em| < 0.15；indent 段在期望 ±0.2 em。
输出：末行 JSON `{"rows":[{sentinel,page,indent_pt,size_pt,em}], "ok":bool, "problems":[...]}`。
用法: python render_measure.py <pdf> [期望em=1.2]
"""
from __future__ import annotations

import json
import re
import sys

SENT = re.compile(r"^(P(?:FLUSH|INDENT)\d+)")
FLUSH_MAX_EM = 0.15
INDENT_TOL_EM = 0.2


def visual_lines(raw: list[tuple[float, float, float, str]]) -> list[dict]:
    """(y0, x0, size, text) 碎片 → 视觉行 [{y, x, size, texts}]：y 差 < 0.6 字号的相邻碎片并成一行，x 取最小。"""
    out: list[dict] = []
    for y, x, size, text in sorted(raw):
        if out and abs(y - out[-1]["y"]) < out[-1]["size"] * 0.6:
            cur = out[-1]
            cur["x"] = min(cur["x"], x)
            cur["texts"].append(text)
        else:
            out.append({"y": y, "x": x, "size": size, "texts": [text]})
    return out


def sentinel_of(line: dict) -> str | None:
    for t in line["texts"]:
        m = SENT.match(t)
        if m:
            return m.group(1)
    return None


def rows_from_lines(lines: list[dict], page_no: int) -> list[dict]:
    rows = []
    for i, ln in enumerate(lines):
        s = sentinel_of(ln)
        if not s or i + 1 >= len(lines):
            continue
        nxt = lines[i + 1]
        if sentinel_of(nxt) or nxt["y"] - ln["y"] > ln["size"] * 3:
            continue  # 下一行不是本段续行（没换行的短段不量）
        d = ln["x"] - nxt["x"]
        rows.append({"sentinel": s, "page": page_no, "indent_pt": round(d, 2), "size_pt": round(ln["size"], 2), "em": round(d / ln["size"], 3) if ln["size"] else 0.0})
    return rows


def measure(pdf: str) -> list[dict]:
    import pymupdf  # 延迟导入：judge()/visual_lines() 不需要它，测试可无 pymupdf 跑

    rows = []
    with pymupdf.open(pdf) as doc:
        for page in doc:
            raw = []
            for b in page.get_text("dict")["blocks"]:
                for ln in b.get("lines", []):
                    spans = ln.get("spans", [])
                    text = "".join(s["text"] for s in spans).strip()
                    if spans and text:
                        raw.append((ln["bbox"][1], ln["bbox"][0], spans[0]["size"], text))
            rows.extend(rows_from_lines(visual_lines(raw), page.number + 1))
    return rows


def judge(rows: list[dict], expect: dict[str, str], expect_em: float = 1.2) -> tuple[bool, list[str]]:
    """`expect`: 哨兵 → 'flush' | 'indent'。缺哨兵也算问题（段没换行 / 整章没渲染）。"""
    seen = {r["sentinel"]: r for r in rows}
    problems = []
    for s, kind in expect.items():
        r = seen.get(s)
        if r is None:
            problems.append(f"{s}: 未量到（段落没渲染或没换行）")
            continue
        em = r["em"]
        if kind == "flush" and abs(em) >= FLUSH_MAX_EM:
            problems.append(f"{s}: 期望顶格，量到 {r['indent_pt']}pt = {em}em")
        elif kind == "indent" and not (expect_em - INDENT_TOL_EM <= em <= expect_em + INDENT_TOL_EM):
            problems.append(f"{s}: 期望缩进 {expect_em}em，量到 {r['indent_pt']}pt = {em}em")
    return not problems, problems


def main() -> int:
    if len(sys.argv) not in (2, 3):
        print("用法: render_measure.py <pdf> [期望em]", file=sys.stderr)
        return 2
    sys.path.insert(0, __file__.rsplit("/", 1)[0])
    from render_probe import EXPECT_EM, SENTINELS

    expect_em = float(sys.argv[2]) if len(sys.argv) == 3 else EXPECT_EM
    rows = measure(sys.argv[1])
    ok, problems = judge(rows, SENTINELS, expect_em)
    print(json.dumps({"rows": rows, "ok": ok, "problems": problems}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
