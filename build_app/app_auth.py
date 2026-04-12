"""App authentication controller — device-auth style flow for desktop apps.

Electron (or any desktop app) can't use OAuth directly because the user
isn't logged into Google/GitHub inside the app's Chromium.  Instead:

1. App calls POST /api/app-auth/init → gets {code, auth_url}
2. App opens auth_url in the system browser
3. User authenticates in browser, clicks Approve
4. Server generates a one-time token and signals the app via SSE
5. App navigates to /api/app-auth/exchange/{token} → session cookie is set
"""

from __future__ import annotations

import asyncio
import json
import logging
import secrets
import time
from collections.abc import AsyncGenerator
from dataclasses import dataclass, field

from litestar import Controller, Request, get, post
from litestar.exceptions import NotFoundException
from litestar.response import Redirect, Response, Template
from litestar.response.sse import ServerSentEvent, ServerSentEventMessage

from skrift.auth.session_keys import (
    SESSION_USER_EMAIL,
    SESSION_USER_ID,
    SESSION_USER_NAME,
    SESSION_USER_PICTURE_URL,
)

logger = logging.getLogger(__name__)

PENDING_EXPIRY_S = 300  # 5 minutes
TOKEN_EXPIRY_S = 60  # 1 minute to exchange


@dataclass
class PendingAppAuth:
    code: str
    created_at: float = field(default_factory=time.time)
    result_queue: asyncio.Queue = field(default_factory=asyncio.Queue)


@dataclass
class AppAuthToken:
    token: str
    user_id: str
    user_name: str
    user_email: str
    user_picture_url: str | None
    created_at: float = field(default_factory=time.time)


_pending: dict[str, PendingAppAuth] = {}
_tokens: dict[str, AppAuthToken] = {}


def _cleanup():
    now = time.time()
    expired_pending = [c for c, p in _pending.items() if now - p.created_at > PENDING_EXPIRY_S]
    for c in expired_pending:
        _pending.pop(c, None)
    expired_tokens = [t for t, tok in _tokens.items() if now - tok.created_at > TOKEN_EXPIRY_S]
    for t in expired_tokens:
        _tokens.pop(t, None)


class AppAuthController(Controller):
    """Desktop app authentication flow."""

    path = "/api/app-auth"

    @post("/init", exclude_from_auth=True, status_code=201)
    async def init_app_auth(self, request: Request) -> Response:
        """Start an app authentication flow. Returns a code and auth URL."""
        _cleanup()

        code = secrets.token_urlsafe(32)
        _pending[code] = PendingAppAuth(code=code)

        base = str(request.base_url).rstrip("/")
        auth_url = f"{base}/app-auth/approve/{code}"

        return Response(
            content={"code": code, "auth_url": auth_url},
            status_code=201,
        )

    @get("/{code:str}/events", exclude_from_auth=True)
    async def app_auth_events(self, code: str) -> ServerSentEvent:
        """SSE stream for a pending app auth. Yields approved + token or expired."""
        pending = _pending.get(code)
        if not pending:
            raise NotFoundException(detail="Auth request not found or expired")

        async def generate() -> AsyncGenerator[ServerSentEventMessage, None]:
            yield ServerSentEventMessage(
                data=json.dumps({"type": "waiting"}),
                event="status",
            )

            while True:
                try:
                    result = await asyncio.wait_for(pending.result_queue.get(), timeout=15.0)
                    yield ServerSentEventMessage(
                        data=json.dumps(result),
                        event=result.get("type", "notification"),
                    )
                    return
                except asyncio.TimeoutError:
                    if time.time() - pending.created_at > PENDING_EXPIRY_S:
                        yield ServerSentEventMessage(
                            data=json.dumps({"type": "expired"}),
                            event="expired",
                        )
                        _pending.pop(code, None)
                        return
                    yield ServerSentEventMessage(comment="keepalive")

        return ServerSentEvent(generate())

    @get("/exchange/{token:str}", exclude_from_auth=True)
    async def exchange_token(self, request: Request, token: str) -> Redirect:
        """Exchange a one-time token for a session. Sets session cookie."""
        _cleanup()

        auth_token = _tokens.pop(token, None)
        if not auth_token:
            raise NotFoundException(detail="Token not found or expired")

        if time.time() - auth_token.created_at > TOKEN_EXPIRY_S:
            raise NotFoundException(detail="Token expired")

        # Set session
        request.session.clear()
        request.session[SESSION_USER_ID] = auth_token.user_id
        request.session[SESSION_USER_NAME] = auth_token.user_name
        request.session[SESSION_USER_EMAIL] = auth_token.user_email
        request.session[SESSION_USER_PICTURE_URL] = auth_token.user_picture_url

        logger.info("App auth exchange: user %s", auth_token.user_email)
        return Redirect(path="/dashboard/")


class AppAuthPageController(Controller):
    """Browser-facing approval page for app auth."""

    path = "/app-auth"

    @get("/approve/{code:str}", exclude_from_auth=True)
    async def approve_page(self, request: Request, code: str) -> Template | Redirect:
        """Show approval page. Redirects to login if not authenticated."""
        _cleanup()

        user_id = request.session.get(SESSION_USER_ID)
        if not user_id:
            return Redirect(path=f"/auth/login?next=/app-auth/approve/{code}")

        pending = _pending.get(code)
        if not pending:
            raise NotFoundException(detail="Auth request not found or expired")

        expires_in = max(0, int(PENDING_EXPIRY_S - (time.time() - pending.created_at)))
        user_name = request.session.get(SESSION_USER_NAME, "")

        return Template(
            "app_auth_approve.html",
            context={
                "code": code,
                "user_name": user_name,
                "expires_in_s": expires_in,
            },
        )

    @post("/approve/{code:str}", exclude_from_auth=True)
    async def approve_action(self, request: Request, code: str) -> Response:
        """Approve the app auth request. Generates a one-time token."""
        _cleanup()

        user_id = request.session.get(SESSION_USER_ID)
        if not user_id:
            return Response(content={"error": "not authenticated"}, status_code=401)

        pending = _pending.get(code)
        if not pending:
            return Response(content={"error": "not found or expired"}, status_code=404)

        token = secrets.token_urlsafe(32)
        _tokens[token] = AppAuthToken(
            token=token,
            user_id=user_id,
            user_name=request.session.get(SESSION_USER_NAME, ""),
            user_email=request.session.get(SESSION_USER_EMAIL, ""),
            user_picture_url=request.session.get(SESSION_USER_PICTURE_URL),
        )

        await pending.result_queue.put({
            "type": "approved",
            "token": token,
        })
        _pending.pop(code, None)

        logger.info("App auth approved for user %s", request.session.get(SESSION_USER_EMAIL))

        return Response(content={"ok": True}, status_code=200)
