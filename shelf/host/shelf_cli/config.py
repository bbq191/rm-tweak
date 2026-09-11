"""`$XDG_CONFIG_HOME/shelf/config.toml`（tomllib，stdlib ≥3.11）。缺省值集中一处。"""
from __future__ import annotations

import tomllib
from dataclasses import dataclass

from .paths import Paths

DEFAULTS = {
    "host": "10.11.99.1",
    "port": 443,
    "scheme": "https",
    "password": "",
    "verify_tls": False,
    "split_pdf_mb": 60,
}


@dataclass
class Config:
    host: str
    port: int
    scheme: str
    password: str
    verify_tls: bool
    split_pdf_mb: int

    @property
    def base_url(self) -> str:
        return f"{self.scheme}://{self.host}:{self.port}"


def load(paths: Paths, overrides: dict | None = None) -> Config:
    d = dict(DEFAULTS)
    f = paths.config_file
    if f.is_file():
        with f.open("rb") as fh:
            d.update({k: v for k, v in tomllib.load(fh).items() if k in DEFAULTS})
    d.update({k: v for k, v in (overrides or {}).items() if v is not None})
    return Config(**d)
