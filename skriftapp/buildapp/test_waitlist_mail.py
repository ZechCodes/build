"""Tests pinning the waitlist email context, the two message builders and the background
send: verbatim copy, the List-Unsubscribe headers each message carries, the base URL /
notify address the context resolves, which messages a signup produces, and the fail-soft
delivery that logs a failed send instead of raising out of the background task."""

from __future__ import annotations

import asyncio
import logging
from types import SimpleNamespace

import pytest
from litestar.background_tasks import BackgroundTask

from buildapp.email_message import (
    LIST_UNSUBSCRIBE_HEADER,
    LIST_UNSUBSCRIBE_POST_HEADER,
    ONE_CLICK_HEADER_VALUE,
)
from buildapp.email_test_support import FailingEmailBackend, RecordingEmailBackend
from buildapp.waitlist_mail import (
    CONFIRMATION_HEADING,
    CONFIRMATION_PARAGRAPHS,
    CONFIRMATION_SUBJECT,
    NOTIFY_ADDRESS_ENV,
    OWNER_HEADING,
    OWNER_SUBJECT_PREFIX,
    WaitlistEmailContext,
    build_confirmation_email,
    build_owner_notification_email,
    build_signup_emails,
    deliver_waitlist_emails,
    resolve_public_base_url,
    resolve_waitlist_email_context,
    waitlist_signup_email_task,
)
from buildapp.waitlist_unsubscribe_token import read_unsubscribe_token

SIGNER = "signer@example.com"
OWNER = "hi@zech.sh"
UNSUBSCRIBE_URL = "https://getbuild.ing/waitlist/unsubscribe/token-value"
PUBLIC_BASE_URL = "https://getbuild.ing"
SECRET_KEY = "the-signing-key"


def _context(*, notify_address: str = "") -> WaitlistEmailContext:
    return WaitlistEmailContext(
        public_base_url=PUBLIC_BASE_URL,
        secret_key=SECRET_KEY,
        notify_address=notify_address,
    )


def _unsubscribe_url_of(message) -> str:
    return message.headers[LIST_UNSUBSCRIBE_HEADER].strip("<>")


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


# ----- the messages one signup produces ---------------------------------------


def test_build_signup_emails_returns_only_the_confirmation_without_a_notify_address():
    messages = build_signup_emails(signup_email=SIGNER, context=_context())
    assert len(messages) == 1
    assert messages[0].to == SIGNER
    assert messages[0].subject == CONFIRMATION_SUBJECT


def test_build_signup_emails_adds_the_owner_notification_when_set():
    messages = build_signup_emails(
        signup_email=SIGNER, context=_context(notify_address=OWNER)
    )
    assert [message.to for message in messages] == [SIGNER, OWNER]
    assert messages[1].subject == f"{OWNER_SUBJECT_PREFIX}{SIGNER}"


def test_both_signup_messages_share_one_unsubscribe_url_that_reads_back_to_the_signup_address():
    messages = build_signup_emails(
        signup_email=SIGNER, context=_context(notify_address=OWNER)
    )
    shared_url = _unsubscribe_url_of(messages[0])
    assert _unsubscribe_url_of(messages[1]) == shared_url
    assert shared_url.startswith(f"{PUBLIC_BASE_URL}/waitlist/unsubscribe/")
    token = shared_url.rsplit("/", 1)[-1]
    assert read_unsubscribe_token(token, SECRET_KEY) == SIGNER


# ----- delivery ---------------------------------------------------------------


def test_deliver_sends_every_message_through_the_backend():
    email_backend = RecordingEmailBackend()
    messages = build_signup_emails(
        signup_email=SIGNER, context=_context(notify_address=OWNER)
    )
    asyncio.run(deliver_waitlist_emails(email_backend, messages))
    assert [sent.to for sent in email_backend.sent] == [SIGNER, OWNER]
    assert email_backend.sent[0].subject == CONFIRMATION_SUBJECT
    assert email_backend.sent[1].headers[LIST_UNSUBSCRIBE_HEADER] == (
        f"<{_unsubscribe_url_of(messages[1])}>"
    )


def test_deliver_swallows_a_failed_send_logs_it_and_continues(caplog):
    messages = build_signup_emails(
        signup_email=SIGNER, context=_context(notify_address=OWNER)
    )
    with caplog.at_level(logging.ERROR):
        asyncio.run(deliver_waitlist_emails(FailingEmailBackend(), messages))
    failures = [record for record in caplog.records if record.levelno == logging.ERROR]
    assert len(failures) == len(messages)
    logged = [record.getMessage() for record in failures]
    for recipient in (SIGNER, OWNER):
        assert sum(recipient in text for text in logged) == 1


def test_signup_email_task_is_a_background_task_that_delivers_when_awaited():
    email_backend = RecordingEmailBackend()
    task = waitlist_signup_email_task(
        email_backend=email_backend,
        signup_email=SIGNER,
        context=_context(notify_address=OWNER),
    )
    assert isinstance(task, BackgroundTask)
    assert email_backend.sent == []
    asyncio.run(task())
    assert [sent.to for sent in email_backend.sent] == [SIGNER, OWNER]
