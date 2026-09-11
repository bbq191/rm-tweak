"""koreader 子命令对假网关：pull 落快照、diff 走 dry_run、sync 写入并同步字体。"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import pytest  # noqa: E402
from test_cli import FakeGateway, run, serve  # noqa: E402


class KoGateway(FakeGateway):

    def do_GET(self):
        if self.path.startswith("/api/koreader/config/"):
            b = b"return { wf_level = 3 }\n"
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(b)))
            self.end_headers()
            self.wfile.write(b)
            return
        if self.path == "/api/koreader/fonts":
            return self._json(200, {"items": [{"name": "g.ttf", "bytes": 4096, "cjkPct": 90}]})
        return super().do_GET()

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        FakeGateway.received.append((self.path, self.headers.get("Content-Type", ""), body))
        if self.path.startswith("/api/koreader/config/"):
            dry = "dry_run=1" in self.path
            return self._json(200, {"file": "x", "dry_run": dry, "written": not dry, "changes": [{"path": "wf_level", "old": 3, "new": 1}], "backup": None if dry else "/bk/x"})
        if self.path.startswith("/api/koreader/fonts"):
            return self._json(200, {"ok": True, "items": [{"file": "f.ttf", "ok": True, "message": "ok"}]})
        return super().do_POST()

    def do_DELETE(self):
        FakeGateway.received.append((self.path, "DELETE", b""))
        return self._json(200, {"ok": True})



@pytest.fixture(scope="module")
def gateway():
    url, srv = serve(KoGateway)
    yield url
    srv.shutdown()


def _profile(tmp_path):
    p = tmp_path / "profile"
    p.mkdir()
    (p / "settings.reader.patch.lua").write_text("return { wf_level = 1 }")
    (p / "defaults.custom.lua").write_text("return {}")
    font = tmp_path / "f.ttf"
    font.write_bytes(b"\x00\x01\x00\x00")
    (p / "fonts.txt").write_text(f"# c\n{font}\n")
    return p


def test_pull_writes_snapshot(gateway, tmp_path, capsys):
    out = tmp_path / "snap"
    rc, o = run(["koreader", "pull", "--out", str(out)], gateway, capsys)
    assert rc == 0 and (out / "settings.reader.lua").read_text().startswith("return")
    assert (out / "gestures.lua").is_file()


def test_diff_uses_dry_run_and_sync_writes(gateway, tmp_path, capsys):
    p = _profile(tmp_path)
    FakeGateway.received.clear()
    rc, o = run(["koreader", "diff", "--profile", str(p)], gateway, capsys)
    assert rc == 0 and "wf_level: 3 → 1" in o and "gestures" in o
    assert all("dry_run=1" in r[0] for r in FakeGateway.received)
    FakeGateway.received.clear()
    rc, o = run(["koreader", "sync", "--profile", str(p), "--fonts"], gateway, capsys)
    assert rc == 0 and "已写入" in o and "字体 f.ttf" in o
    assert any(r[0] == "/api/koreader/config/settings" and r[1].startswith("text/plain") for r in FakeGateway.received)
    assert any(r[0] == "/api/koreader/fonts" for r in FakeGateway.received)


def test_font_add_ls_rm(gateway, tmp_path, capsys):
    f = tmp_path / "h.ttf"
    f.write_bytes(b"\x00\x01\x00\x00")
    FakeGateway.received.clear()
    rc, out = run(["koreader", "font", "add", str(f)], gateway, capsys)
    assert rc == 0 and "f.ttf: ok" in out and FakeGateway.received[-1][0] == "/api/koreader/fonts"
    rc, out = run(["koreader", "font", "ls"], gateway, capsys)
    assert rc == 0 and "g.ttf" in out
    rc, out = run(["koreader", "font", "rm", "g.ttf"], gateway, capsys)
    assert rc == 0 and FakeGateway.received[-1] == ("/api/koreader/fonts/g.ttf", "DELETE", b"")
