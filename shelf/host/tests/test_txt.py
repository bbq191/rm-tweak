"""TXT → EPUB 切章：编码探测、卷/章两级、误判防护、硬切回退、全角空格剥除、书名作者解析；push 路由。"""
from __future__ import annotations

import json
import sys
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "calibre"))
import txt_to_epub as t2e  # noqa: E402
from shelf_cli import calibre_bridge as cb  # noqa: E402
from shelf_cli.commands import push  # noqa: E402
from test_cli import FakeGateway, gateway, run  # noqa: E402,F401

NOVEL = "　　开篇的话。\n第一卷 风起\n第一章 初见\n　　他说：第三章说过的事我不记得了，这一句很长很长不该被当成标题。\n\n第二章：重逢\n　　又见面了。\n第二卷 云涌\n第3章 - 别离\n走了。\n番外 番外一\n小故事。\n"


def test_classify_headings_and_guards():
    assert t2e.classify("第一卷 风起") == ("vol", "第一卷 风起")
    assert t2e.classify("第二章：重逢") == ("ch", "第二章 重逢")
    assert t2e.classify("第3章 - 别离") == ("ch", "第3章 别离")
    assert t2e.classify("　　第十二回") == ("ch", "第十二回")
    assert t2e.classify("楔子") == ("ch", "楔子")
    assert t2e.classify("他说：第三章说过的事我不记得了，这一句很长很长不该被当成标题。") is None
    assert t2e.classify("第三章说过" * 10) is None, "超长不算标题"


def test_split_two_levels_and_leading_fullwidth_stripped():
    chapters, detected = t2e.split_chapters(NOVEL)
    assert detected
    assert [(c["kind"], c["title"]) for c in chapters] == [("ch", "开头"), ("vol", "第一卷 风起"), ("ch", "第一章 初见"), ("ch", "第二章 重逢"), ("vol", "第二卷 云涌"), ("ch", "第3章 别离"), ("ch", "番外 番外一")]
    assert chapters[0]["paras"] == ["开篇的话。"], "全角空格剥掉"
    assert chapters[3]["paras"] == ["又见面了。"]


def test_leading_contents_listing_is_dropped_but_real_headings_kept():
    text = "第一部　一天的國王　1\n第二部　羅卡德原則　9\n第一章　開場　1\n第一部　一天的國王\n第一章　開場\n正文一。\n第二部　羅卡德原則\n第二章\n正文二。\n"
    chapters, _ = t2e.split_chapters(text)
    assert [(c["kind"], c["title"]) for c in chapters] == [("vol", "第一部 一天的國王"), ("ch", "第一章 開場"), ("vol", "第二部 羅卡德原則"), ("ch", "第二章")]
    assert chapters[1]["paras"] == ["正文一。"]


def test_hard_split_when_no_headings():
    text = "\n".join("一二三四五六七八九十" * 50 for _ in range(30))  # 每段 500 字，共 15000 字
    chapters, detected = t2e.split_chapters(text)
    assert not detected and [c["title"] for c in chapters] == ["第 1 部分", "第 2 部分"]
    assert sum(len(c["paras"]) for c in chapters) == 30


def test_decode_gb18030_utf8_bom_and_title_author(tmp_path):
    assert t2e.decode("第一章 你好".encode("gb18030")) == ("第一章 你好", "gb18030")
    assert t2e.decode("﻿第一章".encode("utf-8")) == ("第一章", "utf-8-sig")
    assert t2e.decode("第一章".encode("utf-8")) == ("第一章", "utf-8")
    assert t2e.title_author("斗破苍穹 - 天蚕土豆") == ("斗破苍穹", "天蚕土豆")
    assert t2e.title_author("斗破苍穹（天蚕土豆）") == ("斗破苍穹", "天蚕土豆")
    assert t2e.title_author("斗破苍穹") == ("斗破苍穹", "")


def test_convert_builds_epub_with_nested_nav(tmp_path):
    src = tmp_path / "书 - 作者.txt"
    src.write_bytes(NOVEL.encode("gb18030"))
    meta = t2e.convert(src, tmp_path / "out")
    assert meta["chapters"] == 5 and meta["volumes"] == 2 and meta["encoding"] == "gb18030" and meta["detected"]
    assert meta["title"] == "书" and meta["author"] == "作者"
    with zipfile.ZipFile(meta["out"]) as z:
        assert z.namelist()[0] == "mimetype"
        nav = z.read("OEBPS/nav.xhtml").decode()
        assert nav.count("<ol>") == 3 and "<ol></ol>" not in nav, "根 ol + 两卷各一个子 ol"
        assert nav.index("第一卷") < nav.index("第一章") < nav.index("第二卷") < nav.index("第3章")
        opf = z.read("OEBPS/content.opf").decode()
        assert "<dc:title>书</dc:title>" in opf and "<dc:creator>作者</dc:creator>" in opf
        c3 = z.read("OEBPS/text/c3.xhtml").decode()
        assert "<h2>第一章 初见</h2>" in c3 and "<p>他说：第三章说过" in c3 and "　" not in c3
        assert "<h1>第一卷 风起</h1>" in z.read("OEBPS/text/c2.xhtml").decode()


def test_push_txt_routes_through_txt_to_epub_then_wash(gateway, tmp_path, capsys, monkeypatch):
    txt = tmp_path / "小说.txt"
    txt.write_text("第一章 开始\n正文。\n", encoding="utf-8")
    epub = tmp_path / "小说.epub"
    epub.write_bytes(b"PK\x03\x04")
    calls = []
    monkeypatch.setattr(cb, "has_calibre", lambda: True)
    monkeypatch.setattr(cb, "txt_to_epub", lambda src, work: (calls.append("txt") or (epub, {"chapters": 1, "volumes": 0, "encoding": "utf-8", "detected": True})))
    monkeypatch.setattr(cb, "wash", lambda src, work, **kw: (calls.append(("wash", src.name, kw.get("env"))) or src))
    monkeypatch.setattr(push, "_gate", lambda out, args: None)
    FakeGateway.received.clear()
    rc, out = run(["push", str(txt)], gateway, capsys)
    assert rc == 0 and "TXT 切章 → EPUB（1 章" in out
    assert calls[0] == "txt" and calls[1][0] == "wash" and calls[1][1] == "小说.epub" and calls[1][2]["WASH_AUTOTOC"] == "0"
    assert FakeGateway.received[-1][0] == "/api/books/staging"
    assert push.plan(txt, type("A", (), {"no_optimize": False, "comic": False, "no_comic": False})(), True) == "wash"


def test_push_txt_without_calibre_goes_raw(gateway, tmp_path, capsys, monkeypatch):
    txt = tmp_path / "小说.txt"
    txt.write_text("第一章\n正文", encoding="utf-8")
    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    rc, out = run(["push", str(txt)], gateway, capsys)
    assert rc == 0 and "原样→" in out


def test_json_last_line_contract(tmp_path, capsys):
    src = tmp_path / "a.txt"
    src.write_text("第一章\n正文", encoding="utf-8")
    sys.argv = ["txt_to_epub.py", str(src), str(tmp_path / "o")]
    assert t2e.main() == 0
    d = json.loads(capsys.readouterr().out.strip().splitlines()[-1])
    assert d["chapters"] == 1 and Path(d["out"]).is_file()
