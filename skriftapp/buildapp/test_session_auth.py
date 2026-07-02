"""The shared session helpers: one canonical read of the Skrift session's user id
(``SESSION_USER_ID``), tolerant of a malformed value — a garbage session must mean
"not logged in" (401/redirect), never an unhandled ``ValueError`` → 500."""

from __future__ import annotations

from types import SimpleNamespace
from uuid import UUID, uuid4

import pytest
from litestar.exceptions import NotAuthorizedException

from skrift.auth.session_keys import SESSION_USER_ID

from buildapp.session_auth import require_user, session_user_id


def _request_with_session(session: dict) -> SimpleNamespace:
    return SimpleNamespace(session=session)


def test_session_user_id_returns_the_uuid():
    user_id = uuid4()
    request = _request_with_session({SESSION_USER_ID: str(user_id)})
    assert session_user_id(request) == user_id


def test_session_user_id_is_none_when_absent():
    assert session_user_id(_request_with_session({})) is None


def test_session_user_id_is_none_for_a_malformed_value():
    request = _request_with_session({SESSION_USER_ID: "not-a-uuid"})
    assert session_user_id(request) is None


def test_require_user_returns_the_uuid():
    user_id = uuid4()
    request = _request_with_session({SESSION_USER_ID: str(user_id)})
    assert require_user(request) == UUID(str(user_id))


def test_require_user_raises_not_authorized_for_missing_or_malformed():
    with pytest.raises(NotAuthorizedException):
        require_user(_request_with_session({}))
    with pytest.raises(NotAuthorizedException):
        require_user(_request_with_session({SESSION_USER_ID: "not-a-uuid"}))
