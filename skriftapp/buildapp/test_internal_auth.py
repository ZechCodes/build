"""Tests for the relay-facing internal-endpoint guard: a shared secret in the
``X-Internal-Secret`` header (constant-time compared against env
``INTERNAL_API_SECRET``), with an opt-in localhost fallback for dev configs."""

from __future__ import annotations

import asyncio
from unittest.mock import MagicMock, patch

import pytest
from litestar.exceptions import NotFoundException

from buildapp import internal_auth
from buildapp.internal_auth import (
    INTERNAL_SECRET_HEADER,
    InternalApiConfig,
    internal_auth_guard,
    internal_request_allowed,
)


# ----- internal_request_allowed (pure decision logic) -------------------------


def test_matching_secret_is_allowed():
    assert internal_request_allowed(
        presented_secret="s3cret",
        configured_secret="s3cret",
        client_ip="10.0.0.7",
        allow_localhost=False,
    )


def test_wrong_secret_is_refused():
    assert not internal_request_allowed(
        presented_secret="wrong",
        configured_secret="s3cret",
        client_ip="10.0.0.7",
        allow_localhost=False,
    )


def test_missing_header_is_refused():
    assert not internal_request_allowed(
        presented_secret=None,
        configured_secret="s3cret",
        client_ip="10.0.0.7",
        allow_localhost=False,
    )


def test_unconfigured_secret_never_matches_empty_header():
    # Fail closed: no configured secret + no fallback means nothing gets in,
    # even a request presenting an empty string.
    assert not internal_request_allowed(
        presented_secret="",
        configured_secret=None,
        client_ip="127.0.0.1",
        allow_localhost=False,
    )
    assert not internal_request_allowed(
        presented_secret="",
        configured_secret="",
        client_ip="127.0.0.1",
        allow_localhost=False,
    )


def test_localhost_fallback_requires_opt_in():
    assert not internal_request_allowed(
        presented_secret=None,
        configured_secret=None,
        client_ip="127.0.0.1",
        allow_localhost=False,
    )
    assert internal_request_allowed(
        presented_secret=None,
        configured_secret=None,
        client_ip="127.0.0.1",
        allow_localhost=True,
    )
    assert internal_request_allowed(
        presented_secret=None,
        configured_secret=None,
        client_ip="::1",
        allow_localhost=True,
    )


def test_localhost_fallback_rejects_remote_callers():
    assert not internal_request_allowed(
        presented_secret=None,
        configured_secret=None,
        client_ip="203.0.113.9",
        allow_localhost=True,
    )


# ----- internal_auth_guard (Litestar wiring) ----------------------------------


def _connection(*, headers: dict, client_ip: str) -> MagicMock:
    connection = MagicMock()
    connection.headers = headers
    connection.scope = {"state": {"client_ip": client_ip}, "client": (client_ip, 4242)}
    return connection


def _settings(allow_localhost: bool) -> MagicMock:
    settings = MagicMock()
    settings.internal_api = InternalApiConfig(allow_localhost=allow_localhost)
    return settings


def _run_guard(connection) -> None:
    asyncio.run(internal_auth_guard(connection, MagicMock()))


def test_guard_accepts_matching_secret_header():
    connection = _connection(
        headers={INTERNAL_SECRET_HEADER: "relay-secret"}, client_ip="10.1.2.3"
    )
    with (
        patch.dict(internal_auth.os.environ, {"INTERNAL_API_SECRET": "relay-secret"}),
        patch.object(internal_auth, "get_settings", return_value=_settings(False)),
    ):
        _run_guard(connection)  # must not raise


def test_guard_hides_route_from_wrong_secret():
    connection = _connection(
        headers={INTERNAL_SECRET_HEADER: "nope"}, client_ip="10.1.2.3"
    )
    with (
        patch.dict(internal_auth.os.environ, {"INTERNAL_API_SECRET": "relay-secret"}),
        patch.object(internal_auth, "get_settings", return_value=_settings(False)),
    ):
        with pytest.raises(NotFoundException):
            _run_guard(connection)


def test_guard_dev_localhost_fallback():
    connection = _connection(headers={}, client_ip="127.0.0.1")
    with (
        patch.dict(internal_auth.os.environ, {}, clear=True),
        patch.object(internal_auth, "get_settings", return_value=_settings(True)),
    ):
        _run_guard(connection)  # must not raise


def test_guard_refuses_localhost_when_fallback_disabled():
    connection = _connection(headers={}, client_ip="127.0.0.1")
    with (
        patch.dict(internal_auth.os.environ, {}, clear=True),
        patch.object(internal_auth, "get_settings", return_value=_settings(False)),
    ):
        with pytest.raises(NotFoundException):
            _run_guard(connection)
