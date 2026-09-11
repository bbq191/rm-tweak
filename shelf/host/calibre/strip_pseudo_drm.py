#!/usr/bin/env python3
"""剥「伪 DRM」：多看等书源的 encryption.xml 只加密样式/字体（如 dkagent.css），正文 xhtml 全明文，
但 calibre EPUB Input 见到非字体混淆的加密项一律 DRMError 拒转。救法（白皮书 §11.2《飘》）=丢弃
加密项文件 + encryption.xml（零解密）+ 清掉 OPF manifest 里对应 item。

**红线**：加密清单里只要有正文/导航/图片（.xhtml/.html/.htm/.opf/.ncx/图片）就是真 DRM，不碰、退出码 3。
纯 stdlib、用系统 python3 跑（calibre CLI 环境别进 uv venv，见白皮书 §11.2）。

用法: python3 strip_pseudo_drm.py in.epub out.epub
退出码: 0=已剥（或本无 encryption.xml，原样复制） 3=真 DRM 未产出 1=错误
"""

from __future__ import annotations

import posixpath
import re
import sys
import zipfile

SAFE_EXTS = (".css", ".ttf", ".otf", ".woff", ".woff2", ".js")


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__)
        return 1
    src, dst = sys.argv[1], sys.argv[2]
    zin = zipfile.ZipFile(src)
    names = zin.namelist()
    if "META-INF/encryption.xml" not in names:
        with open(dst, "wb") as f:
            f.write(open(src, "rb").read())
        print("无 encryption.xml，原样复制")
        return 0
    enc = zin.read("META-INF/encryption.xml").decode("utf-8", "ignore")
    uris = re.findall(r'CipherReference\s+URI="([^"]+)"', enc)
    targets = [posixpath.normpath(u) for u in uris if not u.startswith("#")]
    unsafe = [t for t in targets if not t.lower().endswith(SAFE_EXTS)]
    if unsafe:
        print(f"真 DRM：加密了正文/资源 {unsafe[:3]} 等 {len(unsafe)} 项，不剥")
        return 3
    drop = set(targets) | {"META-INF/encryption.xml"}
    zout = zipfile.ZipFile(dst, "w")
    for it in zin.infolist():
        if it.filename in drop:
            continue
        data = zin.read(it.filename)
        if it.filename.lower().endswith(".opf"):
            base = posixpath.dirname(it.filename)
            text = data.decode("utf-8", "ignore")
            for t in targets:
                rel = posixpath.relpath(t, base) if base else t
                text = re.sub(r'<item\b[^>]*\bhref="' + re.escape(rel) + r'"[^>]*/>\s*', "", text)
            data = text.encode("utf-8")
        comp = zipfile.ZIP_STORED if it.filename == "mimetype" else zipfile.ZIP_DEFLATED
        zout.writestr(it, data, compress_type=comp)
    zout.close()
    print(f"已剥伪 DRM：丢弃 {sorted(targets)} + encryption.xml")
    return 0


if __name__ == "__main__":
    sys.exit(main())
