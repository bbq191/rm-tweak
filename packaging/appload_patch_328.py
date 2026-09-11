#!/usr/bin/env python3
"""appload 3.28 兼容补丁：给官方发行版 appload.so（v0.5.3）等长回填一段内嵌的 qmd 字符串。

背景/为什么这么做/许可留痕见同目录 appload-qmd-PROVENANCE.md，别在这里重复。一句话：
appload.so 内嵌了一份 xovi qmd 补丁（NUL 结尾 C 字符串常量），v0.5.3 发行版里那份补丁认的
Sidebar/MainView 锚点是 3.27 的旧标识符，在 3.28 上会导致 qmldiff 解析不了、appload 自己的
注入失败（症状：Sidebar 里挂的入口点了没反应）。上游已经在 PR #59 修好、合并进 master，但
还没有发布新 tag——`vellum add appload` 装的仍是旧的 v0.5.3。这个工具把新内容等长塞回编译好
的 .so 里，不需要重新编译整个 appload.so。

用法：
    python3 appload_patch_328.py <输入 appload.so> <输出路径>
    python3 appload_patch_328.py --check <appload.so>     只检测版本状态，不写文件

退出码：0=成功（或 --check 判定清楚），1=定位失败（唯一性校验没过，不碰文件），
        2=用法错误。
"""
from __future__ import annotations

import hashlib
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
ORIG_QMD = HERE / "appload-qmd-v0.5.3.orig.qmd"
NEW_QMD = HERE / "appload-qmd-3.28.qmd"


def _read_bytes(p: Path) -> bytes:
    return p.read_bytes()


def find_unique(haystack: bytes, needle: bytes) -> int | None:
    """在 haystack 里找 needle，要求命中且只命中一次——0 次或 >1 次都返回 None。

    跟 enhance/shared/pattern.c::cj_find_unique_pattern 同一条纪律：宁可放弃也不要在
    不确定的位置上动手。
    """
    first = haystack.find(needle)
    if first < 0:
        return None
    second = haystack.find(needle, first + 1)
    if second >= 0:
        return None
    return first


def patch(so_bytes: bytes, orig_qmd: bytes, new_qmd: bytes) -> bytes:
    """核心字节替换逻辑，纯函数、不碰文件系统——host 单测直接调这个。

    返回补丁后的完整字节串；找不到/命中不唯一/新内容装不下都抛 ValueError（调用方决定
    怎么报错退出，这里不做 I/O 也不 print，保持可测试）。
    """
    if len(new_qmd) > len(orig_qmd):
        raise ValueError(
            f"新 qmd（{len(new_qmd)} 字节）比原 qmd（{len(orig_qmd)} 字节）长，装不进等长回填的"
            f"空间——这种情况下必须重新编译 appload.so，本工具不处理"
        )

    offset = find_unique(so_bytes, orig_qmd)
    if offset is None:
        count = so_bytes.count(orig_qmd)
        raise ValueError(
            f"在目标文件里搜索已知 v0.5.3 原始 qmd 字节序列，命中 {count} 次（要求恰好 1 次）——"
            f"不是预期的 v0.5.3 appload.so，或者已经打过这个补丁（打过之后原始 v0.5.3 的字节序列"
            f"就不在文件里了，count=0 也会落到这个分支），不碰文件"
        )

    padded_new = new_qmd + b"\x00" * (len(orig_qmd) - len(new_qmd))
    assert len(padded_new) == len(orig_qmd)

    patched = bytearray(so_bytes)
    patched[offset : offset + len(orig_qmd)] = padded_new
    return bytes(patched)


def describe_status(so_bytes: bytes, orig_qmd: bytes, new_qmd: bytes) -> str:
    """--check 模式用：判断目标文件里 qmd 处于哪种已知状态，供部署脚本探测用。"""
    if find_unique(so_bytes, orig_qmd) is not None:
        return "unpatched"  # v0.5.3 原始未打补丁状态，可以打
    padded_new = new_qmd + b"\x00" * (len(orig_qmd) - len(new_qmd))
    if find_unique(so_bytes, padded_new) is not None:
        return "patched"  # 已经是打过补丁之后的样子（幂等识别，重复跑不用报错）
    return "unknown"  # 既不是已知的原始状态也不是已知的打过状态——可能是别的版本，别动


def main(argv: list[str]) -> int:
    if len(argv) == 3 and argv[1] == "--check":
        target = Path(argv[2])
        if not target.is_file():
            print(f"!! 找不到文件：{target}", file=sys.stderr)
            return 2
        status = describe_status(_read_bytes(target), _read_bytes(ORIG_QMD), _read_bytes(NEW_QMD))
        print(status)
        return 0 if status in ("unpatched", "patched") else 1

    if len(argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2

    src, dst = Path(argv[1]), Path(argv[2])
    if not src.is_file():
        print(f"!! 找不到输入文件：{src}", file=sys.stderr)
        return 2

    so_bytes = _read_bytes(src)
    orig_qmd = _read_bytes(ORIG_QMD)
    new_qmd = _read_bytes(NEW_QMD)

    try:
        patched = patch(so_bytes, orig_qmd, new_qmd)
    except ValueError as e:
        print(f"!! {e}", file=sys.stderr)
        return 1

    dst.write_bytes(patched)
    assert len(patched) == len(so_bytes), "补丁后文件总长度必须跟原文件完全一致（等长回填的核心不变式）"
    print(f"-- 已写入 {dst}（{len(patched)} 字节，跟输入文件长度一致）")
    print(f"   输入 md5={hashlib.md5(so_bytes).hexdigest()}")
    print(f"   输出 md5={hashlib.md5(patched).hexdigest()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
