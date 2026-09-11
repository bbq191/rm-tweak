"""设备访问抽象（Strategy）：HTTP 走网关；测试注入 FakeTransport。纯 urllib，自写 multipart。"""
from __future__ import annotations

import base64
import json
import mimetypes
import ssl
import urllib.error
import urllib.parse
import urllib.request
import uuid
from pathlib import Path


class TransportError(RuntimeError):
    pass


_DEFAULT = object()  # `_open(timeout=…)` 的哨兵：区分"用缺省超时"与"None=不超时"


class HttpTransport:
    """HTTPS（私有 CA 自签，缺省不校验证书——局域网 + 密码保护）+ HTTP Basic（网关只看密码，用户名任意）。"""

    def __init__(self, base_url: str, timeout: float = 900.0, user: str = "shelf", password: str = "", verify_tls: bool = False):
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self.auth = base64.b64encode(f"{user}:{password}".encode()).decode() if password else ""
        if verify_tls:
            self.ctx = ssl.create_default_context()
        else:
            self.ctx = ssl.create_default_context()
            self.ctx.check_hostname = False
            self.ctx.verify_mode = ssl.CERT_NONE

    def reachable(self, timeout: float = 3.0) -> bool:
        """设备网关是否在线：探 `GET /health`（网关公开路由，不用密码、不触发交互输入）。任何 HTTP 应答都算在线，
        连不上 / 超时算离线（设备离 USB 后几秒就自动休眠关 WiFi，这是 push 最常见的失败）。"""
        req = urllib.request.Request(self.base_url + "/health", method="GET")
        try:
            with urllib.request.urlopen(req, timeout=timeout, context=self.ctx):
                return True
        except urllib.error.HTTPError:
            return True
        except (urllib.error.URLError, OSError):
            return False

    def _open(self, method: str, path: str, query: dict | None = None, data: bytes | None = None, content_type: str | None = None, accept: str | None = None, timeout: float | None | object = _DEFAULT):
        """所有请求的唯一出口：拼 URL/鉴权头/超时，把 401/403/其它 HTTP 错、连不上统一翻成 TransportError。返回可读的响应对象。"""
        url = self.base_url + path
        if query:
            url += "?" + urllib.parse.urlencode({k: v for k, v in query.items() if v is not None})
        req = urllib.request.Request(url, data=data, method=method)
        if content_type:
            req.add_header("Content-Type", content_type)
        if accept:
            req.add_header("Accept", accept)
        if self.auth:
            req.add_header("Authorization", f"Basic {self.auth}")
        try:
            return urllib.request.urlopen(req, timeout=self.timeout if timeout is _DEFAULT else timeout, context=self.ctx)
        except urllib.error.HTTPError as e:
            if e.code == 401:
                raise TransportError("密码错误或未设置（config.toml 的 password / 环境变量 SHELF_PASSWORD / 交互输入；首次默认 shelf）") from None
            body = e.read()
            try:
                j = json.loads(body)
            except ValueError:
                j = {"ok": False, "message": body.decode("utf-8", "replace")[:200]}
            if e.code == 403 and "改密码" in str(j.get("message", "")):
                raise TransportError("首次登录必须先改密码：`shelf passwd`（或网页 /password）") from None
            raise TransportError(f"HTTP {e.code}: {j.get('message', j)}") from None
        except urllib.error.URLError as e:
            raise TransportError(f"连不上 {self.base_url}: {e.reason}") from None

    def _do(self, method: str, path: str, query: dict | None = None, data: bytes | None = None, content_type: str | None = None) -> dict:
        with self._open(method, path, query, data, content_type) as r:
            body = r.read()
        try:
            return json.loads(body)
        except ValueError:
            return {"raw": body.decode("utf-8", "replace")}

    def get(self, path: str, query: dict | None = None) -> dict:
        return self._do("GET", path, query)

    def get_bytes(self, path: str) -> bytes:
        """取二进制体（如渲染缓存 PDF）。"""
        with self._open("GET", path) as r:
            return r.read()

    def stream_lines(self, path: str):
        """长连接逐行读（SSE）：无读超时，靠服务端 20s 心跳保活；连接断开时生成器结束，调用方决定是否重连。"""
        with self._open("GET", path, accept="text/event-stream", timeout=None) as r:
            for raw in r:
                yield raw.decode("utf-8", "replace").rstrip("\r\n")

    def get_text(self, path: str) -> str:
        r = self._do("GET", path)
        return r["raw"] if "raw" in r else json.dumps(r)

    def post_text(self, path: str, data: bytes, query: dict | None = None) -> dict:
        return self._do("POST", path, query, data, "text/plain; charset=utf-8")

    def delete(self, path: str) -> dict:
        return self._do("DELETE", path)

    def delete_named(self, base_path: str, name: str) -> dict:
        """DELETE `<base_path>/<url 编码的 name>`——URL 编码规则单点收在 transport 层（各命令不再各自 import quote）。"""
        return self._do("DELETE", f"{base_path.rstrip('/')}/{urllib.parse.quote(name)}")

    def post_json(self, path: str, obj: dict) -> dict:
        return self._do("POST", path, data=json.dumps(obj).encode(), content_type="application/json")

    def put_json(self, path: str, obj: dict) -> dict:
        return self._do("PUT", path, data=json.dumps(obj).encode(), content_type="application/json")

    def post_files(self, path: str, files: list[Path], query: dict | None = None, field: str = "file") -> dict:
        boundary = "----shelfcli" + uuid.uuid4().hex
        body = bytearray()
        for f in files:
            ctype = mimetypes.guess_type(f.name)[0] or "application/octet-stream"
            fname = f.name.replace('"', "%22")
            body += (f"--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"; filename=\"{fname}\"; "
                     f"filename*=UTF-8''{urllib.parse.quote(f.name)}\r\nContent-Type: {ctype}\r\n\r\n").encode()
            body += f.read_bytes()
            body += b"\r\n"
        body += f"--{boundary}--\r\n".encode()
        return self._do("POST", path, query, bytes(body), f"multipart/form-data; boundary={boundary}")
