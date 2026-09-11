"""CLI 测试：起本地 http.server 假网关（FakeGateway），打真 HttpTransport；纯 stdlib。"""
from __future__ import annotations

import json
import sys
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from shelf_cli import __main__ as cli  # noqa: E402
from shelf_cli import config as cfgmod  # noqa: E402
from shelf_cli import paths as pathsmod  # noqa: E402
from shelf_cli import transport as tr  # noqa: E402
from shelf_cli.commands import doctor  # noqa: E402


class FakeGateway(BaseHTTPRequestHandler):
    services = [
        {"name": "gateway", "port": 443, "label": "秘密花园", "version": "0.1.0", "pid": 1},
        {"name": "font-serve", "port": 8792, "label": "字体", "version": "0.1.0", "pid": 2, "ui": {"title": "字体", "order": 30}},
    ]
    received: list = []
    must_change = False
    staging_items: list = []  # 测过 push 重跑跳过已存在文件（同名同大小）后须手动清回 []，见 test_push.py

    def _json(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def _authed(self):
        h = self.headers.get("Authorization", "")
        return h == "Basic " + __import__("base64").b64encode(b"shelf:pw").decode()

    def do_GET(self):
        if not self._authed():
            return self._json(401, {"ok": False, "message": "需要密码"})
        if self.path == "/api/services":
            return self._json(200, {"services": self.services})
        if self.path == "/api/manage":
            return self._json(200, {"modules": [{"seg": "fonts", "service": "font-serve"}, {"seg": "books", "service": "book-serve"}]})
        if self.path == "/api/fonts/health":
            return self._json(200, {"ok": True, "service": "font-serve", "version": "0.1.0"})
        if self.path == "/api/books/inbox":
            return self._json(200, {"items": [{"name": "bad.epub", "state": "failed", "bytes": 12, "reason": "质量门未过：双 id"}]})
        if self.path == "/api/books/staging":
            return self._json(200, {"items": self.staging_items, "freeBytes": 999999999})
        return self._json(404, {"ok": False, "message": "not found"})

    def do_POST(self):
        if not self._authed():
            return self._json(401, {"ok": False, "message": "需要密码"})
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        FakeGateway.received.append((self.path, self.headers.get("Content-Type", ""), body))
        if self.path == "/password":
            new = json.loads(body).get("new", "")
            if len(new) < 6:
                return self._json(400, {"ok": False, "message": "密码至少 6 位"})
            return self._json(200, {"ok": True, "message": "密码已更新"})
        if self.path.startswith("/api/books") and FakeGateway.must_change:
            return self._json(403, {"ok": False, "message": "首次登录必须先改密码"})
        if self.path == "/api/books/inbox/retry":
            return self._json(200, {"ok": True, "items": [{"name": json.loads(body)["name"], "ok": True, "message": "已重投"}]})
        if self.path == "/api/books/inbox/delete":
            return self._json(200, {"ok": True})
        return self._json(200, {"ok": True, "items": [{"name": "x", "ok": True}]})

    def log_message(self, *a):  # 静音
        pass


def serve(handler):
    """起一个假网关（独立线程），返回 (base_url, server)。各测试文件用自己的 Handler 子类，互不污染。"""
    srv = HTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return f"http://127.0.0.1:{srv.server_port}", srv


@pytest.fixture(scope="module")
def gateway():
    url, srv = serve(FakeGateway)
    yield url
    srv.shutdown()


def run(argv, base_url, capsys, password="pw"):
    rc = cli.main(argv, transport_factory=lambda c: tr.HttpTransport(base_url, user="shelf", password=password))
    return rc, capsys.readouterr().out


def test_services_lists_registry(gateway, capsys):
    rc, out = run(["services"], gateway, capsys)
    assert rc == 0
    assert "font-serve" in out and "tab=字体" in out


def test_status_probes_each_service(gateway, capsys):
    rc, out = run(["status"], gateway, capsys)
    assert rc == 0
    assert "font-serve" in out and "ok" in out


def test_unreachable_gateway_is_reported(capsys):
    rc, out = run(["services"], "http://127.0.0.1:9", capsys)
    assert rc == 2


def test_wrong_password_is_401(gateway, capsys):
    rc, out = run(["services"], gateway, capsys, password="nope")
    assert rc == 2


def test_multipart_upload_encoding(gateway, tmp_path):
    f = tmp_path / "中 文.epub"
    f.write_bytes(b"PK\x03\x04data")
    t = tr.HttpTransport(gateway, user="shelf", password="pw")
    FakeGateway.received.clear()
    j = t.post_files("/api/books/staging", [f], {"x": "1"})
    assert j["ok"]
    path, ctype, body = FakeGateway.received[0]
    assert path == "/api/books/staging?x=1"
    assert ctype.startswith("multipart/form-data; boundary=")
    assert b"filename*=UTF-8''%E4%B8%AD%20%E6%96%87.epub" in body
    assert b"PK\x03\x04data\r\n--" in body


def test_config_defaults_and_overrides(tmp_path):
    p = pathsmod.Paths({"HOME": str(tmp_path), "XDG_CONFIG_HOME": str(tmp_path / "cfg")})
    assert p.config_file == tmp_path / "cfg" / "shelf" / "config.toml"
    p.config.mkdir(parents=True)
    p.config_file.write_text('host = "192.168.1.5"\nretired_key = "x"\nbogus = 1\n')
    c = cfgmod.load(p, {"port": 9999, "host": None})
    assert (c.host, c.port, c.split_pdf_mb) == ("192.168.1.5", 9999, 60), "未知键忽略"
    assert c.base_url == "https://192.168.1.5:9999"
    assert cfgmod.load(p, {"scheme": "http"}).base_url.startswith("http://")


def test_xdg_relative_paths_are_ignored(tmp_path):
    p = pathsmod.Paths({"HOME": str(tmp_path), "XDG_DATA_HOME": "rel/x"})
    assert p.data == tmp_path / ".local/share" / "shelf"


def test_doctor_detects_venv_hijack():
    assert doctor.venv_hijack({"VIRTUAL_ENV": "/x"})
    assert doctor.venv_hijack({"PATH": "/repo/.venv/bin:/usr/bin"})
    assert not doctor.venv_hijack({"PATH": "/usr/bin"})


def test_passwd_command_posts_new_password(gateway, capsys):
    rc, out = run(["passwd", "--new", "longer1"], gateway, capsys)
    assert rc == 0 and "已更新" in out
    assert FakeGateway.received[-1][0] == "/password"
    assert json.loads(FakeGateway.received[-1][2]) == {"new": "longer1"}
    rc, _ = run(["passwd", "--new", "abc"], gateway, capsys)
    assert rc != 0


def test_must_change_403_is_explained(gateway, capsys, tmp_path, monkeypatch):
    from shelf_cli.commands import push

    f = tmp_path / "a.epub"
    f.write_bytes(b"PK")
    monkeypatch.setattr(push.cb, "has_calibre", lambda: False)  # 原样落母版库，403 命中
    FakeGateway.must_change = True
    try:
        rc, out = run(["push", str(f)], gateway, capsys)
    finally:
        FakeGateway.must_change = False
    assert rc != 0
    assert "shelf passwd" in capsys.readouterr().err + out


def test_inbox_list_shows_failed_reason(gateway, capsys):
    rc, out = run(["inbox"], gateway, capsys)
    assert rc == 0
    assert "bad.epub" in out and "质量门未过" in out


def test_inbox_retry_and_delete(gateway, capsys):
    rc, out = run(["inbox", "--retry", "bad.epub"], gateway, capsys)
    assert rc == 0 and "已重投" in out
    assert FakeGateway.received[-1][0] == "/api/books/inbox/retry"
    rc, out = run(["inbox", "--delete", "bad.epub"], gateway, capsys)
    assert rc == 0 and "已删除" in out
    assert FakeGateway.received[-1][0] == "/api/books/inbox/delete"
