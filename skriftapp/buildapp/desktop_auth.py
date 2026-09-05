"""Shared browser-session and Skrift OAuth authentication for Build clients, plus the
alpha gate every route carrying this guard sits behind.

Two questions, in order: who is this (session or desktop OAuth token), and are they in
the alpha (``alpha_membership``). Anonymous is 401; a signed-in account with no redeemed
invite is 403 ``invite only``. Device approval rides this guard, so an uninvited account
cannot pair a bridge."""

from __future__ import annotations

from uuid import UUID

from litestar.connection import ASGIConnection
from litestar.exceptions import NotAuthorizedException
from litestar.handlers import BaseRouteHandler

from skrift.config import get_settings
from skrift.controllers.oauth2 import verify_oauth_token

from buildapp.alpha_membership import require_alpha_member
from buildapp.session_auth import DESKTOP_USER_STATE_KEY, session_user_id

DESKTOP_CLIENT_ID = "build-desktop"


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


async def _bearer_user_id(connection: ASGIConnection) -> UUID | None:
    """The user behind this app's OAuth access token, or ``None`` when there is no
    usable bearer at all."""
    authorization = connection.headers.get("authorization", "")
    if not authorization.startswith("Bearer "):
        raise NotAuthorizedException("Authentication required")
    token = authorization.removeprefix("Bearer ").strip()
    session_maker = connection.app.state.session_maker_class
    async with session_maker() as db_session:
        payload = await verify_oauth_token(token, get_settings().secret_key, db_session)
    return desktop_token_user_id(payload)


async def build_auth_guard(
    connection: ASGIConnection, _route_handler: BaseRouteHandler
) -> None:
    """Accept a normal Skrift session or this app's OAuth access token, then require
    that whoever it is holds a redeemed invite."""
    user_id = session_user_id(connection)
    if user_id is None:
        user_id = await _bearer_user_id(connection)
    if user_id is None:
        raise NotAuthorizedException("Authentication required")

    session_maker = connection.app.state.session_maker_class
    async with session_maker() as db_session:
        await require_alpha_member(db_session, user_id)
    connection.scope.setdefault("state", {})[DESKTOP_USER_STATE_KEY] = str(user_id)
