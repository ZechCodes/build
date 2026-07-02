"""Shared session helpers — the one canonical read of the Skrift session's user
id. ``session_user_id`` answers "who, if anyone, is logged in" (for handlers that
redirect anonymous visitors); ``require_user`` is the belt-and-suspenders read for
routes behind ``auth_guard``. A malformed session value means "not logged in" —
never an unhandled ``ValueError`` → 500."""

from __future__ import annotations

from uuid import UUID

from litestar import Request
from litestar.exceptions import NotAuthorizedException

from skrift.auth.session_keys import SESSION_USER_ID


def session_user_id(request: Request) -> UUID | None:
    """The current session's user id, or ``None`` when absent or malformed."""
    raw_user_id = request.session.get(SESSION_USER_ID)
    if not raw_user_id:
        return None
    try:
        return UUID(str(raw_user_id))
    except ValueError:
        return None


def require_user(request: Request) -> UUID:
    """The current session's user id. Routes are guarded by ``auth_guard``; this
    is the belt-and-suspenders read of what the guard verified."""
    user_id = session_user_id(request)
    if user_id is None:
        raise NotAuthorizedException("Authentication required")
    return user_id
