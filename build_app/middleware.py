"""Middleware — install script serving and relay lifecycle."""

from __future__ import annotations

import asyncio
import logging
import os
from pathlib import Path
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from litestar.types import ASGIApp, Receive, Scope, Send

logger = logging.getLogger(__name__)

_INSTALL_SCRIPT = (Path(__file__).resolve().parent.parent / "install.sh").read_bytes()
_CLI_USER_AGENTS = (b"curl", b"wget", b"httpie", b"fetch", b"powershell")


_relay_initialized = False


def create_relay_lifecycle_middleware(app: "ASGIApp") -> "ASGIApp":
    """ASGI middleware that starts/stops the relay Redis stream consumer on lifespan."""

    async def middleware(scope: "Scope", receive: "Receive", send: "Send") -> None:
        global _relay_initialized
        if scope["type"] == "lifespan":
            # Wrap the inner lifespan to add relay init/close.
            async def wrapped_receive():
                return await receive()

            async def wrapped_send(message):
                if message["type"] == "lifespan.startup.complete":
                    redis_url = os.environ.get("REDIS_URL", "")
                    if redis_url:
                        try:
                            from build_app.devices._relay import init_relay
                            # Get session maker from app state (set by Skrift after startup).
                            litestar_app = scope.get("app")
                            session_maker = getattr(litestar_app.state, "session_maker_class", None) if litestar_app else None
                            if session_maker:
                                await init_relay(session_maker)
                                _relay_initialized = True
                                logger.info("Relay stream consumer started")
                            else:
                                logger.warning("No session_maker found, relay consumer not started")
                        except Exception:
                            logger.exception("Failed to start relay consumer")
                elif message["type"] == "lifespan.shutdown.complete" and _relay_initialized:
                    try:
                        from build_app.devices._relay import close_relay
                        await close_relay()
                        logger.info("Relay stream consumer stopped")
                    except Exception:
                        logger.exception("Failed to stop relay consumer")
                await send(message)

            await app(scope, wrapped_receive, wrapped_send)
        else:
            await app(scope, receive, send)

    return middleware


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
