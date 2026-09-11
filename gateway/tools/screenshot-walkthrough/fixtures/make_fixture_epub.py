#!/usr/bin/env python3
"""生成一份最小合法 EPUB（供截图走查用的书架 fixture，不是真实书籍内容）。"""
import sys
import zipfile

CONTAINER_XML = """<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/></rootfiles>
</container>"""

def make(path, title, author, n_chapters=3):
    manifest_items = "\n".join(f'<item id="ch{i}" href="ch{i}.xhtml" media-type="application/xhtml+xml"/>' for i in range(n_chapters))
    spine_items = "\n".join(f'<itemref idref="ch{i}"/>' for i in range(n_chapters))
    opf = f"""<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="bookid">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:identifier id="bookid">urn:uuid:{title.replace(' ', '-')}</dc:identifier>
    <dc:title>{title}</dc:title>
    <dc:creator>{author}</dc:creator>
    <dc:language>zh</dc:language>
  </metadata>
  <manifest>
    {manifest_items}
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
  </manifest>
  <spine toc="ncx">
    {spine_items}
  </spine>
</package>"""
    navpoints = "\n".join(
        f'<navPoint id="np{i}" playOrder="{i+1}"><navLabel><text>第{i+1}章</text></navLabel><content src="ch{i}.xhtml"/></navPoint>'
        for i in range(n_chapters)
    )
    ncx = f"""<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <head/>
  <docTitle><text>{title}</text></docTitle>
  <navMap>{navpoints}</navMap>
</ncx>"""
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("mimetype", "application/epub+zip", zipfile.ZIP_STORED)
        z.writestr("META-INF/container.xml", CONTAINER_XML)
        z.writestr("OEBPS/content.opf", opf)
        z.writestr("OEBPS/toc.ncx", ncx)
        for i in range(n_chapters):
            z.writestr(
                f"OEBPS/ch{i}.xhtml",
                f'<?xml version="1.0" encoding="UTF-8"?><html xmlns="http://www.w3.org/1999/xhtml">'
                f'<body><h1>第{i+1}章</h1><p>截图走查用的占位正文，不是真实内容。</p></body></html>',
            )

if __name__ == "__main__":
    make(sys.argv[1], sys.argv[2], sys.argv[3], int(sys.argv[4]) if len(sys.argv) > 4 else 3)
    print(f"-- 写好 {sys.argv[1]}")
