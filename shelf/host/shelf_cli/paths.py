"""XDG 基目录规范（host 侧），与 Rust `shelf_core::paths`、shell `${XDG_*:-…}` 共享同一套**规则**
（缺省展开、相对路径视为无效）。host 只镜像它实际用到的 config / data / state / cache 四个根；
runtime / koreader_root / bin 等设备专属项不在 host（host 是 Python，不链接 Rust crate）。
⚠ 命名差异：本类字段（`self.data` 等）已含 `/shelf` 应用段，等价于 Rust 的 `data_dir()`（**非** Rust 的根字段 `data`）。"""
from __future__ import annotations

import os
from pathlib import Path

APP = "shelf"


def _pick(env: dict, var: str, default: Path) -> Path:
    v = env.get(var, "")
    return Path(v) if v.startswith("/") else default  # 规范：相对路径视为无效（同 Rust paths.rs 的 pick）


class Paths:
    def __init__(self, env: dict | None = None):
        env = dict(os.environ) if env is None else env
        home = Path(env.get("HOME") or "~").expanduser()
        self.home = home
        self.config = _pick(env, "XDG_CONFIG_HOME", home / ".config") / APP
        self.data = _pick(env, "XDG_DATA_HOME", home / ".local/share") / APP
        self.state = _pick(env, "XDG_STATE_HOME", home / ".local/state") / APP
        self.cache = _pick(env, "XDG_CACHE_HOME", home / ".cache") / APP

    @property
    def config_file(self) -> Path:
        return self.config / "config.toml"

    @property
    def snapshots_dir(self) -> Path:
        return self.cache / "snapshots"
