"""Tests for the public waitlist endpoint: a new address is persisted normalised and
committed, an address already on the list answers identically without writing, and a
malformed body is a 400 rather than an unhandled error."""

from __future__ import annotations

import asyncio

import pytest
from litestar.exceptions import ClientException
from sqlalchemy.exc import IntegrityError

from buildapp.models import WaitlistSignup
from buildapp.waitlist_controller import WaitlistController


class _StubResult:
    def __init__(self, row):
        self._row = row

    def scalar_one_or_none(self):
        return self._row


class _StubSession:
    def __init__(self, existing_row=None, commit_error=None):
        self._existing_row = existing_row
        self._commit_error = commit_error
        self.added: list = []
        self.committed = False
        self.rolled_back = False

    async def execute(self, statement):
        return _StubResult(self._existing_row)

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


def _join(body, session):
    return asyncio.run(
        WaitlistController.join.fn(None, request=_StubRequest(body), db_session=session)
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


def test_existing_email_answers_identically_and_writes_nothing():
    session = _StubSession(existing_row=WaitlistSignup(email="alice@example.com"))
    response = _join({"email": "Alice@example.com"}, session)
    assert session.added == []
    assert session.committed is False
    assert response.content == {"ok": True}


def test_concurrent_duplicate_insert_rolls_back_and_answers_ok():
    session = _StubSession(
        commit_error=IntegrityError("INSERT", {}, Exception("duplicate"))
    )
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


def test_join_route_is_public_and_carries_no_guard():
    assert not WaitlistController.join.guards


def test_join_route_is_registered_at_api_waitlist():
    assert set(WaitlistController.join.paths) == {"/api/waitlist"}
    assert "POST" in WaitlistController.join.http_methods
    assert WaitlistController.join.status_code == 200
