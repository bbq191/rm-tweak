"""comic2cbz.py::spine_ordered_images：漫画管线里唯一没有单测兜底的一环（2026-09-09 审计发现，
比三轮代码体检本身还老），补合成 EPUB 用例覆盖正常/异常两类输入，不必真的跑 Calibre/ebook-convert。"""
from __future__ import annotations

import sys
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "calibre"))
import comic2cbz as c2c  # noqa: E402


def _epub(entries: dict[str, str | bytes]) -> zipfile.ZipFile:
    """entries: 路径 -> 内容（str 当 UTF-8 文本写，bytes 原样写）。"""
    import io

    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        for name, content in entries.items():
            z.writestr(name, content.encode("utf-8") if isinstance(content, str) else content)
    buf.seek(0)
    return zipfile.ZipFile(buf)


CONTAINER = '<?xml version="1.0"?><container><rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles></container>'


def _opf(items: str, spine: str) -> str:
    return f'<?xml version="1.0"?><package><manifest>{items}</manifest><spine>{spine}</spine></package>'


def test_spine_with_direct_image_items_in_order():
    """最常见形态：spine 直接引用图片文件（Calibre 洗过的漫画 EPUB 常这样排）。"""
    opf = _opf(
        items='<item id="p1" href="images/001.jpg" media-type="image/jpeg"/>'
        '<item id="p2" href="images/002.jpg" media-type="image/jpeg"/>',
        spine='<itemref idref="p1"/><itemref idref="p2"/>',
    )
    z = _epub({
        "META-INF/container.xml": CONTAINER,
        "OEBPS/content.opf": opf,
        "OEBPS/images/001.jpg": b"\xff\xd8\xff\x00",
        "OEBPS/images/002.jpg": b"\xff\xd8\xff\x01",
    })
    assert c2c.spine_ordered_images(z) == ["OEBPS/images/001.jpg", "OEBPS/images/002.jpg"]


def test_spine_with_xhtml_pages_wrapping_img_tags():
    """另一种形态：spine 引用 xhtml 页，页内嵌 <img src=...>（图片路径相对页面所在目录解析）。"""
    opf = _opf(
        items='<item id="pg1" href="text/p1.xhtml" media-type="application/xhtml+xml"/>'
        '<item id="im1" href="images/a.png" media-type="image/png"/>',
        spine='<itemref idref="pg1"/>',
    )
    page = '<html><body><img src="../images/a.png"/></body></html>'
    z = _epub({
        "META-INF/container.xml": CONTAINER,
        "OEBPS/content.opf": opf,
        "OEBPS/text/p1.xhtml": page,
        "OEBPS/images/a.png": b"\x89PNG\x00",
    })
    assert c2c.spine_ordered_images(z) == ["OEBPS/images/a.png"]


def test_manifest_missing_entry_for_spine_idref_is_skipped_not_fatal():
    """spine 引用了一个 manifest 里没有的 idref（畸形 EPUB）——跳过这一条，不抛异常，其余照常抽。"""
    opf = _opf(
        items='<item id="p1" href="images/001.jpg" media-type="image/jpeg"/>',
        spine='<itemref idref="missing"/><itemref idref="p1"/>',
    )
    z = _epub({
        "META-INF/container.xml": CONTAINER,
        "OEBPS/content.opf": opf,
        "OEBPS/images/001.jpg": b"\xff\xd8\xff\x00",
    })
    assert c2c.spine_ordered_images(z) == ["OEBPS/images/001.jpg"]


def test_duplicate_image_refs_are_deduped_keeping_first_occurrence():
    opf = _opf(
        items='<item id="p1" href="images/001.jpg" media-type="image/jpeg"/>'
        '<item id="p2" href="text/p2.xhtml" media-type="application/xhtml+xml"/>',
        spine='<itemref idref="p1"/><itemref idref="p2"/>',
    )
    page = '<html><body><img src="../images/001.jpg"/></body></html>'
    z = _epub({
        "META-INF/container.xml": CONTAINER,
        "OEBPS/content.opf": opf,
        "OEBPS/images/001.jpg": b"\xff\xd8\xff\x00",
        "OEBPS/text/p2.xhtml": page,
    })
    assert c2c.spine_ordered_images(z) == ["OEBPS/images/001.jpg"], "同一张图被两处引用只算一次，且在首次出现的位置"


def test_missing_container_xml_returns_empty_not_raises():
    """中转 EPUB 缺 META-INF/container.xml（畸形输入）——原来这里会抛 KeyError 冒到 main() 变裸
    traceback，现在应该优雅退化成空列表，交给 main() 的"抽不到图，不像漫画"分支处理。"""
    z = _epub({"OEBPS/content.opf": _opf("", "")})
    assert c2c.spine_ordered_images(z) == []


def test_container_xml_without_full_path_attribute_returns_empty_not_raises():
    z = _epub({
        "META-INF/container.xml": '<?xml version="1.0"?><container><rootfiles><rootfile/></rootfiles></container>',
    })
    assert c2c.spine_ordered_images(z) == []


def test_opf_referencing_missing_opf_path_returns_empty_not_raises():
    """container.xml 指向的 content.opf 本身不存在于压缩包里。"""
    z = _epub({"META-INF/container.xml": CONTAINER})
    assert c2c.spine_ordered_images(z) == []
