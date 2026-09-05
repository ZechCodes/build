"""Shared authentication for Build clients, plus the alpha gate every route carrying
one of these guards sits behind.

Two questions, in order: who is this, and are they in the alpha
(``alpha_membership``). The first is a table — a browser session, this app's desktop
OAuth token, and on the download routes a short-lived download token in the query
string, because ``curl … | sh`` carries no cookies. They are walked in order and the
first that answers wins; a new way in is a new entry, never a branch.

The second question is asked of whoever answered, always: a token is an identity and
never an authorisation, so a revoked invite closes every route in the minutes a live
token still has to run. Anonymous is 401; a signed-in account with no redeemed invite
is 403 ``invite only``. Device approval rides this guard, so an uninvited account
cannot pair a bridge."""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from uuid import UUID

from litestar.connection import ASGIConnection
from litestar.exceptions import NotAuthorizedException
from litestar.handlers import BaseRouteHandler

from skrift.config import get_settings
from skrift.controllers.oauth2 import verify_oauth_token

from buildapp import download_tokens
from buildapp.alpha_membership import require_alpha_member
from buildapp.clock import utc_now
from buildapp.releases import DOWNLOAD_TOKEN_PARAM
from buildapp.session_auth import DESKTOP_USER_STATE_KEY, session_user_id

DESKTOP_CLIENT_ID = "build-desktop"

#: One way of saying who the caller is: the account it names, or ``None``.
Identifier = Callable[[ASGIConnection], Awaitable[UUID | None]]


def desktop_token_user_id(payload: dict | None) -> UUID | None:
    if not payload or payload.get("type") != "access":
        return None
    if payload.get("client_id") != DESKTOP_CLIENT_ID:
        return None
    if "openid" not in str(payload.get("scope", "")).split():
        return None
    try:
        return UUID(str(payload["user_id"]))
    except (KeyError, ValueError):
        return None


async def session_user(connection: ASGIConnection) -> UUID | None:
    """The user behind a normal Skrift browser session."""
    return session_user_id(connection)


async def bearer_user(connection: ASGIConnection) -> UUID | None:
    """The user behind this app's OAuth access token. No bearer at all, or a bearer
    that verifies but is not one of this app's desktop tokens, names nobody."""
    authorization = connection.headers.get("authorization", "")
    if not authorization.startswith("Bearer "):
        return None
    token = authorization.removeprefix("Bearer ").strip()
    session_maker = connection.app.state.session_maker_class
    async with session_maker() as db_session:
        payload = await verify_oauth_token(token, get_settings().secret_key, db_session)
    return desktop_token_user_id(payload)


async def download_token_user(connection: ASGIConnection) -> UUID | None:
    """The member whose install one-liner is making this request. Whether the token
    is even the right shape is ``download_tokens``' judgement, not a second copy of
    the rule here — it checks before it reads, so junk still costs no query."""
    raw = connection.query_params.get(DOWNLOAD_TOKEN_PARAM)
    session_maker = connection.app.state.session_maker_class
    async with session_maker() as db_session:
        return await download_tokens.holder(db_session, raw, utc_now())


#: What a browser-facing route accepts.
BROWSER_IDENTIFIERS: tuple[Identifier, ...] = (session_user, bearer_user)
#: What a download route accepts: the same, plus the install line's own token.
DOWNLOAD_IDENTIFIERS: tuple[Identifier, ...] = (*BROWSER_IDENTIFIERS, download_token_user)


async def _admit(
    connection: ASGIConnection, identifiers: tuple[Identifier, ...]
) -> None:
    """Identify the caller, require that they are an alpha member, and record them
    where ``require_user`` will find them in the handler."""
    user_id = None
    for identify in identifiers:
        user_id = await identify(connection)
        if user_id is not None:
            break
    if user_id is None:
        raise NotAuthorizedException("Authentication required")

    session_maker = connection.app.state.session_maker_class
    async with session_maker() as db_session:
        await require_alpha_member(db_session, user_id)
    connection.scope.setdefault("state", {})[DESKTOP_USER_STATE_KEY] = str(user_id)


async def build_auth_guard(
    connection: ASGIConnection, _route_handler: BaseRouteHandler
) -> None:
    """A browser session or this app's desktop OAuth token, then alpha membership."""
    await _admit(connection, BROWSER_IDENTIFIERS)


async def download_auth_guard(
    connection: ASGIConnection, _route_handler: BaseRouteHandler
) -> None:
    """The same, or a live download token — the install script has no session."""
    await _admit(connection, DOWNLOAD_IDENTIFIERS)
