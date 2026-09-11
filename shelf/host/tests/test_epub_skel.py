"""共享 EPUB 骨架：布局、两级目录、css/图片/作者可选、mimetype 首个且 STORED。"""
from __future__ import annotations

import sys
import zipfile
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "calibre"))
import epub_skel as sk  # noqa: E402


def test_nav_two_levels_without_empty_ol():
    n = sk.nav_items([("a", "开头", 2), ("b", "卷一", 1), ("c", "第一章", 2), ("d", "第二章", 2), ("e", "卷二", 1), ("f", "尾声", 1)])
    assert n.count("<ol>") == 1 and "<ol></ol>" not in n
    assert n.index("卷一") < n.index("第一章") < n.index("卷二") < n.index("尾声")
    assert n.startswith('<li><a href="a">开头</a></li>'), "没有父级的 2 级按 1 级"
    assert n.endswith('<li><a href="f">尾声</a></li>')


def test_write_epub_layout_css_images_author(tmp_path):
    img = tmp_path / "p1.jpg"
    img.write_bytes(b"\xff\xd8\xff")
    out = sk.write_epub(tmp_path / "b.epub", "书 & 名", [sk.Chapter("一", "<p>x</p>"), sk.Chapter("二", '<p><img src="../images/p1.jpg"/></p>', 2)], css="p{}", css_name="cangjie-wash.css", author="作者", lang="en", images=[img])
    with zipfile.ZipFile(out) as z:
        names = z.namelist()
        assert names[0] == "mimetype" and z.getinfo("mimetype").compress_type == zipfile.ZIP_STORED
        assert {"META-INF/container.xml", "OEBPS/content.opf", "OEBPS/nav.xhtml", "OEBPS/cangjie-wash.css", "OEBPS/text/c1.xhtml", "OEBPS/text/c2.xhtml", "OEBPS/images/p1.jpg"} <= set(names)
        opf = z.read("OEBPS/content.opf").decode()
        assert "<dc:title>书 &amp; 名</dc:title>" in opf and "<dc:creator>作者</dc:creator>" in opf and "<dc:language>en</dc:language>" in opf
        assert 'href="cangjie-wash.css" media-type="text/css"' in opf and 'href="images/p1.jpg" media-type="image/jpeg"' in opf
        assert '<link rel="stylesheet" type="text/css" href="../cangjie-wash.css"/>' in z.read("OEBPS/text/c1.xhtml").decode()
        assert z.read("OEBPS/nav.xhtml").decode().count("<ol>") == 2
    plain = sk.write_epub(tmp_path / "c.epub", "t", [sk.Chapter("一", "<p>x</p>")])
    with zipfile.ZipFile(plain) as z:
        assert "OEBPS/style.css" not in z.namelist() and "<link" not in z.read("OEBPS/text/c1.xhtml").decode()
        assert "<dc:creator>" not in z.read("OEBPS/content.opf").decode()
    with pytest.raises(ValueError):
        sk.write_epub(tmp_path / "d.epub", "t", [])
