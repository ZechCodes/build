"""Tests pinning the message value and the one sender: List-Unsubscribe headers, a
composed message that carries every rendered field, a message composed without an
unsubscribe link, and a send that passes exactly those fields to the backend and never
swallows a delivery failure."""

from __future__ import annotations

import asyncio

import pytest
from skrift.lib.email_backends import EmailBackend

from buildapp.email_message import (
    LIST_UNSUBSCRIBE_HEADER,
    LIST_UNSUBSCRIBE_POST_HEADER,
    ONE_CLICK_HEADER_VALUE,
    compose_email,
    list_unsubscribe_headers,
    send_email_message,
)
from buildapp.email_test_support import (
    PUBLIC_BASE_URL,
    UNSUBSCRIBE_URL,
    EmailDeliveryFailure,
    FailingEmailBackend,
    RecordingEmailBackend,
    SentEmail,
)

RECIPIENT = "recipient@example.com"
SUBJECT = "A subject"
HEADING = "A heading."
PARAGRAPHS = ("First paragraph.", "Second paragraph.")


def compose_test_message(unsubscribe_url: str | None = UNSUBSCRIBE_URL, *, one_click=True):
    return compose_email(
        to=RECIPIENT,
        subject=SUBJECT,
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=unsubscribe_url,
        one_click=one_click,
        public_base_url=PUBLIC_BASE_URL,
    )


def test_the_test_backends_satisfy_the_framework_email_backend_protocol():
    assert isinstance(RecordingEmailBackend(), EmailBackend)
    assert isinstance(FailingEmailBackend(), EmailBackend)


def test_list_unsubscribe_header_is_angle_bracketed():
    headers = list_unsubscribe_headers(UNSUBSCRIBE_URL, one_click=False)
    assert headers[LIST_UNSUBSCRIBE_HEADER] == f"<{UNSUBSCRIBE_URL}>"


def test_one_click_header_is_added_only_when_requested():
    without_one_click = list_unsubscribe_headers(UNSUBSCRIBE_URL, one_click=False)
    with_one_click = list_unsubscribe_headers(UNSUBSCRIBE_URL, one_click=True)
    assert LIST_UNSUBSCRIBE_POST_HEADER not in without_one_click
    assert with_one_click[LIST_UNSUBSCRIBE_POST_HEADER] == ONE_CLICK_HEADER_VALUE


def test_no_unsubscribe_url_means_no_unsubscribe_header():
    assert list_unsubscribe_headers(None, one_click=False) == {}
    assert list_unsubscribe_headers(None, one_click=True) == {}


def test_compose_email_fills_every_field():
    message = compose_test_message()
    assert message.to == RECIPIENT
    assert message.subject == SUBJECT
    for body in (message.text_body, message.html_body):
        assert HEADING in body
        for paragraph in PARAGRAPHS:
            assert paragraph in body
        assert UNSUBSCRIBE_URL in body
    assert message.headers == list_unsubscribe_headers(UNSUBSCRIBE_URL, one_click=True)


def test_compose_email_links_the_mark_from_this_deployment():
    assert f'src="{PUBLIC_BASE_URL}/landing/email/brand-mark@2x.png"' in (
        compose_test_message().html_body
    )


def test_compose_email_without_an_unsubscribe_url_carries_no_link_and_no_header():
    message = compose_test_message(None, one_click=False)
    assert message.headers == {}
    for body in (message.text_body, message.html_body):
        assert UNSUBSCRIBE_URL not in body
        assert HEADING in body


def test_send_passes_exactly_the_message_fields_to_the_backend():
    message = compose_test_message()
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
        asyncio.run(send_email_message(FailingEmailBackend(), compose_test_message()))
