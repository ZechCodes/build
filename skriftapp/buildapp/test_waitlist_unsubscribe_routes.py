"""Tests for the two unsubscribe routes: the GET only asks (mail scanners prefetch
links, so it must never mutate), the POST removes the row the signed token names and is
idempotent, and an unreadable token is a 404 page on both methods."""

from __future__ import annotations

import asyncio
import inspect

from litestar.enums import MediaType
from litestar.status_codes import HTTP_200_OK, HTTP_404_NOT_FOUND

from buildapp.email_test_support import SECRET_KEY, waitlist_email_context
from buildapp.unsubscribe_pages import CONFIRM_MESSAGE, INVALID_HEADING, REMOVED_HEADING
from buildapp.waitlist_controller import UNSUBSCRIBE_ROUTE_PATH, WaitlistController
from buildapp.waitlist_unsubscribe_token import mint_unsubscribe_token

SIGNER_ADDRESS = "alice@example.com"
UNREADABLE_TOKEN = "not.asignedtoken"

EMAIL_CONTEXT = waitlist_email_context()


class _StubSession:
    def __init__(self):
        self.executed: list = []
        self.committed = False

    async def execute(self, statement):
        self.executed.append(statement)
        return None

    async def commit(self):
        self.committed = True


def _valid_token(address: str = SIGNER_ADDRESS) -> str:
    return mint_unsubscribe_token(address, SECRET_KEY)


def _confirm(token: str):
    return asyncio.run(
        WaitlistController.unsubscribe_confirm.fn(
            None, unsubscribe_token=token, waitlist_email_context=EMAIL_CONTEXT
        )
    )


def _remove(token: str, session: _StubSession):
    return asyncio.run(
        WaitlistController.unsubscribe_remove.fn(
            None,
            unsubscribe_token=token,
            waitlist_email_context=EMAIL_CONTEXT,
            db_session=session,
        )
    )


def _bound_parameters(statement) -> list:
    return list(statement.compile().params.values())


def test_get_with_valid_token_renders_the_confirm_page_with_200():
    response = _confirm(_valid_token())
    assert response.media_type == MediaType.HTML
    assert SIGNER_ADDRESS in response.content
    assert CONFIRM_MESSAGE in response.content
    assert response.status_code is None
    assert WaitlistController.unsubscribe_confirm.status_code == HTTP_200_OK


def test_get_never_touches_the_database():
    parameters = inspect.signature(WaitlistController.unsubscribe_confirm.fn).parameters
    assert "db_session" not in parameters


def test_get_with_bad_token_renders_the_invalid_page_with_404():
    response = _confirm(UNREADABLE_TOKEN)
    assert response.status_code == HTTP_404_NOT_FOUND
    assert INVALID_HEADING in response.content


def test_post_with_valid_token_deletes_the_exact_normalised_address_and_commits():
    session = _StubSession()
    response = _remove(_valid_token("  Alice@Example.COM "), session)
    assert len(session.executed) == 1
    assert _bound_parameters(session.executed[0]) == [SIGNER_ADDRESS]
    assert session.committed is True
    assert REMOVED_HEADING in response.content


def test_post_for_an_address_with_no_row_still_renders_the_removed_page_with_200():
    session = _StubSession()
    response = _remove(_valid_token("nobody@example.com"), session)
    assert response.status_code is None
    assert WaitlistController.unsubscribe_remove.status_code == HTTP_200_OK
    assert REMOVED_HEADING in response.content


def test_post_with_bad_token_renders_the_invalid_page_with_404_and_no_database_call():
    session = _StubSession()
    response = _remove(UNREADABLE_TOKEN, session)
    assert response.status_code == HTTP_404_NOT_FOUND
    assert INVALID_HEADING in response.content
    assert session.executed == []
    assert session.committed is False


def test_post_reads_no_request_body():
    parameters = inspect.signature(WaitlistController.unsubscribe_remove.fn).parameters
    assert "data" not in parameters
    assert "request" not in parameters


def test_routes_are_registered_at_the_unsubscribe_path_with_get_and_post():
    assert set(WaitlistController.unsubscribe_confirm.paths) == {UNSUBSCRIBE_ROUTE_PATH}
    assert "GET" in WaitlistController.unsubscribe_confirm.http_methods
    assert set(WaitlistController.unsubscribe_remove.paths) == {UNSUBSCRIBE_ROUTE_PATH}
    assert "POST" in WaitlistController.unsubscribe_remove.http_methods
    assert WaitlistController.unsubscribe_remove.status_code == HTTP_200_OK


def test_routes_carry_no_guard():
    assert not WaitlistController.unsubscribe_confirm.guards
    assert not WaitlistController.unsubscribe_remove.guards
