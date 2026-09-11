"""`shelf doctor --render`：探针 EPUB 结构、judge 纯函数、CLI 编排（假网关给出渲染自检结果与 PDF，量测函数打桩）。"""
from __future__ import annotations

import json
import sys
import urllib.parse
import zipfile
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "calibre"))
import render_measure as rm  # noqa: E402
import render_probe as rp  # noqa: E402
from shelf_cli import calibre_bridge as cb  # noqa: E402
from shelf_cli.commands import doctor  # noqa: E402
from test_cli import FakeGateway, run, serve  # noqa: E402,F401


def test_probe_epub_has_sentinels_css_and_two_chapters(tmp_path):
    p = rp.build(tmp_path / "探针.epub", "探针")
    with zipfile.ZipFile(p) as z:
        names = z.namelist()
        assert names[0] == "mimetype" and z.getinfo("mimetype").compress_type == zipfile.ZIP_STORED
        css = z.read("OEBPS/cangjie-wash.css").decode()
        assert css == rp.CSS and "text-indent:1.2em;" in css and ".cj-flush{text-indent:0.01em;" in css, "配方字面（含尾分号）"
        c1 = z.read("OEBPS/text/c1.xhtml").decode()
        c2 = z.read("OEBPS/text/c2.xhtml").decode()
        for s, kind in rp.SENTINELS.items():
            body = c1 if s in c1 else c2
            assert s in body, s
            tag = '<div class="cj-flush">' if kind == "flush" else "<p>"
            assert f"{tag}{s} " in body, f"{s} 应在 {tag} 里"
        assert 'href="../cangjie-wash.css"' in c1 and 'href="../cangjie-wash.css"' in c2
        assert "<h1>Chapter One</h1>" in c1 and "<h1>第二章</h1>" in c2


def test_visual_lines_merge_cjk_fragments_then_measure():
    """真机页 3 的几何：拉丁哨兵与 CJK 回退字体各成一段（y 差 2pt）；合并后首行缩进=前行 x − 续行 x。"""
    raw = [
        (73.3, 17.8, 12.05, "PFLUSH3 "),
        (75.4, 79.9, 12.05, "藏日天咸"),
        (91.2, 17.8, 12.05, "阳冈收李"),
        (184.3, 32.0, 12.05, "PINDENT4 "),
        (186.4, 104.0, 12.05, "致鳞水吕"),
        (202.3, 17.8, 12.05, "光日宿秋"),
    ]
    lines = rm.visual_lines(raw)
    assert [len(l["texts"]) for l in lines] == [2, 1, 2, 1]
    rows = rm.rows_from_lines(lines, 3)
    assert [(r["sentinel"], r["indent_pt"]) for r in rows] == [("PFLUSH3", 0.0), ("PINDENT4", 14.2)]
    assert abs(rows[1]["em"] - 1.178) < 0.01


def test_judge_flags_missing_wrong_indent_and_passes_good():
    rows = [
        {"sentinel": "PFLUSH1", "page": 1, "indent_pt": 0.3, "size_pt": 12.1, "em": 0.025},
        {"sentinel": "PINDENT1", "page": 1, "indent_pt": 14.2, "size_pt": 12.1, "em": 1.174},
    ]
    ok, problems = rm.judge(rows, {"PFLUSH1": "flush", "PINDENT1": "indent"})
    assert ok and problems == []
    rows[0]["em"] = 1.17
    rows.append({"sentinel": "PINDENT2", "page": 1, "indent_pt": 0.0, "size_pt": 12.1, "em": 0.0})
    ok, problems = rm.judge(rows, {"PFLUSH1": "flush", "PINDENT1": "indent", "PINDENT2": "indent", "PFLUSH3": "flush"})
    assert not ok
    assert any("PFLUSH1" in p and "期望顶格" in p for p in problems)
    assert any("PINDENT2" in p and "期望缩进" in p for p in problems)
    assert any("PFLUSH3" in p and "未量到" in p for p in problems)


class RenderGateway(FakeGateway):
    """母版库 + 渲染自检 + 渲染缓存 PDF 的假网关。"""

    landed = "书架自检探针 x.epub"

    def do_GET(self):
        if not self._authed():
            return self._json(401, {"ok": False})
        if self.path == "/api/books/staging":
            return self._json(200, {"items": [{"name": self.landed, "format": "epub", "delivered": {"native": 1, "render": {"uuid": "1234abcd-0000-0000-0000-000000000000", "pages": 3, "expected": 3, "status": "ok"}}}]})
        if self.path.startswith("/api/books/staging/render/"):
            b = b"%PDF-1.4 fake"
            self.send_response(200)
            self.send_header("Content-Type", "application/pdf")
            self.send_header("Content-Length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)
            return None
        return super().do_GET()

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        RenderGateway.received.append((self.path, self.headers.get("Content-Type", ""), body))
        if self.path == "/api/books/staging":
            return self._json(200, {"ok": True, "items": [{"name": self.landed, "ok": True, "message": "已入母版库"}]})
        if self.path == "/api/books/staging/deliver":
            return self._json(200, {"ok": True, "message": "已投入原生书库"})
        return self._json(200, {"ok": True})


@pytest.fixture(scope="module")
def render_gateway():
    url, srv = serve(RenderGateway)
    yield url
    srv.shutdown()


def test_doctor_render_end_to_end_with_stubbed_measure(render_gateway, capsys, monkeypatch):
    measured = {}

    def fake_measure(pdf: Path):
        measured["bytes"] = pdf.read_bytes()
        return {"rows": [{"sentinel": s, "page": 1, "indent_pt": 0.0 if k == "flush" else 14.2, "size_pt": 12.1, "em": 0.0 if k == "flush" else 1.17} for s, k in rp.SENTINELS.items()], "ok": True, "problems": []}

    monkeypatch.setattr(cb, "render_measure", fake_measure)
    RenderGateway.received.clear()
    rc, out = run(["doctor", "--render"], render_gateway, capsys)
    assert rc == 0, out
    assert "PASS" in out and "PFLUSH1" in out and "渲染自检      : ok" in out
    assert measured["bytes"].startswith(b"%PDF")
    paths = [p for p, _, _ in RenderGateway.received]
    assert paths[0] == "/api/books/staging" and paths[1] == "/api/books/staging/deliver" and paths[-1] == "/api/books/staging/delete"
    assert "/api/books/trash/add" in paths and "排队进原生回收站" in out
    trash = json.loads(RenderGateway.received[paths.index("/api/books/trash/add")][2])
    assert trash["uuid"].startswith("1234abcd") and trash["name"].startswith("书架自检探针")
    deliver = json.loads(RenderGateway.received[1][2])
    assert deliver["folder"] == doctor.RENDER_FOLDER and deliver["keep"] is True and deliver["name"] == RenderGateway.landed
    assert urllib.parse.quote("书架自检探针").encode() in RenderGateway.received[0][2], "探针文件名 UTF-8 编码进 multipart"


def test_doctor_render_fail_lists_problems(render_gateway, capsys, monkeypatch):
    monkeypatch.setattr(cb, "render_measure", lambda pdf: {"rows": [], "ok": False, "problems": ["PFLUSH1: 未量到（段落没渲染或没换行）"]})
    rc, out = run(["doctor", "--render"], render_gateway, capsys)
    assert rc == 1 and "FAIL" in out and "✗ PFLUSH1" in out
