"""Tests that drive the waitlist over a real ASGI stack rather than calling the handlers:
the dependency wiring (the email backend off app state, the context off settings), the
signed token surviving a URL path, and the one-click unsubscribe a mail client sends as a
form body."""

from __future__ import annotations

from collections.abc import Iterator

import pytest
from litestar.status_codes import HTTP_200_OK, HTTP_404_NOT_FOUND
from litestar.testing import TestClient
from sqlalchemy import select

from buildapp import waitlist_controller
from buildapp.db_test_support import asgi_app, in_memory_session_maker
from buildapp.email_test_support import OWNER_ADDRESS, SECRET_KEY, email_settings
from buildapp.models import WaitlistSignup
from buildapp.unsubscribe_pages import (
    CONFIRM_BUTTON_LABEL,
    INVALID_HEADING,
    REMOVED_HEADING,
)
from buildapp.waitlist_controller import JOIN_ROUTE_PATH, WaitlistController
from buildapp.waitlist_mail import CONFIRMATION_SUBJECT, NOTIFY_ADDRESS_ENV
from buildapp.waitlist_unsubscribe_token import mint_unsubscribe_token, unsubscribe_path

SIGNER_ADDRESS = "signer@example.com"
MIXED_CASE_ADDRESS = "  Signer@Example.COM "
ONE_CLICK_FORM_BODY = {"List-Unsubscribe": "One-Click"}


@pytest.fixture()
def client(monkeypatch, email_backend) -> Iterator[TestClient]:
    monkeypatch.setattr(waitlist_controller, "get_settings", email_settings)
    monkeypatch.setenv(NOTIFY_ADDRESS_ENV, OWNER_ADDRESS)
    app = asgi_app([WaitlistController], session_maker=in_memory_session_maker())
    app.state.email_backend = email_backend
    with TestClient(app=app) as test_client:
        yield test_client


def stored_addresses(client: TestClient) -> list[str]:
    async def read_addresses() -> list[str]:
        async with client.app.state.make_session() as session:
            result = await session.execute(select(WaitlistSignup.email))
            return list(result.scalars())

    with client.portal() as portal:
        return portal.call(read_addresses)


def signup_count(client: TestClient) -> int:
    return len(stored_addresses(client))


def test_join_delivers_the_confirmation_through_the_wired_backend(client, email_backend):
    response = client.post(JOIN_ROUTE_PATH, json={"email": MIXED_CASE_ADDRESS})
    assert response.status_code == HTTP_200_OK
    assert response.json() == {"ok": True}
    assert stored_addresses(client) == [SIGNER_ADDRESS]
    assert [sent.to for sent in email_backend.sent] == [SIGNER_ADDRESS, OWNER_ADDRESS]
    assert email_backend.sent[0].subject == CONFIRMATION_SUBJECT


def test_a_duplicate_join_answers_identically_and_sends_nothing(client, email_backend):
    first = client.post(JOIN_ROUTE_PATH, json={"email": SIGNER_ADDRESS})
    email_backend.sent.clear()
    duplicate = client.post(JOIN_ROUTE_PATH, json={"email": SIGNER_ADDRESS})
    assert (duplicate.status_code, duplicate.json()) == (first.status_code, first.json())
    assert email_backend.sent == []
    assert signup_count(client) == 1


def test_the_unsubscribe_link_from_the_confirmation_asks_before_it_removes(
    client, email_backend
):
    client.post(JOIN_ROUTE_PATH, json={"email": SIGNER_ADDRESS})
    link_path = unsubscribe_path(mint_unsubscribe_token(SIGNER_ADDRESS, SECRET_KEY))
    response = client.get(link_path)
    assert response.status_code == HTTP_200_OK
    assert SIGNER_ADDRESS in response.text
    assert CONFIRM_BUTTON_LABEL in response.text
    assert signup_count(client) == 1


def test_the_mail_client_one_click_post_removes_the_row_and_is_idempotent(client):
    client.post(JOIN_ROUTE_PATH, json={"email": SIGNER_ADDRESS})
    link_path = unsubscribe_path(mint_unsubscribe_token(SIGNER_ADDRESS, SECRET_KEY))
    removed = client.post(link_path, data=ONE_CLICK_FORM_BODY)
    assert removed.status_code == HTTP_200_OK
    assert REMOVED_HEADING in removed.text
    assert signup_count(client) == 0
    again = client.post(link_path, data=ONE_CLICK_FORM_BODY)
    assert again.status_code == HTTP_200_OK
    assert REMOVED_HEADING in again.text


def test_a_mangled_token_is_the_expired_page_on_both_methods(client):
    client.post(JOIN_ROUTE_PATH, json={"email": SIGNER_ADDRESS})
    token = mint_unsubscribe_token(SIGNER_ADDRESS, SECRET_KEY)
    mangled = token[:-1] + ("a" if token[-1] != "a" else "b")
    link_path = unsubscribe_path(mangled)
    for response in (client.get(link_path), client.post(link_path)):
        assert response.status_code == HTTP_404_NOT_FOUND
        assert INVALID_HEADING in response.text
    assert signup_count(client) == 1
