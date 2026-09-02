"""Tests pinning the message value and the one sender: List-Unsubscribe headers, a
composed message that carries every rendered field, and a send that passes exactly those
fields to the backend and never swallows a delivery failure."""

from __future__ import annotations

import asyncio

import pytest

from buildapp.email_message import (
    LIST_UNSUBSCRIBE_HEADER,
    LIST_UNSUBSCRIBE_POST_HEADER,
    ONE_CLICK_HEADER_VALUE,
    compose_email,
    list_unsubscribe_headers,
    send_email_message,
)
from buildapp.email_test_support import (
    EmailDeliveryFailure,
    FailingEmailBackend,
    RecordingEmailBackend,
    SentEmail,
)

UNSUBSCRIBE_URL = "https://getbuild.ing/waitlist/unsubscribe/token-value"
HEADING = "You’re on the list."
PARAGRAPHS = ("First paragraph.", "Second paragraph.")


def _message():
    return compose_email(
        to="signer@example.com",
        subject="You’re on the Build list",
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=UNSUBSCRIBE_URL,
        one_click=True,
    )


def test_list_unsubscribe_header_is_angle_bracketed():
    headers = list_unsubscribe_headers(UNSUBSCRIBE_URL, one_click=False)
    assert headers[LIST_UNSUBSCRIBE_HEADER] == f"<{UNSUBSCRIBE_URL}>"


def test_one_click_header_is_added_only_when_requested():
    without_one_click = list_unsubscribe_headers(UNSUBSCRIBE_URL, one_click=False)
    with_one_click = list_unsubscribe_headers(UNSUBSCRIBE_URL, one_click=True)
    assert LIST_UNSUBSCRIBE_POST_HEADER not in without_one_click
    assert with_one_click[LIST_UNSUBSCRIBE_POST_HEADER] == ONE_CLICK_HEADER_VALUE


def test_compose_email_fills_every_field():
    message = _message()
    assert message.to == "signer@example.com"
    assert message.subject == "You’re on the Build list"
    for body in (message.text_body, message.html_body):
        assert HEADING in body
        for paragraph in PARAGRAPHS:
            assert paragraph in body
        assert UNSUBSCRIBE_URL in body
    assert message.headers == list_unsubscribe_headers(UNSUBSCRIBE_URL, one_click=True)


def test_send_passes_exactly_the_message_fields_to_the_backend():
    message = _message()
    backend = RecordingEmailBackend()
    asyncio.run(send_email_message(backend, message))
    assert backend.sent == [
        SentEmail(
            to=message.to,
            subject=message.subject,
            text_body=message.text_body,
            html_body=message.html_body,
            headers=message.headers,
        )
    ]


def test_send_propagates_backend_failure():
    with pytest.raises(EmailDeliveryFailure):
        asyncio.run(send_email_message(FailingEmailBackend(), _message()))
