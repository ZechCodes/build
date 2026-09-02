"""Tests pinning the waitlist email context and the two message builders: verbatim copy,
the List-Unsubscribe headers each message carries, and the base URL / notify address the
context resolves from settings and the process environment."""

from __future__ import annotations

from types import SimpleNamespace

import pytest

from buildapp.email_message import (
    LIST_UNSUBSCRIBE_HEADER,
    LIST_UNSUBSCRIBE_POST_HEADER,
    ONE_CLICK_HEADER_VALUE,
)
from buildapp.waitlist_mail import (
    CONFIRMATION_HEADING,
    CONFIRMATION_PARAGRAPHS,
    CONFIRMATION_SUBJECT,
    NOTIFY_ADDRESS_ENV,
    OWNER_HEADING,
    OWNER_SUBJECT_PREFIX,
    build_confirmation_email,
    build_owner_notification_email,
    resolve_public_base_url,
    resolve_waitlist_email_context,
)

SIGNER = "signer@example.com"
OWNER = "hi@zech.sh"
UNSUBSCRIBE_URL = "https://getbuild.ing/waitlist/unsubscribe/token-value"


def _settings(*, public_base_url: str, redirect_base_url: str, secret_key: str = "secret"):
    return SimpleNamespace(
        email=SimpleNamespace(public_base_url=public_base_url),
        auth=SimpleNamespace(redirect_base_url=redirect_base_url),
        secret_key=secret_key,
    )


# ----- copy -------------------------------------------------------------------


def test_confirmation_subject_and_copy_are_verbatim():
    assert CONFIRMATION_SUBJECT == "You’re on the Build list"
    assert CONFIRMATION_HEADING == "You’re on the list."
    assert CONFIRMATION_PARAGRAPHS == (
        "Build is the agentic coding IDE for teams. Invites go out weekly.",
        "The next email you get from us is your invite. Nothing else.",
    )
    message = build_confirmation_email(to=SIGNER, unsubscribe_url=UNSUBSCRIBE_URL)
    assert message.to == SIGNER
    assert message.subject == CONFIRMATION_SUBJECT
    for body in (message.text_body, message.html_body):
        assert CONFIRMATION_HEADING in body
        for paragraph in CONFIRMATION_PARAGRAPHS:
            assert paragraph in body


def test_confirmation_carries_list_unsubscribe_and_one_click():
    headers = build_confirmation_email(to=SIGNER, unsubscribe_url=UNSUBSCRIBE_URL).headers
    assert headers[LIST_UNSUBSCRIBE_HEADER] == f"<{UNSUBSCRIBE_URL}>"
    assert headers[LIST_UNSUBSCRIBE_POST_HEADER] == ONE_CLICK_HEADER_VALUE


def test_owner_notification_subject_names_the_address():
    message = build_owner_notification_email(
        to=OWNER, signup_email=SIGNER, unsubscribe_url=UNSUBSCRIBE_URL
    )
    assert message.to == OWNER
    assert message.subject == f"{OWNER_SUBJECT_PREFIX}{SIGNER}"


def test_owner_notification_body_names_the_address_once_per_body():
    message = build_owner_notification_email(
        to=OWNER, signup_email=SIGNER, unsubscribe_url=UNSUBSCRIBE_URL
    )
    for body in (message.text_body, message.html_body):
        assert OWNER_HEADING in body
        assert body.count(SIGNER) == 1


def test_owner_notification_carries_list_unsubscribe_but_not_one_click():
    headers = build_owner_notification_email(
        to=OWNER, signup_email=SIGNER, unsubscribe_url=UNSUBSCRIBE_URL
    ).headers
    assert headers[LIST_UNSUBSCRIBE_HEADER] == f"<{UNSUBSCRIBE_URL}>"
    assert LIST_UNSUBSCRIBE_POST_HEADER not in headers


def test_both_messages_embed_the_unsubscribe_url():
    messages = (
        build_confirmation_email(to=SIGNER, unsubscribe_url=UNSUBSCRIBE_URL),
        build_owner_notification_email(
            to=OWNER, signup_email=SIGNER, unsubscribe_url=UNSUBSCRIBE_URL
        ),
    )
    for message in messages:
        assert UNSUBSCRIBE_URL in message.text_body
        assert UNSUBSCRIBE_URL in message.html_body


# ----- context ----------------------------------------------------------------


def test_resolve_public_base_url_prefers_email_then_auth_and_strips_the_slash():
    assert (
        resolve_public_base_url(
            _settings(
                public_base_url="https://getbuild.ing/",
                redirect_base_url="https://elsewhere.example/",
            )
        )
        == "https://getbuild.ing"
    )
    assert (
        resolve_public_base_url(
            _settings(public_base_url="", redirect_base_url="https://elsewhere.example/")
        )
        == "https://elsewhere.example"
    )
    assert resolve_public_base_url(_settings(public_base_url="", redirect_base_url="")) == ""


def test_resolve_context_reads_the_notify_address_from_the_mapping():
    context = resolve_waitlist_email_context(
        _settings(public_base_url="https://getbuild.ing", redirect_base_url=""),
        {NOTIFY_ADDRESS_ENV: f"  {OWNER}  "},
    )
    assert context.public_base_url == "https://getbuild.ing"
    assert context.notify_address == OWNER


def test_resolve_context_yields_empty_notify_address_when_unset():
    context = resolve_waitlist_email_context(
        _settings(public_base_url="https://getbuild.ing", redirect_base_url=""), {}
    )
    assert context.notify_address == ""


def test_resolve_context_fails_fast_without_a_base_url():
    with pytest.raises(ValueError):
        resolve_waitlist_email_context(
            _settings(public_base_url="", redirect_base_url=""), {}
        )


def test_resolve_context_carries_the_secret_key():
    context = resolve_waitlist_email_context(
        _settings(
            public_base_url="https://getbuild.ing",
            redirect_base_url="",
            secret_key="the-signing-key",
        ),
        {},
    )
    assert context.secret_key == "the-signing-key"
