"""`shelf doctor --render` 的探针 EPUB（纯 stdlib）：把书架白皮书 §03y 八轮手工"诊断 EPUB"固化成一本可重复投的书。

每段以哨兵词开头（PFLUSHn = 期望顶格，PINDENTn = 期望首行缩进），量测脚本 render_measure.py 在 xochitl 的渲染缓存
`<uuid>.pdf` 里按哨兵定位，量"首行 x − 下一行 x"。探针**自带** cangjie-wash.css，字面上与 bookconv `wash_css`
拉丁配方一致（`p{text-indent:1.2em;…}` + `.cj-flush{text-indent:0.01em;…}`，每条声明带尾分号）——配方改了这里要跟。
第一章拉丁（h1 后首段 / 场景切换后 / 续段），第二章中文（同一规则走 CJK 字体回退）。不过优化器（投原生是纯字节拷贝）。

用法: python3 render_probe.py <输出目录> [书名]   → 末行 JSON {"out": 产物路径}
"""
from __future__ import annotations

import json
import random
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from epub_skel import Chapter, write_epub  # noqa: E402  共享 EPUB 骨架

CSS = "p{text-indent:1.2em;margin-top:0;margin-bottom:0;padding-top:0;padding-bottom:0;}\n.cj-flush{text-indent:0.01em;margin-top:0;margin-bottom:0;}\n"
EXPECT_EM = 1.2
# 哨兵 → 期望：flush（顶格）/ indent（1.2em）
SENTINELS = {"PFLUSH1": "flush", "PINDENT1": "indent", "PINDENT2": "indent", "PFLUSH2": "flush", "PINDENT3": "indent", "PFLUSH3": "flush", "PINDENT4": "indent"}

_WORDS = "the quick brown fox jumps over a lazy dog while reading on the small paper device by the window in soft light".split()
_HAN = "天地玄黄宇宙洪荒日月盈昃辰宿列张寒来暑往秋收冬藏闰余成岁律吕调阳云腾致雨露结为霜金生丽水玉出昆冈剑号巨阙珠称夜光果珍李柰菜重芥姜海咸河淡鳞潜羽翔"


def _latin(sentinel: str, rnd: random.Random, n: int = 70) -> str:
    return sentinel + " " + " ".join(rnd.choice(_WORDS) for _ in range(n)) + "."


def _cjk(sentinel: str, rnd: random.Random, n: int = 140) -> str:
    return sentinel + " " + "".join(rnd.choice(_HAN) for _ in range(n)) + "。"


def build(out: Path, title: str) -> Path:
    rnd = random.Random(20260906)
    ch1 = "\n".join(
        [
            "<h1>Chapter One</h1>",
            f'<div class="cj-flush">{_latin("PFLUSH1", rnd)}</div>',
            f"<p>{_latin('PINDENT1', rnd)}</p>",
            f"<p>{_latin('PINDENT2', rnd)}</p>",
            '<p class="cj-flush">* * *</p>',
            f'<div class="cj-flush">{_latin("PFLUSH2", rnd)}</div>',
            f"<p>{_latin('PINDENT3', rnd)}</p>",
        ]
    )
    ch2 = "\n".join(
        [
            "<h1>第二章</h1>",
            f'<div class="cj-flush">{_cjk("PFLUSH3", rnd)}</div>',
            f"<p>{_cjk('PINDENT4', rnd)}</p>",
        ]
    )
    write_epub(out, title, [Chapter("Chapter One", ch1), Chapter("第二章", ch2)], css=CSS, css_name="cangjie-wash.css", lang="en", uid=f"urn:shelf:probe:{title}")
    return out


def main() -> int:
    if len(sys.argv) not in (2, 3):
        print("用法: render_probe.py <输出目录> [书名]", file=sys.stderr)
        return 2
    title = sys.argv[2] if len(sys.argv) == 3 else "书架自检探针"
    p = build(Path(sys.argv[1]) / f"{title}.epub", title)
    print(json.dumps({"out": str(p)}, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    sys.exit(main())
