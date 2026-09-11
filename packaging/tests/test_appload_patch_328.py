"""appload_patch_328.py 的 host 单测——纯字节操作，不需要真实 appload.so、不需要真机。

用构造出的假"二进制片段"（前后各垫一段无关字节，模拟 qmd 字符串常量前后的其它数据）验证：
唯一命中才动手、总长度不变（等长回填的核心不变式）、0 次/多次命中都安全拒绝、--check 状态
判定幂等。真实 appload.so 里能不能找到且只找到一次这段 qmd 字节，这里没法验证——见同目录
appload-qmd-PROVENANCE.md「已知局限」。
"""
from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import appload_patch_328 as ap  # noqa: E402

ORIG_QMD = (Path(__file__).resolve().parents[1] / "appload-qmd-v0.5.3.orig.qmd").read_bytes()
NEW_QMD = (Path(__file__).resolve().parents[1] / "appload-qmd-3.28.qmd").read_bytes()

PREFIX = b"\x7fELF" + b"\x01" * 40  # 假装是 ELF 头之类的无关前缀
SUFFIX = b"\x02" * 60  # 假装 qmd 字符串之后还有其它数据段


def make_fake_so(qmd: bytes) -> bytes:
    return PREFIX + qmd + SUFFIX


def test_reference_qmd_files_shape():
    """先确认两份 bundled 参照文件本身的形状符合 PROVENANCE.md 记录的事实——如果这个测试
    先挂了，说明文件被意外改动/替换过，后面的测试全都不可信。"""
    assert len(ORIG_QMD) == 7895
    assert len(NEW_QMD) == 7427
    assert len(NEW_QMD) < len(ORIG_QMD), "等长回填要求新内容不长于原内容"
    assert ORIG_QMD.startswith(b"AFFECT [[17477757197668945522]]")
    assert NEW_QMD.startswith(b"AFFECT [[17477757197668945522]]")


def test_find_unique_single_hit():
    hay = PREFIX + b"needle" + SUFFIX
    assert ap.find_unique(hay, b"needle") == len(PREFIX)


def test_find_unique_zero_hits():
    hay = PREFIX + SUFFIX
    assert ap.find_unique(hay, b"needle") is None


def test_find_unique_multi_hits():
    hay = b"needle" + b"." * 5 + b"needle"
    assert ap.find_unique(hay, b"needle") is None


def test_patch_rewrites_only_the_qmd_region_and_preserves_length():
    so = make_fake_so(ORIG_QMD)
    patched = ap.patch(so, ORIG_QMD, NEW_QMD)

    assert len(patched) == len(so), "等长回填：补丁后文件总长度必须跟原文件完全一致"
    assert patched[: len(PREFIX)] == PREFIX, "前缀不该被动"
    assert patched[len(PREFIX) + len(ORIG_QMD) :] == SUFFIX, "后缀不该被动"

    region = patched[len(PREFIX) : len(PREFIX) + len(ORIG_QMD)]
    assert region[: len(NEW_QMD)] == NEW_QMD
    assert region[len(NEW_QMD) :] == b"\x00" * (len(ORIG_QMD) - len(NEW_QMD)), "剩余部分必须是 NUL 填充"


def test_patch_rejects_zero_hits():
    so = PREFIX + SUFFIX  # 根本没有 orig_qmd
    try:
        ap.patch(so, ORIG_QMD, NEW_QMD)
        assert False, "0 次命中应该抛 ValueError"
    except ValueError as e:
        assert "0" in str(e) or "命中" in str(e)


def test_patch_rejects_multi_hits():
    so = ORIG_QMD + b"." * 8 + ORIG_QMD  # 两处命中，唯一性校验必须拒绝
    try:
        ap.patch(so, ORIG_QMD, NEW_QMD)
        assert False, "多次命中应该抛 ValueError，不能挑一个位置蒙混过关"
    except ValueError:
        pass


def test_patch_rejects_new_content_longer_than_original():
    so = make_fake_so(ORIG_QMD)
    try:
        ap.patch(so, ORIG_QMD, ORIG_QMD + b"x")  # 故意构造一个比原内容还长的"新内容"
        assert False, "新内容比原内容长应该拒绝（装不进等长回填空间）"
    except ValueError as e:
        assert "长" in str(e)


def test_describe_status_unpatched_then_patched_then_unknown():
    unpatched_so = make_fake_so(ORIG_QMD)
    assert ap.describe_status(unpatched_so, ORIG_QMD, NEW_QMD) == "unpatched"

    patched_so = ap.patch(unpatched_so, ORIG_QMD, NEW_QMD)
    assert ap.describe_status(patched_so, ORIG_QMD, NEW_QMD) == "patched", "打过补丁后要能幂等识别，重复跑不该再报错"

    unrelated_so = PREFIX + b"totally unrelated content" + SUFFIX
    assert ap.describe_status(unrelated_so, ORIG_QMD, NEW_QMD) == "unknown"


def test_cli_check_mode_exit_codes(tmp_path):
    unpatched = tmp_path / "unpatched.so"
    unpatched.write_bytes(make_fake_so(ORIG_QMD))
    assert ap.main(["appload_patch_328.py", "--check", str(unpatched)]) == 0

    patched = tmp_path / "patched.so"
    patched.write_bytes(ap.patch(make_fake_so(ORIG_QMD), ORIG_QMD, NEW_QMD))
    assert ap.main(["appload_patch_328.py", "--check", str(patched)]) == 0

    unknown = tmp_path / "unknown.so"
    unknown.write_bytes(b"nothing recognizable here")
    assert ap.main(["appload_patch_328.py", "--check", str(unknown)]) == 1


def test_cli_patch_mode_writes_file(tmp_path):
    src = tmp_path / "in.so"
    src.write_bytes(make_fake_so(ORIG_QMD))
    dst = tmp_path / "out.so"
    rc = ap.main(["appload_patch_328.py", str(src), str(dst)])
    assert rc == 0
    assert dst.is_file()
    assert len(dst.read_bytes()) == len(src.read_bytes())
