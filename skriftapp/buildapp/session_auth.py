"""Shared helpers for the user established by a session or reusable auth guard."""

from __future__ import annotations

from uuid import UUID

from litestar import Request
from litestar.exceptions import NotAuthorizedException
from litestar.response import Redirect

from skrift.auth.session_keys import SESSION_USER_ID

DESKTOP_USER_STATE_KEY = "build_user_id"
LOGIN_PATH_TEMPLATE = "/auth/login?next={next_path}"


def login_redirect(next_path: str) -> Redirect:
    """Hand the visitor to Skrift's login and get them back to ``next_path``. Skrift
    stores ``next`` in its own session key and honours it after a sign-in AND after a
    passkey account creation, so a guest with no account yet makes one and lands where
    they were headed."""
    return Redirect(LOGIN_PATH_TEMPLATE.format(next_path=next_path))


def session_user_id(request: Request) -> UUID | None:
    """The current session's user id, or ``None`` when absent or malformed."""
    raw_user_id = request.session.get(SESSION_USER_ID)
    if not raw_user_id:
        return None
    try:
        return UUID(str(raw_user_id))
    except ValueError:
        return None


def guarded_desktop_user_id(request: Request) -> UUID | None:
    raw_user_id = request.scope.get("state", {}).get(DESKTOP_USER_STATE_KEY)
    if not raw_user_id:
        return None
    try:
        return UUID(str(raw_user_id))
    except ValueError:
        return None


def require_user(request: Request) -> UUID:
    """The user established by the session or the route's reusable guard."""
    user_id = session_user_id(request) or guarded_desktop_user_id(request)
    if user_id is None:
        raise NotAuthorizedException("Authentication required")
    return user_id
