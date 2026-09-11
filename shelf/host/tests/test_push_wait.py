"""`shelf push --wait`：设备睡着（`/health` 不通）时的探活/等待/放弃逻辑。探活打桩在 transport 上，sleep/时钟打桩不真等。"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from shelf_cli import calibre_bridge as cb  # noqa: E402
from shelf_cli import transport as tr  # noqa: E402
from shelf_cli.commands import push  # noqa: E402
from test_cli import FakeGateway, gateway, run  # noqa: E402,F401


class Probe:
    def __init__(self, answers):
        self.answers = list(answers)
        self.calls = 0

    def __call__(self):
        self.calls += 1
        return self.answers.pop(0) if self.answers else True


def test_ensure_reachable_without_wait_prints_hint(capsys):
    t = type("T", (), {"reachable": Probe([False])})()
    assert push.ensure_reachable(t, None) is False
    out = capsys.readouterr().out
    assert "点亮屏幕" in out and "--wait" in out


def test_ensure_reachable_waits_until_awake(capsys):
    t = type("T", (), {"reachable": Probe([False, False, False, True])})()
    clock = iter(range(0, 1000, 5))
    slept = []
    ok = push.ensure_reachable(t, 600, sleep=slept.append, clock=lambda: next(clock))
    assert ok and slept == [5, 5, 5]
    out = capsys.readouterr().out
    assert "每 5 秒探一次" in out and "设备醒了" in out


def test_ensure_reachable_gives_up_on_timeout(capsys):
    t = type("T", (), {"reachable": Probe([False] * 100)})()
    clock = iter(range(0, 10000, 5))
    ok = push.ensure_reachable(t, 20, sleep=lambda s: None, clock=lambda: next(clock))
    assert ok is False
    assert "还没醒" in capsys.readouterr().out


def test_push_stops_before_upload_when_device_asleep(gateway, tmp_path, capsys, monkeypatch):
    for n in ("a.epub", "b.epub"):
        (tmp_path / n).write_bytes(b"PK")
    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    monkeypatch.setattr(tr.HttpTransport, "reachable", lambda self, timeout=3.0: False)
    FakeGateway.received.clear()
    rc, out = run(["push", str(tmp_path / "a.epub"), str(tmp_path / "b.epub")], gateway, capsys)
    assert rc == 2 and "点亮屏幕" in out and "未上传：a.epub, b.epub" in out
    assert FakeGateway.received == [], "探活不通就不该碰上传接口"


def test_push_wait_then_uploads(gateway, tmp_path, capsys, monkeypatch):
    (tmp_path / "a.epub").write_bytes(b"PK")
    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    answers = iter([False, True])
    monkeypatch.setattr(tr.HttpTransport, "reachable", lambda self, timeout=3.0: next(answers))
    monkeypatch.setattr(push.time, "sleep", lambda s: None)
    FakeGateway.received.clear()
    rc, out = run(["push", "--wait", "60", str(tmp_path / "a.epub")], gateway, capsys)
    assert rc == 0 and "设备醒了" in out and "✓" in out
    assert FakeGateway.received[-1][0] == "/api/books/staging"


def test_real_transport_reachable_against_fake_gateway(gateway):
    t = tr.HttpTransport(gateway, password="pw")
    assert t.reachable() is True, "假网关对 /health 回 401/404 也算在线"
    assert tr.HttpTransport("http://127.0.0.1:9", password="pw").reachable(timeout=1) is False
