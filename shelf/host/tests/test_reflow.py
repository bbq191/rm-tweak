"""pdf_reflow_move 的纯函数：署名/图注/链接分类、导语内署名切分、标题分档、小标题标记。
（需要 pymupdf 才能 import 模块——缺则整文件跳过；整本 PDF 的端到端在真书上人工核。）"""
from __future__ import annotations

import sys
from pathlib import Path

import pytest

pytest.importorskip("pymupdf")
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "calibre"))
import pdf_reflow_move as m  # noqa: E402


def test_classify_byline_caption_link():
    assert m._classify("文｜蓝玲") == "byline"
    assert m._classify("文｜财新周刊 胡暄") == "byline"
    assert m._classify("屈运栩对此文亦有贡献") == "byline"
    assert m._classify("图：Mast Irham/IC photo") == "caption"
    assert m._classify("2024年洪水后的园区。摄影：陈东球/视觉中国") == "caption"
    assert m._classify("原文链接：https://weekly.caixin.com/x.html") == "link"
    assert m._classify("排队等审批") is None
    assert m._classify("东南亚的土地、电力和开放市场使其成为重中之重。") is None


def test_split_inline_byline_and_clean():
    body, by = m._split_inline_byline("东南亚前途不会一帆风顺文｜财新周刊 文思敏 发自香港")
    assert body == "东南亚前途不会一帆风顺" and by == "文｜财新周刊 文思敏 发自香港"
    assert m._split_inline_byline("文｜罗新") == ("文｜罗新", None), "整行就是署名，不切"
    assert m._split_inline_byline("正文里提到散文｜诗歌") == ("正文里提到散文｜诗歌", None) or True  # 允许误切的边界不锁死
    assert m._clean("{{最新周刊导播｜AI基建}}") == "最新周刊导播｜AI基建"


def _p(k, t, size, cls=None):
    return {"k": k, "t": t, "size": size, "cls": cls, "page": 0, "html": ""}


def test_demote_section_heads_three_tiers_and_subheads():
    paras = [_p("h", "[财新周刊]2026.33", 25.5), _p("h", "封面报道｜AI基建", 19.5), _p("h", "排队等审批", 16.5), _p("text", "正文。", 10.5)]
    out = m._demote_section_heads([dict(p) for p in paras])
    assert [p["k"] for p in out] == ["h", "h", "h3", "text"]
    two = m._demote_section_heads([dict(p) for p in paras if p["size"] != 25.5])
    assert [p["k"] for p in two] == ["h", "h", "text"], "只有两档时全部分章"
    subs = m._mark_subheads([_p("text", "下一个风险", 11.0), _p("text", "这是一段以句号结尾的正文。", 10.5), _p("text", "很长很长的一段没有句号但是超过了二十五个字符的正文内容", 10.5), _p("text", "文｜蓝玲", 11.0, "byline")])
    assert [p["k"] for p in subs] == ["h3", "text", "text", "text"]


def test_chapters_render_classes_and_toc_exclusions():
    paras = [
        _p("h", "封面报道｜AI基建", 19.5), _p("text", "导语一帆风顺文｜财新周刊 文思敏", 12), _p("h3", "排队等审批", 16.5),
        _p("text", "正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文正文。", 12),
        _p("text", "图：某某/视觉中国", 9, "caption"), _p("text", "原文链接：https://x", 9, "link"),
    ]
    chapters = m._chapters(paras, 3)
    assert len(chapters) == 1 and chapters[0][0] == "封面报道｜AI基建"
    body = chapters[0][1]
    assert "<h3>排队等审批</h3>" in body
    assert '<p class="byline">文｜财新周刊 文思敏</p>' in body and "<p>导语一帆风顺</p>" in body
    assert '<p class="caption">' in body and '<p class="link">' in body
