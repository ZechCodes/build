"""Middleware — install script serving."""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from litestar.types import ASGIApp, Receive, Scope, Send


_INSTALL_SCRIPT = (Path(__file__).resolve().parent.parent / "install.sh").read_bytes()
_CLI_USER_AGENTS = (b"curl", b"wget", b"httpie", b"fetch", b"powershell")


def create_install_middleware(app: "ASGIApp") -> "ASGIApp":
    """ASGI middleware that serves install.sh to CLI clients requesting /."""

    async def middleware(scope: "Scope", receive: "Receive", send: "Send") -> None:
        if scope["type"] == "http" and scope["path"] == "/":
            headers = dict(scope.get("headers", []))
            ua = headers.get(b"user-agent", b"").lower()
            if any(agent in ua for agent in _CLI_USER_AGENTS):
                await send({
                    "type": "http.response.start",
                    "status": 200,
                    "headers": [
                        [b"content-type", b"text/x-shellscript; charset=utf-8"],
                        [b"content-disposition", b"inline; filename=install.sh"],
                        [b"content-length", str(len(_INSTALL_SCRIPT)).encode()],
                    ],
                })
                await send({
                    "type": "http.response.body",
                    "body": _INSTALL_SCRIPT,
                })
                return
        await app(scope, receive, send)

    return middleware
