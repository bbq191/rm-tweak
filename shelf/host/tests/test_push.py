"""push 端到端（假网关）：落母版库 / 无直投逃生 / PDF 重排 / 漫画→CBZ / 分卷 / calibre 桥环境清洗。"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from shelf_cli import calibre_bridge as cb  # noqa: E402
from shelf_cli import comic, pdfsplit  # noqa: E402
from shelf_cli.commands import push  # noqa: E402
from test_cli import FakeGateway, gateway, run  # noqa: E402,F401


def test_clean_env_strips_venv():
    e = cb.clean_env({"VIRTUAL_ENV": "/r/.venv", "PATH": "/r/.venv/bin:/usr/bin:/bin"})
    assert "VIRTUAL_ENV" not in e
    assert e["PATH"] == "/usr/bin:/bin"


def test_split_volumes_math(tmp_path):
    assert pdfsplit.volumes_for(10 * 2**20, 60) == 1
    assert pdfsplit.volumes_for(61 * 2**20, 60) == 2
    assert pdfsplit.volumes_for(300 * 2**20, 60) == 5
    f = tmp_path / "x.pdf"
    f.write_bytes(b"%PDF" + b"\0" * 10)
    assert not pdfsplit.needs_split(f, 60)
    assert not pdfsplit.needs_split(f, 0)


def test_push_lands_in_staging(gateway, tmp_path, capsys, monkeypatch):
    (tmp_path / "b.epub").write_bytes(b"PK")
    monkeypatch.setattr(cb, "has_calibre", lambda: False)  # 无 Calibre → 原样落母版库
    FakeGateway.received.clear()
    rc, out = run(["push", str(tmp_path / "b.epub")], gateway, capsys)
    assert rc == 0 and "母版库" in out and "✓" in out
    assert FakeGateway.received[-1][0] == "/api/books/staging"


def test_push_rerun_skips_already_staged_file(gateway, tmp_path, capsys, monkeypatch):
    """2026-09-13 Reddit 用户报告的场景：批量推送中途失败后原样重跑，已经成功落地的那份不该
    再传一次（否则撞 unique_path 的"同名不覆盖"变出 1_x 副本）。同名同大小 → 跳过；同名不同
    大小（内容真的换了）→ 照样传，不能因为名字一样就误伤真正想更新的文件。"""
    f = tmp_path / "a.epub"
    f.write_bytes(b"PK" * 5)  # 10 字节
    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    try:
        FakeGateway.staging_items = [{"name": "a.epub", "bytes": 10}]  # 假装上一轮已经成功落地
        FakeGateway.received.clear()
        rc, out = run(["push", str(f)], gateway, capsys)
        assert rc == 0 and "已在母版库（同名同大小），跳过重传" in out
        assert not FakeGateway.received, "同名同大小不该再 POST 一次"

        FakeGateway.staging_items = [{"name": "a.epub", "bytes": 999}]  # 名字一样但大小对不上＝内容真的不同
        FakeGateway.received.clear()
        rc, out = run(["push", str(f)], gateway, capsys)
        assert rc == 0 and "跳过重传" not in out
        assert FakeGateway.received and FakeGateway.received[-1][0] == "/api/books/staging", "大小对不上要照常传"
    finally:
        FakeGateway.staging_items = []  # module-scope fixture 复用同一个 FakeGateway，别漏给后面的测试


def test_push_batch_partial_failure_then_rerun_lands_once(gateway, tmp_path, capsys, monkeypatch):
    """更贴近 Reddit 原话的完整场景：`push a.epub b.epub` 里 a 先成功、b 失败，原样重跑整条命令——
    a 不重复落库，b 补传成功。"""
    a = tmp_path / "a.epub"
    a.write_bytes(b"A" * 4)
    b = tmp_path / "b.epub"
    b.write_bytes(b"BB" * 3)
    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    try:
        FakeGateway.staging_items = [{"name": "a.epub", "bytes": 4}]  # 第一轮 a 已经成功、b 那次失败
        FakeGateway.received.clear()
        rc, out = run(["push", str(a), str(b)], gateway, capsys)
        assert rc == 0
        assert "a.epub: 已在母版库（同名同大小），跳过重传" in out
        posted = [r[0] for r in FakeGateway.received]
        assert posted == ["/api/books/staging"], "只有 b 应该被 POST，a 跳过"
        assert b"b.epub" in FakeGateway.received[-1][2] and b"a.epub" not in FakeGateway.received[-1][2]
    finally:
        FakeGateway.staging_items = []


def test_push_has_no_direct_escape(gateway, tmp_path, capsys):
    # 规则与网页一致：所有书只落母版库，--direct 不存在
    (tmp_path / "b.epub").write_bytes(b"PK")
    import pytest

    with pytest.raises(SystemExit):
        run(["push", "--direct", str(tmp_path / "b.epub")], gateway, capsys)


def test_push_native_pdf_reflow_to_staging(gateway, tmp_path, capsys, monkeypatch):
    pdf = tmp_path / "paper.pdf"
    pdf.write_bytes(b"%PDF-1.4 fake")
    epub = tmp_path / "paper.epub"
    epub.write_bytes(b"PK\x03\x04reflowed")
    monkeypatch.setattr(cb, "has_calibre", lambda: True)
    monkeypatch.setattr(cb, "reflow_pdf", lambda src, work: (epub, "epub"))
    monkeypatch.setattr(cb, "wash", lambda src, work, **kw: src)  # 洗书直返（不跑 Calibre）
    monkeypatch.setattr(push, "_gate", lambda out, args: None)
    FakeGateway.received.clear()
    rc, out = run(["push", str(pdf)], gateway, capsys)
    assert rc == 0 and "结构化重排 → EPUB" in out
    assert FakeGateway.received[-1][0] == "/api/books/staging", FakeGateway.received
    assert "已入母版库。去 " in out and "传书 → 母版库" in out, "推完要给网页去向提示"


def test_push_keep_spacing_passes_env(gateway, tmp_path, capsys, monkeypatch):
    (tmp_path / "poem.epub").write_bytes(b"PK")
    seen = {}
    monkeypatch.setattr(cb, "has_calibre", lambda: True)
    monkeypatch.setattr(cb, "wash", lambda src, work, env=None: seen.__setitem__("env", env) or src)
    monkeypatch.setattr(push, "_gate", lambda out, args: None)
    FakeGateway.received.clear()
    rc, _ = run(["push", "--keep-spacing", str(tmp_path / "poem.epub")], gateway, capsys)
    assert rc == 0 and seen["env"] == {"WASH_KEEP_PARA_SPACING": "1"}
    rc, _ = run(["push", str(tmp_path / "poem.epub")], gateway, capsys)
    assert seen["env"] is None, "不带 --keep-spacing 不设环境"


def test_plan_no_calibre_epub_goes_optimize_only_regardless_of_calibre(tmp_path):
    """--no-calibre：EPUB 走 optimize-only，装没装 Calibre 都一样——这条路径本来就是为了不依赖它。"""
    args = type("A", (), {"no_optimize": False, "no_calibre": True, "comic": False, "no_comic": False})()
    epub = tmp_path / "b.epub"
    assert push.plan(epub, args, True) == "optimize-only"
    assert push.plan(epub, args, False) == "optimize-only"


def test_plan_no_calibre_non_epub_falls_back_to_raw(tmp_path):
    """--no-calibre 对非 EPUB 没法只靠这条路径转格式，原样传（不是报错，也不是偷偷还是走 Calibre）。"""
    args = type("A", (), {"no_optimize": False, "no_calibre": True, "comic": False, "no_comic": False})()
    assert push.plan(tmp_path / "b.azw3", args, True) == "raw"
    assert push.plan(tmp_path / "b.pdf", args, True) == "raw"


def test_plan_no_calibre_takes_priority_over_comic(tmp_path):
    """漫画 EPUB 也一样退化成 optimize-only，不因为看起来像漫画就偷偷换回 Calibre 路径。"""
    args = type("A", (), {"no_optimize": False, "no_calibre": True, "comic": True, "no_comic": False})()
    assert push.plan(tmp_path / "manga.epub", args, True) == "optimize-only"


def test_push_no_calibre_calls_optimize_only_not_wash(gateway, tmp_path, capsys, monkeypatch):
    """端到端：--no-calibre 落母版库这条路，`cb.wash`（走 Calibre 的那个）完全不该被调用。"""
    (tmp_path / "b.epub").write_bytes(b"PK")
    calls = {"optimize_only": 0, "wash": 0}

    def fake_optimize_only(src, out, keep_spacing=False):
        calls["optimize_only"] += 1
        out.write_bytes(b"PK-optimized")
        return out

    def fake_wash(*a, **kw):
        calls["wash"] += 1
        raise AssertionError("--no-calibre 不该调 cb.wash")

    monkeypatch.setattr(cb, "has_calibre", lambda: True)  # 装了也不该用
    monkeypatch.setattr(cb, "optimize_only", fake_optimize_only)
    monkeypatch.setattr(cb, "wash", fake_wash)
    monkeypatch.setattr(push, "_gate", lambda out, args: None)
    FakeGateway.received.clear()
    rc, out = run(["push", "--no-calibre", str(tmp_path / "b.epub")], gateway, capsys)
    assert rc == 0 and calls == {"optimize_only": 1, "wash": 0}
    assert "纯优化（跳过 Calibre）→母版库" in out
    assert FakeGateway.received[-1][0] == "/api/books/staging"


def test_push_no_calibre_passes_keep_spacing(gateway, tmp_path, capsys, monkeypatch):
    (tmp_path / "poem.epub").write_bytes(b"PK")
    seen = {}

    def fake_optimize_only(src, out, keep_spacing=False):
        seen["keep_spacing"] = keep_spacing
        out.write_bytes(b"PK")
        return out

    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    monkeypatch.setattr(cb, "optimize_only", fake_optimize_only)
    monkeypatch.setattr(push, "_gate", lambda out, args: None)
    FakeGateway.received.clear()
    rc, _ = run(["push", "--no-calibre", "--keep-spacing", str(tmp_path / "poem.epub")], gateway, capsys)
    assert rc == 0 and seen["keep_spacing"] is True


def test_calibre_bridge_epub_optimize_bin_resolution(monkeypatch, tmp_path):
    """定位优先级：WASH_OPTIMIZE_BIN 覆盖 > PATH > 仓库内 target/release/epub-optimize > 找不到返回 None。"""
    monkeypatch.delenv("WASH_OPTIMIZE_BIN", raising=False)
    monkeypatch.setattr(cb.shutil, "which", lambda *a, **kw: None)
    monkeypatch.setattr(cb, "REPO_ROOT", tmp_path)
    assert cb.epub_optimize_bin() is None, "PATH 没有、仓库内也没编译产物 → None"
    local = tmp_path / "shelf" / "target" / "release" / "epub-optimize"
    local.parent.mkdir(parents=True)
    local.write_text("#!/bin/sh\n")
    assert cb.epub_optimize_bin() == str(local)
    monkeypatch.setattr(cb.shutil, "which", lambda *a, **kw: "/usr/local/bin/epub-optimize")
    assert cb.epub_optimize_bin() == "/usr/local/bin/epub-optimize", "PATH 优先于仓库内产物"
    monkeypatch.setenv("WASH_OPTIMIZE_BIN", "/custom/epub-optimize")
    assert cb.epub_optimize_bin() == "/custom/epub-optimize", "环境变量优先级最高"


def test_push_no_reflow_raw_to_staging(gateway, tmp_path, capsys, monkeypatch):
    pdf = tmp_path / "p.pdf"
    pdf.write_bytes(b"%PDF raw")
    called = {"n": 0}
    monkeypatch.setattr(cb, "has_calibre", lambda: True)
    monkeypatch.setattr(cb, "reflow_pdf", lambda *a: called.__setitem__("n", 1) or (pdf, "pdf"))
    monkeypatch.setattr(push, "_gate", lambda out, args: None)
    FakeGateway.received.clear()
    rc, out = run(["push", "--no-reflow", str(pdf)], gateway, capsys)
    assert rc == 0 and called["n"] == 0, "--no-reflow 不应调用 reflow"
    assert FakeGateway.received[-1][0] == "/api/books/staging"


def test_push_dry_run_and_missing_file(gateway, tmp_path, capsys):
    rc, out = run(["push", "-n", str(tmp_path / "nope.epub")], gateway, capsys)
    assert rc == 1 and "不是文件" in out
    (tmp_path / "a.epub").write_bytes(b"PK")
    FakeGateway.received.clear()
    rc, out = run(["push", "-n", str(tmp_path / "a.epub")], gateway, capsys)
    assert rc == 0 and "母版库" in out and not FakeGateway.received


def _palmdb(records: list[bytes]) -> bytes:
    """最小 PalmDB：78 字节头 + 记录表 + 记录体。"""
    n = len(records)
    head = bytearray(78)
    head[76:78] = n.to_bytes(2, "big")
    off = 78 + n * 8
    table = bytearray()
    for i, r in enumerate(records):
        table += off.to_bytes(4, "big") + bytes([0, 0, 0, i & 0xFF])
        off += len(r)
    return bytes(head) + bytes(table) + b"".join(records)


def test_comic_probe_palmdb_and_epub(tmp_path):
    jpg = b"\xff\xd8\xff" + b"\0" * 2000
    comic_book = tmp_path / "manga.azw3"
    comic_book.write_bytes(_palmdb([b"MOBIhdr" + b"\0" * 100] + [jpg] * 30))
    text_book = tmp_path / "novel.azw3"
    text_book.write_bytes(_palmdb([b"text record " * 200] * 40 + [jpg] * 2))
    assert comic.is_comic(comic_book) and not comic.is_comic(text_book)
    assert comic.is_comic(tmp_path / "x.cbz") and not comic.is_comic(tmp_path / "x.pdf")
    import zipfile

    def epub(path, pages, imgs_per_page, text_per_page):
        with zipfile.ZipFile(path, "w") as z:
            z.writestr("META-INF/container.xml", '<container><rootfile full-path="OEBPS/content.opf"/></container>')
            items = "".join(f'<item id="p{i}" href="p{i}.xhtml"/>' for i in range(pages))
            spine = "".join(f'<itemref idref="p{i}"/>' for i in range(pages))
            z.writestr("OEBPS/content.opf", f"<package><manifest>{items}</manifest><spine>{spine}</spine></package>")
            for i in range(pages):
                body = '<img src="i.jpg"/>' * imgs_per_page + "<p>" + "字" * text_per_page + "</p>"
                z.writestr(f"OEBPS/p{i}.xhtml", f"<html><head><style>p{{x}}</style></head><body>{body}</body></html>")
    epub(tmp_path / "c.epub", 10, 17, 30)      # Calibre 洗过的漫画：一页十几张图、几乎无字
    epub(tmp_path / "t.epub", 30, 1, 600)      # 文字书：每章一张插图、几百字
    assert comic.is_comic(tmp_path / "c.epub") and not comic.is_comic(tmp_path / "t.epub")


def test_push_comic_route_lands_cbz_only(gateway, tmp_path, capsys, monkeypatch):
    """体积超原生上限（本测试直接让 cbz_to_pdf 失败模拟）时只出 CBZ 进母版库，不产 PDF。"""
    src = tmp_path / "manga.azw3"
    src.write_bytes(b"x")
    monkeypatch.setattr(cb, "has_calibre", lambda: True)
    monkeypatch.setattr(comic, "is_comic", lambda p: True)
    monkeypatch.setattr(cb, "comic2cbz", lambda s, o: (o.write_bytes(b"PK"), o)[1])

    def _no_pdf(s, o):
        raise cb.CalibreError("mock: 这条测试不关心投原生 PDF")

    monkeypatch.setattr(cb, "cbz_to_pdf", _no_pdf)
    FakeGateway.received.clear()
    rc, out = run(["push", str(src)], gateway, capsys)  # 缺省过 16 灰；假字节让 comic_gray 失败 → 退回原图 CBZ 不挡推送
    assert rc == 0 and "漫画 CBZ→母版库" in out and "16 灰失败，按原图 CBZ 推" in out
    names = [r[0] for r in FakeGateway.received]
    assert names.count("/api/books/staging") == 1, "只有 CBZ 一次入库"
    assert b"manga.cbz" in FakeGateway.received[-1][2]
    # --no-comic 强制走洗书路；--no-optimize 原样
    monkeypatch.setattr(cb, "wash", lambda s, w, env=None: s)
    monkeypatch.setattr(push, "_gate", lambda o, a: None)
    rc, out = run(["push", "--no-comic", str(src)], gateway, capsys)
    assert rc == 0 and "洗书→母版库" in out
    rc, out = run(["push", "--no-optimize", str(src)], gateway, capsys)
    assert rc == 0 and "原样→母版库" in out
    # CBZ 本身就是终态：--no-eink-gray 原样入库，不需要 Calibre
    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    cbz = tmp_path / "v1.cbz"
    cbz.write_bytes(b"PK")
    rc, out = run(["push", "--no-eink-gray", str(cbz)], gateway, capsys)
    assert rc == 0 and "原样→母版库" in out


def test_native_limit_bytes_queries_device_then_falls_back(gateway, monkeypatch):
    """能查到设备的 nativeUploadLimitBytes 就用它；查不到（假网关没这个接口）退回静态兜底。"""
    from shelf_cli import transport as tr

    ctx = type("Ctx", (), {"transport": tr.HttpTransport(gateway, user="shelf", password="pw")})()
    assert push._native_limit_bytes(ctx) == push.NATIVE_LIMIT_FALLBACK_MB * 2**20, "假网关没实现这个接口，该退回兜底值"


def test_native_limit_fallback_mb_matches_rust_default(monkeypatch):
    """`NATIVE_LIMIT_FALLBACK_MB`（push.py 注释自称"跟 book-serve BookConfig::default() 一致，
    那边改了要手动同步"）跟 Rust 侧真实默认值做一次跨语言正则核对（2026-09-09 审计建议：靠
    人工记忆同步是"目前没出问题、迟早会有一次被漏掉"的模式，加这条低成本回归比指望记住注释更
    可靠）。只在这个仓库布局下才断言，找不到源文件就跳过而不是报错——避免打包/CI 环境路径不
    一样时误报。"""
    import re

    rs = Path(__file__).resolve().parents[2] / "services" / "book-serve" / "src" / "config.rs"
    if not rs.is_file():
        return
    m = re.search(r"native_upload_limit_mb:\s*(\d+)", rs.read_text(encoding="utf-8"))
    assert m, f"没在 {rs} 里找到 native_upload_limit_mb 默认值——是不是改了写法，这条检查也要跟着改"
    assert int(m.group(1)) == push.NATIVE_LIMIT_FALLBACK_MB, "book-serve 的默认体积上限跟 push.py 的静态兜底值不一致了，两边要手动同步"


def test_push_comic_native_pdf_added_when_small_enough(gateway, tmp_path, capsys, monkeypatch):
    """灰阶 CBZ 体积估算转 PDF 后仍在原生上限内：CBZ 和 PDF 都落母版库，两次独立入库。"""
    src = tmp_path / "manga.cbz"
    src.write_bytes(b"PK\x03\x04")
    gray = tmp_path / "manga.gray.cbz"
    gray.write_bytes(b"g" * 1000)  # 体积远小于任何合理上限
    pdf_calls = []

    def _mk_pdf(s, o):
        o.write_bytes(b"%PDF" + b"\0" * 100)
        pdf_calls.append((s.name, o.name))
        return o

    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    monkeypatch.setattr(cb, "comic_gray", lambda s, o, rtl=True: (gray, {"pages": 1, "split": 0, "gray": 1, "color": 0, "bytes_in": 10, "bytes_out": 1000, "flagged": []}))
    monkeypatch.setattr(cb, "cbz_to_pdf", _mk_pdf)
    FakeGateway.received.clear()
    rc, out = run(["push", str(src)], gateway, capsys)
    assert rc == 0 and pdf_calls == [("manga.gray.cbz", "manga.pdf")]
    assert "顺带出一份 PDF" in out
    names = [r[0] for r in FakeGateway.received]
    assert names.count("/api/books/staging") == 2, "CBZ 和 PDF 各自独立入库一次"
    bodies = b"".join(r[2] for r in FakeGateway.received)
    assert b"manga.gray.cbz" in bodies and b"manga.pdf" in bodies


def test_push_comic_native_pdf_skipped_when_too_big(gateway, tmp_path, capsys, monkeypatch):
    """灰阶 CBZ 体积估算超原生上限：只出 CBZ，cbz_to_pdf 压根不该被调用，也不该触发分卷。
    用一个极小的上限（1 字节）代替真造一个大文件——只测"超限就不生成 PDF"这条逻辑分支，
    不用真传几百 MB 数据折腾测试环境。"""
    src = tmp_path / "manga.cbz"
    src.write_bytes(b"PK\x03\x04")
    gray = tmp_path / "manga.gray.cbz"
    gray.write_bytes(b"g" * 10)
    called = {"n": 0}

    def _should_not_run(s, o):
        called["n"] += 1
        raise AssertionError("体积超限不该调用 cbz_to_pdf")

    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    monkeypatch.setattr(cb, "comic_gray", lambda s, o, rtl=True: (gray, {"pages": 1, "split": 0, "gray": 1, "color": 0, "bytes_in": 10, "bytes_out": 10, "flagged": []}))
    monkeypatch.setattr(cb, "cbz_to_pdf", _should_not_run)
    monkeypatch.setattr(push, "_native_limit_bytes", lambda ctx: 1)  # 逼近"任何真实文件都超限"
    FakeGateway.received.clear()
    rc, out = run(["push", str(src)], gateway, capsys)
    assert rc == 0 and called["n"] == 0 and "只出 CBZ" in out
    names = [r[0] for r in FakeGateway.received]
    assert names.count("/api/books/staging") == 1, "只有 CBZ 落库"


def test_push_no_comic_native_flag_skips_pdf_generation(gateway, tmp_path, capsys, monkeypatch):
    """--no-comic-native：不管体积多小，都不生成投原生用的 PDF。"""
    src = tmp_path / "manga.cbz"
    src.write_bytes(b"PK\x03\x04")
    gray = tmp_path / "manga.gray.cbz"
    gray.write_bytes(b"g" * 10)
    called = {"n": 0}
    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    monkeypatch.setattr(cb, "comic_gray", lambda s, o, rtl=True: (gray, {"pages": 1, "split": 0, "gray": 1, "color": 0, "bytes_in": 10, "bytes_out": 10, "flagged": []}))
    monkeypatch.setattr(cb, "cbz_to_pdf", lambda s, o: called.__setitem__("n", called["n"] + 1))
    FakeGateway.received.clear()
    rc, out = run(["push", "--no-comic-native", str(src)], gateway, capsys)
    assert rc == 0 and called["n"] == 0
    names = [r[0] for r in FakeGateway.received]
    assert names.count("/api/books/staging") == 1


def test_comic_native_pdf_is_never_split_even_if_over_split_threshold(gateway, tmp_path, capsys, monkeypatch):
    """漫画通道出的 PDF 绝不分卷——就算体积超过 split_pdf_mb（分卷是给普通 PDF 用的独立机制）。"""
    src = tmp_path / "manga.cbz"
    src.write_bytes(b"PK\x03\x04")
    gray = tmp_path / "manga.gray.cbz"
    gray.write_bytes(b"g" * 10)
    split_called = {"n": 0}

    def _mk_pdf(s, o):
        o.write_bytes(b"%PDF" + b"\0" * 100)
        return o

    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    monkeypatch.setattr(cb, "comic_gray", lambda s, o, rtl=True: (gray, {"pages": 1, "split": 0, "gray": 1, "color": 0, "bytes_in": 10, "bytes_out": 10, "flagged": []}))
    monkeypatch.setattr(cb, "cbz_to_pdf", _mk_pdf)
    monkeypatch.setattr(pdfsplit, "needs_split", lambda *a: split_called.__setitem__("n", split_called["n"] + 1) or True)
    FakeGateway.received.clear()
    rc, out = run(["push", str(src)], gateway, capsys)
    assert rc == 0 and split_called["n"] == 0, "route==comic 时压根不该问 pdfsplit.needs_split"
    assert "分卷" not in out


def test_split_fails_loudly_without_pymupdf(tmp_path, monkeypatch):
    """>上限的 PDF 切不了必须抛错（老实现静默返回原文件 → 297MB 整本推上去被 xochitl 拒）。"""
    import subprocess

    big = tmp_path / "big.pdf"
    big.write_bytes(b"%PDF" + b"\0" * (2 * 2**20))
    monkeypatch.setattr(cb, "_run", lambda cmd, **kw: subprocess.CompletedProcess(cmd, 1, "", "ModuleNotFoundError: pymupdf"))
    import pytest

    with pytest.raises(cb.CalibreError, match="分卷失败"):
        pdfsplit.split(big, 1, tmp_path)
    # 成功路径：按脚本 stdout 最后一行 JSON 取分卷
    p1, p2 = tmp_path / "big (1of2).pdf", tmp_path / "big (2of2).pdf"
    monkeypatch.setattr(cb, "_run", lambda cmd, **kw: subprocess.CompletedProcess(cmd, 0, f'log\n{{"parts": ["{p1}", "{p2}"]}}\n', ""))
    assert pdfsplit.split(big, 1, tmp_path) == [p1, p2]
    small = tmp_path / "s.pdf"
    small.write_bytes(b"%PDF")
    assert pdfsplit.split(small, 60, tmp_path) == [small], "不超上限原样返回、不调脚本"
