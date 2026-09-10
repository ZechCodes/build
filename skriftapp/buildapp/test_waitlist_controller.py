"""Tests for the public waitlist endpoint: a new address is persisted normalised and
committed, an address already on the list answers identically, a malformed or
wrong-shaped body is a 400 rather than an unhandled error, and the confirmation mail is
scheduled as a background task only on the path where the row actually landed."""

from __future__ import annotations

import asyncio

import pytest
from litestar.background_tasks import BackgroundTask
from litestar.exceptions import ClientException, SerializationException
from litestar.status_codes import HTTP_200_OK
from sqlalchemy.exc import IntegrityError

from buildapp.email_test_support import (
    OWNER_ADDRESS,
    FailingEmailBackend,
    RecordingEmailBackend,
    waitlist_email_context,
)
from buildapp.models import WaitlistSignup
from buildapp.waitlist_controller import JOIN_ROUTE_PATH, WaitlistController
from buildapp.waitlist_mail import CONFIRMATION_SUBJECT, OWNER_SUBJECT_PREFIX

_DUPLICATE_EMAIL_ERROR = IntegrityError("INSERT", {}, Exception("duplicate"))

NORMALISED_EMAIL = "alice@example.com"
MIXED_CASE_EMAIL = "  Alice@Example.COM "


class _StubSession:
    def __init__(self, commit_error=None):
        self._commit_error = commit_error
        self.added: list = []
        self.committed = False
        self.rolled_back = False

    def add(self, instance):
        self.added.append(instance)

    async def commit(self):
        if self._commit_error is not None:
            raise self._commit_error
        self.committed = True

    async def rollback(self):
        self.rolled_back = True


class _StubRequest:
    def __init__(self, body):
        self._body = body

    async def json(self):
        return self._body


class _MalformedRequest:
    async def json(self):
        raise SerializationException("JSON is malformed")


def _join(body, session, *, context=None, email_backend=None):
    return asyncio.run(
        WaitlistController.join.fn(
            None,
            request=_StubRequest(body),
            db_session=session,
            waitlist_email_context=context or waitlist_email_context(),
            email_backend=email_backend or RecordingEmailBackend(),
        )
    )


def test_new_email_is_persisted_normalized():
    session = _StubSession()
    _join({"email": "  Alice@Example.COM "}, session)
    assert len(session.added) == 1
    signup = session.added[0]
    assert isinstance(signup, WaitlistSignup)
    assert signup.email == "alice@example.com"


def test_new_signup_commits_the_session():
    session = _StubSession()
    _join({"email": "alice@example.com"}, session)
    assert session.committed is True


def test_new_signup_answers_ok():
    session = _StubSession()
    response = _join({"email": "alice@example.com"}, session)
    assert response.content == {"ok": True}


def test_existing_email_answers_identically_without_committing():
    session = _StubSession(commit_error=_DUPLICATE_EMAIL_ERROR)
    response = _join({"email": "Alice@example.com"}, session)
    assert session.committed is False
    assert response.content == {"ok": True}


def test_concurrent_duplicate_insert_rolls_back_and_answers_ok():
    session = _StubSession(commit_error=_DUPLICATE_EMAIL_ERROR)
    response = _join({"email": "alice@example.com"}, session)
    assert session.rolled_back is True
    assert response.content == {"ok": True}


def test_invalid_email_raises_client_exception():
    with pytest.raises(ClientException):
        _join({"email": "not-an-email"}, _StubSession())


def test_missing_email_field_raises_client_exception():
    with pytest.raises(ClientException):
        _join({}, _StubSession())


def test_non_string_email_raises_client_exception():
    with pytest.raises(ClientException):
        _join({"email": 42}, _StubSession())


def test_non_object_body_raises_client_exception():
    with pytest.raises(ClientException):
        _join(["alice@example.com"], _StubSession())


def test_malformed_body_raises_client_exception():
    with pytest.raises(ClientException):
        asyncio.run(
            WaitlistController.join.fn(
                None,
                request=_MalformedRequest(),
                db_session=_StubSession(),
                waitlist_email_context=waitlist_email_context(),
                email_backend=RecordingEmailBackend(),
            )
        )


def test_a_rejected_address_never_reaches_the_session():
    session = _StubSession()
    with pytest.raises(ClientException):
        _join({"email": {"nested": "object"}}, session)
    assert session.added == []


def test_join_route_is_public_and_carries_no_guard():
    assert not WaitlistController.join.guards


def test_join_route_is_registered_at_api_waitlist():
    assert JOIN_ROUTE_PATH == "/api/waitlist"
    assert set(WaitlistController.join.paths) == {JOIN_ROUTE_PATH}
    assert "POST" in WaitlistController.join.http_methods
    assert WaitlistController.join.status_code == HTTP_200_OK


def test_new_signup_schedules_the_confirmation():
    email_backend = RecordingEmailBackend()
    response = _join(
        {"email": MIXED_CASE_EMAIL}, _StubSession(), email_backend=email_backend
    )
    assert isinstance(response.background, BackgroundTask)
    asyncio.run(response.background())
    assert [sent.to for sent in email_backend.sent] == [NORMALISED_EMAIL]
    assert email_backend.sent[0].subject == CONFIRMATION_SUBJECT


def test_new_signup_also_notifies_the_owner_when_configured():
    email_backend = RecordingEmailBackend()
    response = _join(
        {"email": NORMALISED_EMAIL},
        _StubSession(),
        context=waitlist_email_context(notify_address=OWNER_ADDRESS),
        email_backend=email_backend,
    )
    asyncio.run(response.background())
    assert [sent.to for sent in email_backend.sent] == [NORMALISED_EMAIL, OWNER_ADDRESS]
    assert email_backend.sent[1].subject == f"{OWNER_SUBJECT_PREFIX}{NORMALISED_EMAIL}"


def test_new_signup_sends_nothing_before_the_task_runs():
    email_backend = RecordingEmailBackend()
    _join({"email": NORMALISED_EMAIL}, _StubSession(), email_backend=email_backend)
    assert email_backend.sent == []


def test_duplicate_signup_schedules_nothing():
    email_backend = RecordingEmailBackend()
    response = _join(
        {"email": NORMALISED_EMAIL},
        _StubSession(commit_error=_DUPLICATE_EMAIL_ERROR),
        email_backend=email_backend,
    )
    assert response.background is None
    assert email_backend.sent == []


def test_invalid_address_schedules_nothing():
    email_backend = RecordingEmailBackend()
    with pytest.raises(ClientException):
        _join({"email": "not-an-email"}, _StubSession(), email_backend=email_backend)
    assert email_backend.sent == []


def test_response_body_and_status_are_identical_for_new_and_duplicate():
    new_response = _join({"email": NORMALISED_EMAIL}, _StubSession())
    duplicate_response = _join(
        {"email": NORMALISED_EMAIL}, _StubSession(commit_error=_DUPLICATE_EMAIL_ERROR)
    )
    assert new_response.content == duplicate_response.content
    assert new_response.status_code == duplicate_response.status_code


def test_a_failing_backend_still_leaves_the_response_ok_and_the_row_committed():
    session = _StubSession()
    response = _join(
        {"email": NORMALISED_EMAIL}, session, email_backend=FailingEmailBackend()
    )
    asyncio.run(response.background())
    assert response.content == {"ok": True}
    assert WaitlistController.join.status_code == HTTP_200_OK
    assert session.committed is True
