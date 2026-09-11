"""`shelf push --eink-gray`：CBZ 逐页 16 灰（灰页 4-bit PNG ≤16 色 / 彩页保色 JPEG / 超尺寸缩进屏盒 / 页序自然排序）；push 路由。"""
from __future__ import annotations

import io
import sys
import zipfile
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from shelf_cli import calibre_bridge as cb  # noqa: E402
from shelf_cli.commands import push  # noqa: E402
from test_cli import FakeGateway, gateway, run  # noqa: E402,F401

Image = pytest.importorskip("PIL.Image")
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "calibre"))
import comic_gray as cg  # noqa: E402


def _png(img) -> bytes:
    b = io.BytesIO()
    img.save(b, "PNG")
    return b.getvalue()


def _jpg(img) -> bytes:
    b = io.BytesIO()
    img.convert("RGB").save(b, "JPEG", quality=90)
    return b.getvalue()


def _cbz(path: Path):
    line = Image.new("L", (600, 900), 255)
    for y in range(0, 900, 7):
        for x in range(600):
            line.putpixel((x, y), 0)
    grad = Image.linear_gradient("L").resize((600, 900))  # 256 灰渐变 → 16 灰抖动
    color = Image.new("RGB", (600, 900), (200, 30, 30))
    big = Image.new("L", (3000, 4000), 128)
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("page_10.png", _png(line))
        z.writestr("page_2.jpg", _jpg(grad))
        z.writestr("page_1.png", _png(color))
        z.writestr("page_3.png", _png(big))
        z.writestr("readme.txt", "x")


def test_convert_gray_color_resize_and_order(tmp_path):
    src = tmp_path / "in.cbz"
    _cbz(src)
    out = tmp_path / "out.cbz"
    st = cg.convert(src, out)
    assert st["pages"] == 4 and st["gray"] == 3 and st["color"] == 1
    with zipfile.ZipFile(out) as z:
        names = z.namelist()
        assert names == ["page_1.jpg", "page_2.png", "page_3.png", "page_10.png"], "自然序 + 按类改扩展名"
        color = Image.open(io.BytesIO(z.read("page_1.jpg")))
        assert color.format == "JPEG" and color.mode == "RGB"
        g = Image.open(io.BytesIO(z.read("page_2.png")))
        assert g.format == "PNG" and g.mode == "P" and len(g.getcolors()) <= 16, "16 灰调色板"
        grays = {g.getpalette()[i * 3] for i in range(16)}
        assert {0, 255} <= grays and len(grays) == 16, "等距 16 级"
        big = Image.open(io.BytesIO(z.read("page_3.png")))
        assert max(big.size) <= 1696 and min(big.size) <= 954 and big.size == (954, 1272)
    assert st["bytes_in"] > 0 and st["bytes_out"] > 0


def test_normal_portrait_page_is_not_split_and_border_is_cropped():
    # 630×960 竖版页，50px 白边包一块灰色内容——不该被判成跨页；白边该被裁掉大半。
    img = Image.new("L", (630, 960), 255)
    for y in range(50, 910):
        for x in range(50, 580):
            img.putpixel((x, y), 100)
    assert not cg.looks_like_spread(img)
    x0, y0, x1, y1 = cg.content_bbox(img)
    assert (x0, y0, x1, y1) == (50, 50, 580, 910), "该精确裁到内容边界"


def test_large_uniform_content_is_never_treated_as_border():
    # 复用现有 fixture 的 3000×4000 纯 128 灰图：整页都是"内容"，不该裁任何一边。
    big = Image.new("L", (3000, 4000), 128)
    assert cg.content_bbox(big) == (0, 0, 3000, 4000)


def test_large_near_white_region_exceeding_cap_is_not_trimmed():
    # 上 70% 是内容、下 30% 纯白——超过 MAX_TRIM_FRAC(20%)，该放弃裁 Y 轴，保留整页。
    img = Image.new("L", (400, 1000))
    for y in range(1000):
        for x in range(400):
            img.putpixel((x, y), 100 if y < 700 else 255)
    x0, y0, x1, y1 = cg.content_bbox(img)
    assert (y0, y1) == (0, 1000), "裁掉比例超上限，该放弃裁这一轴"


def _spread(w=1200, h=800, gutter=(580, 620)):
    """合成一张跨页图：左半红（真彩）、装订缝纯白、右半灰（黑白）。"""
    img = Image.new("RGB", (w, h), (200, 30, 30))
    for x in range(*gutter):
        for y in range(h):
            img.putpixel((x, y), (255, 255, 255))
    for x in range(gutter[1], w):
        for y in range(h):
            img.putpixel((x, y), (100, 100, 100))
    return img


def test_find_gutter_lands_inside_the_known_white_band():
    img = _spread()
    x, density = cg.find_gutter_x(img)
    assert 580 <= x < 620, f"装订缝应落在已知白带内，got {x}"
    assert density <= cg.GUTTER_CLEAN_FRAC


def test_spread_splits_in_rtl_order_by_default():
    img = _spread()
    parts, flag = cg.split_spread(img)
    assert len(parts) == 2 and flag is None
    right, left = parts  # RTL：右半（灰）在前，左半（红）在后
    assert cg.mean_chroma(right) < cg.COLOR_KEEP_CHROMA, "右半该是黑白那半"
    assert cg.mean_chroma(left) >= cg.COLOR_KEEP_CHROMA, "左半该是真彩那半"


def test_spread_splits_ltr_when_requested():
    img = _spread()
    parts, _ = cg.split_spread(img, rtl=False)
    left, right = parts
    assert cg.mean_chroma(left) >= cg.COLOR_KEEP_CHROMA and cg.mean_chroma(right) < cg.COLOR_KEEP_CHROMA


def test_convert_spread_cbz_produces_two_ordered_pages_per_source(tmp_path):
    src, out = tmp_path / "in.cbz", tmp_path / "out.cbz"
    with zipfile.ZipFile(src, "w") as z:
        z.writestr("001.jpg", _jpg(_spread()))
    st = cg.convert(src, out)
    assert st["pages"] == 1 and st["split"] == 1 and st["gray"] == 1 and st["color"] == 1
    with zipfile.ZipFile(out) as z:
        names = z.namelist()
        assert names == ["001a.png", "001b.jpg"], "自然序里 a 排 b 前面；灰=PNG 彩=JPEG"


def test_ambiguous_wide_image_without_clean_gutter_is_left_alone():
    # 宽高比 1.08（超过 1.05 但没到 1.3 的"肯定"线），全图均匀内容、没有任何空白装订缝。
    img = Image.new("L", (650, 600), 100)
    parts, flag = cg.split_spread(img)
    assert len(parts) == 1 and flag is None


def test_extremely_wide_image_that_stays_wide_after_split_is_flagged_not_split():
    # 3000×800（比例 3.75），中间纵向留一条白缝——但对半切完两边仍各 1500×800（比例 1.875，仍偏宽），
    # 该放弃拆分、原图直出、给出人工核对提示，不硬切成两张还是难读的宽图。
    img = Image.new("L", (3000, 800), 100)
    for x in range(1490, 1510):
        for y in range(800):
            img.putpixel((x, y), 255)
    parts, flag = cg.split_spread(img)
    assert len(parts) == 1 and flag is not None and "人工核对" in flag


def test_mean_chroma_mirrors_device_rule():
    assert cg.mean_chroma(Image.new("L", (10, 10), 100)) == 0.0
    assert cg.mean_chroma(Image.new("RGB", (100, 100), (120, 120, 120))) == 0.0
    assert cg.mean_chroma(Image.new("RGB", (100, 100), (200, 30, 30))) > cg.COLOR_KEEP_CHROMA
    assert cg.mean_chroma(Image.new("RGB", (100, 100), (128, 130, 126))) < cg.COLOR_KEEP_CHROMA, "偏色扫描当灰"
    assert cg.COLOR_KEEP_CHROMA == 0.06, "与 bookconv imgopt.rs 同值"


def test_push_cbz_with_eink_gray_routes_comic_and_calls_gray(gateway, tmp_path, capsys, monkeypatch):
    src = tmp_path / "manga.cbz"
    src.write_bytes(b"PK\x03\x04")
    gray = tmp_path / "manga.gray.cbz"
    gray.write_bytes(b"PK\x03\x04g")
    calls = []
    monkeypatch.setattr(cb, "has_calibre", lambda: False)
    monkeypatch.setattr(cb, "comic_gray", lambda s, o, rtl=True: (calls.append((s.name, o.name)) or (gray, {"pages": 3, "split": 0, "gray": 2, "color": 1, "bytes_in": 300, "bytes_out": 450, "flagged": []})))

    def _no_pdf(s, o):  # 这个测试不关心投原生 PDF 这条支线，交给 test_push.py 专门测
        raise cb.CalibreError("mock: skip native-pdf in this test")

    monkeypatch.setattr(cb, "cbz_to_pdf", _no_pdf)
    FakeGateway.received.clear()
    rc, out = run(["push", str(src)], gateway, capsys)  # 缺省开
    assert rc == 0 and calls == [("manga.cbz", "manga.gray.cbz")] and "16 灰 2 页" in out and "保色 1 页" in out
    assert FakeGateway.received[-1][0].split("?")[0] == "/api/books/staging"
    calls.clear()
    rc, out = run(["push", "--no-eink-gray", str(src)], gateway, capsys)
    assert rc == 0 and calls == [] and "原样→" in out, "--no-eink-gray CBZ 原样"
