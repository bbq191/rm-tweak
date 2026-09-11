"""安装/卸载/部署脚本的本机模拟测试入口（真正的断言都在 run_sim_tests.sh 里）。

用假 ssh/scp/systemctl/mount 等桩命令 + 临时目录当"设备"，跑 shelf/install.sh、uninstall.sh、各 deploy-*.sh、
install-all/uninstall-all 的真代码，断言幂等、失败时恢复 ro、缺载荷不留半成品、xovi 已生效时绝不 xovi/start、
安装与卸载清单对称等。不碰真机。详见 run_sim_tests.sh 头注。
"""
from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

import pytest

SCRIPT = Path(__file__).resolve().parent / "run_sim_tests.sh"


@pytest.mark.skipif(shutil.which("bash") is None, reason="需要 bash")
@pytest.mark.skipif(os.geteuid() == 0, reason="模拟测试拒绝以 root 运行")
def test_install_scripts_simulation() -> None:
    proc = subprocess.run(["bash", str(SCRIPT)], capture_output=True, text=True, timeout=300)
    out = proc.stdout + proc.stderr
    assert proc.returncode == 0, "模拟测试有失败项：\n" + "\n".join(l for l in out.splitlines() if "FAIL" in l or "结果" in l)
