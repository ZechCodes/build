"""Account-scoped bridge connection policy at Skrift's response boundary.

Runs inside Skrift's security middleware and session layer. The stock middleware
therefore sees this single enforced CSP and still supplies every other security
header. Origins are permission to attempt discovery, never device authority.
"""
from __future__ import annotations

from datetime import datetime
from uuid import UUID

from litestar import Request
from litestar.types import ASGIApp, Receive, Scope, Send
from skrift.config import get_settings
from skrift.middleware.security import apply_csp_nonce
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.bridge_http_proofs import canonical_origin
from buildapp.clock import utc_now
from buildapp.desktop_auth import auth_session_maker, bridge_http_client_binding
from buildapp.session_auth import require_user, session_user_id

MAX_DOCUMENT_ENDPOINTS = 16
DOCUMENT_PATHS = frozenset({"/app/", "/app", "/app/desktop"})


def connect_policy(base: str, origins: list[str]) -> str:
    """Append concrete origins while retaining every unrelated directive."""
    directives = [part.strip() for part in base.split(";") if part.strip()]
    for index, directive in enumerate(directives):
        tokens = directive.split()
        if tokens[0].lower() == "connect-src":
            sources = tokens[1:]
            # 'none' only works as a sole source and cannot accompany origins.
            if origins:
                sources = [source for source in sources if source != "'none'"]
            directives[index] = " ".join([tokens[0], *dict.fromkeys([*sources, *origins])])
            return "; ".join(directives)
    default = next((part.split()[1:] for part in directives if part.split()[0].lower() == "default-src"), ["'self'"])
    if origins:
        default = [source for source in default if source != "'none'"]
    directives.append(" ".join(["connect-src", *dict.fromkeys([*default, *origins])]))
    return "; ".join(directives)


async def document_origins(
    db: AsyncSession, user_id: UUID, client_binding: str, challenge_id: str | None, now: datetime
) -> list[str]:
    """At most sixteen current owned endpoints and one live initiating candidate.

    The device join rechecks current ownership/approval, so stale registration
    records cannot carry an old owner's origin into a new document.
    """
    from buildapp.models import BridgeHttpChallenge, BridgeHttpEndpoint, Device

    result = await db.execute(
        select(BridgeHttpEndpoint.origin)
        .join(Device, Device.id == BridgeHttpEndpoint.device_id)
        .where(
            BridgeHttpEndpoint.user_id == user_id,
            BridgeHttpEndpoint.enabled.is_(True),
            BridgeHttpEndpoint.authorization_epoch == Device.authorization_epoch,
            Device.owner_user_id == user_id,
            Device.approved.is_(True),
        )
        .order_by(BridgeHttpEndpoint.device_id)
        .limit(MAX_DOCUMENT_ENDPOINTS)
    )
    origins = list(dict.fromkeys(result.scalars()))
    try:
        selected_id = UUID(challenge_id) if challenge_id else None
    except (ValueError, TypeError, AttributeError):
        selected_id = None
    if selected_id is not None:
        pending = await db.scalar(
            select(BridgeHttpChallenge.origin).where(
                BridgeHttpChallenge.id == selected_id,
                BridgeHttpChallenge.purpose == "discovery",
                BridgeHttpChallenge.user_id == user_id,
                BridgeHttpChallenge.client_binding == client_binding,
                BridgeHttpChallenge.consumed_at.is_(None),
                BridgeHttpChallenge.expires_at > now,
            )
        )
        if pending and pending not in origins:
            origins.append(pending)
    return _valid_origins(origins)


def _valid_origins(origins: list[str]) -> list[str]:
    valid = []
    for origin in origins:
        try:
            normalized = canonical_origin(origin)
        except (ValueError, TypeError):
            continue
        if normalized == origin and origin not in valid:
            valid.append(origin)
    return valid


class BuildBridgeCSPMiddleware:
    """Modify only successful authenticated app HTML in the real framework stack."""

    def __init__(self, app: ASGIApp) -> None:
        self.app = app
        self.settings = get_settings().security_headers

    async def __call__(self, scope: Scope, receive: Receive, send: Send) -> None:
        if scope["type"] != "http" or scope["path"] not in DOCUMENT_PATHS:
            await self.app(scope, receive, send)
            return

        async def send_document(message: dict) -> None:
            if message["type"] == "http.response.start" and message["status"] == 200:
                await self._document_headers(scope, message)
            await send(message)

        await self.app(scope, receive, send_document)

    async def _document_headers(self, scope: Scope, message: dict) -> None:
        headers = list(message.get("headers", []))
        if not any(name.lower() == b"content-type" and value.startswith(b"text/html") for name, value in headers):
            return
        request = Request(scope)
        user_id = require_user(request)
        binding = bridge_http_client_binding(request)
        async with auth_session_maker(request)() as db:
            origins = await document_origins(
                db, user_id, binding, request.query_params.get("bridge_discovery"), utc_now()
            )
        if self.settings.enabled and self.settings.content_security_policy:
            existing = next((value.decode() for name, value in headers if name.lower() == b"content-security-policy"), None)
            base = existing or self.settings.content_security_policy
            nonce = scope.get("state", {}).get("csp_nonce")
            if not existing and nonce:
                base = apply_csp_nonce(base, nonce)
            headers = [(name, value) for name, value in headers if name.lower() != b"content-security-policy"]
            headers.append((b"content-security-policy", connect_policy(base, origins).encode()))
        headers = [(name, value) for name, value in headers if name.lower() not in {b"cache-control", b"referrer-policy"}]
        headers.extend([(b"cache-control", b"private, no-store"), (b"referrer-policy", b"no-referrer")])
        message["headers"] = headers


async def revoke_browser_client(request: Request, user_id: UUID | None, binding: str | None) -> None:
    """Revoke only the family whose CSRF-protected logout actually completed."""
    if user_id is None or binding is None or session_user_id(request) is not None:
        return
    from buildapp.bridge_http_auth import revoke_client

    async with auth_session_maker(request)() as db:
        await revoke_client(db, user_id, binding, utc_now())
        await db.commit()
