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

import re
import secrets
from collections.abc import Awaitable, Callable
from uuid import UUID

from advanced_alchemy.extensions.litestar import SQLAlchemyPlugin
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
BRIDGE_BINDING_KEY = "build_bridge_http_binding"
BRIDGE_ACCOUNT_KEY = "build_bridge_http_account"
OAUTH_CLIENT_STATE_KEY = "build_oauth_client_id"
OAUTH_JTI_STATE_KEY = "build_oauth_access_jti"
OAUTH_EXP_STATE_KEY = "build_oauth_access_exp"

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


def auth_session_maker(connection: ASGIConnection):
    """Resolve the framework's configured maker, including multi-app state keys."""
    maker = getattr(connection.app.state, "session_maker_class", None)
    if maker is not None:
        return maker
    config = connection.app.plugins.get(SQLAlchemyPlugin).config[0]
    return connection.app.state[config.session_maker_app_state_key]


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
    session_maker = auth_session_maker(connection)
    async with session_maker() as db_session:
        payload = await verify_oauth_token(token, get_settings().secret_key, db_session)
    user_id = desktop_token_user_id(payload)
    if user_id is not None:
        state = connection.scope.setdefault("state", {})
        state[OAUTH_CLIENT_STATE_KEY] = DESKTOP_CLIENT_ID
        state[OAUTH_JTI_STATE_KEY] = payload.get("jti")
        state[OAUTH_EXP_STATE_KEY] = payload.get("exp")
    return user_id


async def download_token_user(connection: ASGIConnection) -> UUID | None:
    """The member whose install one-liner is making this request. Whether the token
    is even the right shape is ``download_tokens``' judgement, not a second copy of
    the rule here — it checks before it reads, so junk still costs no query."""
    raw = connection.query_params.get(DOWNLOAD_TOKEN_PARAM)
    session_maker = auth_session_maker(connection)
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

    session_maker = auth_session_maker(connection)
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


def bridge_http_client_binding(connection: ASGIConnection) -> str:
    """A cookie session family or an exact, verified desktop access-token family.

    Skrift access tokens lack an installation claim, so this binding is limited
    to their jti and expiry. Refreshing OAuth starts a new discovery family.
    """
    user_id = session_user_id(connection)
    if user_id is not None:
        if connection.session.get(BRIDGE_ACCOUNT_KEY) != str(user_id):
            connection.session[BRIDGE_ACCOUNT_KEY] = str(user_id)
            connection.session[BRIDGE_BINDING_KEY] = "browser:" + secrets.token_urlsafe(32)
        binding = connection.session.get(BRIDGE_BINDING_KEY)
        if not isinstance(binding, str) or not binding.startswith("browser:"):
            binding = "browser:" + secrets.token_urlsafe(32)
            connection.session[BRIDGE_BINDING_KEY] = binding
        return binding
    state = connection.scope.get("state", {})
    if state.get(OAUTH_CLIENT_STATE_KEY) != DESKTOP_CLIENT_ID or not state.get(DESKTOP_USER_STATE_KEY):
        raise NotAuthorizedException("Authentication required")
    jti, exp = state.get(OAUTH_JTI_STATE_KEY), state.get(OAUTH_EXP_STATE_KEY)
    if not isinstance(jti, str) or not re.fullmatch(r"[A-Za-z0-9_-]{16,64}", jti):
        raise NotAuthorizedException("Authentication required")
    if type(exp) is not int or exp <= int(utc_now().timestamp()):
        raise NotAuthorizedException("Authentication required")
    return f"desktop:{jti}:{exp}"
