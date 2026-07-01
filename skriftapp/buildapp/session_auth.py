"""Shared session helper for controllers whose routes run behind ``auth_guard``:
resolve the current user's id from the Skrift session, failing loudly if the
guard was somehow bypassed."""

from __future__ import annotations

from uuid import UUID

from litestar import Request
from litestar.exceptions import NotAuthorizedException

from skrift.auth.session_keys import SESSION_USER_ID


def require_user(request: Request) -> UUID:
    """The current session's user id. Routes are guarded by ``auth_guard``; this
    is the belt-and-suspenders read of what the guard verified."""
    user_id = request.session.get(SESSION_USER_ID)
    if not user_id:
        raise NotAuthorizedException("Authentication required")
    return UUID(user_id)
