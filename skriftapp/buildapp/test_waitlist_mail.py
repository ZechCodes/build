"""Tests pinning the waitlist email context, the two message builders and the background
send: verbatim copy, the List-Unsubscribe header the confirmation carries and the owner
notification deliberately does not, the base URL / notify address the context resolves,
which messages a signup produces, and the fail-soft delivery that logs a failed send
instead of raising out of the background task."""

from __future__ import annotations

import asyncio
import logging

import pytest
from litestar.background_tasks import BackgroundTask

from buildapp.email_message import (
    LIST_UNSUBSCRIBE_HEADER,
    LIST_UNSUBSCRIBE_POST_HEADER,
    ONE_CLICK_HEADER_VALUE,
    deliver_emails,
    resolve_public_base_url,
)
from buildapp.email_test_support import (
    OWNER_ADDRESS,
    PUBLIC_BASE_URL,
    SECRET_KEY,
    UNSUBSCRIBE_URL,
    FailingEmailBackend,
    RecordingEmailBackend,
    email_settings,
    waitlist_email_context,
)
from buildapp.waitlist_mail import (
    CONFIRMATION_HEADING,
    CONFIRMATION_PARAGRAPHS,
    CONFIRMATION_SUBJECT,
    NOTIFY_ADDRESS_ENV,
    OWNER_HEADING,
    OWNER_PARAGRAPH_TEMPLATE,
    OWNER_SUBJECT_PREFIX,
    build_confirmation_email,
    build_owner_notification_email,
    build_signup_emails,
    resolve_waitlist_email_context,
    waitlist_signup_email_task,
)
from buildapp.waitlist_unsubscribe_token import (
    UNSUBSCRIBE_PATH_PREFIX,
    read_unsubscribe_token,
)

SIGNER = "signer@example.com"


def unsubscribe_url_of(message) -> str:
    return message.headers[LIST_UNSUBSCRIBE_HEADER].strip("<>")


def test_confirmation_subject_and_copy_are_verbatim():
    assert CONFIRMATION_SUBJECT == "You’re on the Build list"
    assert CONFIRMATION_HEADING == "You’re on the list."
    assert CONFIRMATION_PARAGRAPHS == (
        "Build turns an issue into shipped code, on your machine. Invites go out as seats open.",
        "The next email you get from us is your invite. Nothing else.",
    )
    message = build_confirmation_email(
        to=SIGNER, unsubscribe_url=UNSUBSCRIBE_URL, public_base_url=PUBLIC_BASE_URL
    )
    assert message.to == SIGNER
    assert message.subject == CONFIRMATION_SUBJECT
    for body in (message.text_body, message.html_body):
        assert CONFIRMATION_HEADING in body
        for paragraph in CONFIRMATION_PARAGRAPHS:
            assert paragraph in body


def test_confirmation_carries_list_unsubscribe_and_one_click():
    headers = build_confirmation_email(
        to=SIGNER, unsubscribe_url=UNSUBSCRIBE_URL, public_base_url=PUBLIC_BASE_URL
    ).headers
    assert headers[LIST_UNSUBSCRIBE_HEADER] == f"<{UNSUBSCRIBE_URL}>"
    assert headers[LIST_UNSUBSCRIBE_POST_HEADER] == ONE_CLICK_HEADER_VALUE


def test_owner_notification_subject_and_copy_are_verbatim():
    assert OWNER_HEADING == "New waitlist signup"
    assert OWNER_PARAGRAPH_TEMPLATE == "{signup_email} joined the Build waitlist."
    message = build_owner_notification_email(
        to=OWNER_ADDRESS, signup_email=SIGNER, public_base_url=PUBLIC_BASE_URL
    )
    assert message.to == OWNER_ADDRESS
    assert message.subject == f"{OWNER_SUBJECT_PREFIX}{SIGNER}"
    for body in (message.text_body, message.html_body):
        assert OWNER_HEADING in body
        assert OWNER_PARAGRAPH_TEMPLATE.format(signup_email=SIGNER) in body
        assert body.count(SIGNER) == 1


def test_owner_notification_carries_no_unsubscribe_at_all():
    message = build_owner_notification_email(
        to=OWNER_ADDRESS, signup_email=SIGNER, public_base_url=PUBLIC_BASE_URL
    )
    assert message.headers == {}
    for body in (message.text_body, message.html_body):
        assert UNSUBSCRIBE_PATH_PREFIX not in body


def test_only_the_confirmation_carries_the_signers_removal_token():
    messages = build_signup_emails(
        signup_email=SIGNER, context=waitlist_email_context(notify_address=OWNER_ADDRESS)
    )
    confirmation_url = unsubscribe_url_of(messages[0])
    token = confirmation_url.removeprefix(f"{PUBLIC_BASE_URL}{UNSUBSCRIBE_PATH_PREFIX}")
    assert confirmation_url.startswith(f"{PUBLIC_BASE_URL}{UNSUBSCRIBE_PATH_PREFIX}")
    assert read_unsubscribe_token(token, SECRET_KEY) == SIGNER
    owner_notification = messages[1]
    assert owner_notification.headers == {}
    for body in (owner_notification.text_body, owner_notification.html_body):
        assert token not in body
        assert confirmation_url not in body


def test_resolve_public_base_url_prefers_email_then_auth_and_strips_the_slash():
    assert (
        resolve_public_base_url(
            email_settings(
                public_base_url="https://getbuild.ing/",
                redirect_base_url="https://elsewhere.example/",
            )
        )
        == PUBLIC_BASE_URL
    )
    assert (
        resolve_public_base_url(
            email_settings(
                public_base_url="", redirect_base_url="https://elsewhere.example/"
            )
        )
        == "https://elsewhere.example"
    )
    assert (
        resolve_public_base_url(email_settings(public_base_url="", redirect_base_url=""))
        == ""
    )


def test_resolve_context_reads_the_notify_address_from_the_mapping():
    context = resolve_waitlist_email_context(
        email_settings(public_base_url=PUBLIC_BASE_URL, redirect_base_url=""),
        {NOTIFY_ADDRESS_ENV: f"  {OWNER_ADDRESS}  "},
    )
    assert context.public_base_url == PUBLIC_BASE_URL
    assert context.notify_address == OWNER_ADDRESS


def test_resolve_context_yields_empty_notify_address_when_unset():
    context = resolve_waitlist_email_context(
        email_settings(public_base_url=PUBLIC_BASE_URL, redirect_base_url=""), {}
    )
    assert context.notify_address == ""


def test_resolve_context_fails_fast_without_a_base_url():
    with pytest.raises(ValueError):
        resolve_waitlist_email_context(
            email_settings(public_base_url="", redirect_base_url=""), {}
        )


def test_resolve_context_carries_the_secret_key():
    context = resolve_waitlist_email_context(
        email_settings(
            public_base_url=PUBLIC_BASE_URL,
            redirect_base_url="",
            secret_key=SECRET_KEY,
        ),
        {},
    )
    assert context.secret_key == SECRET_KEY


def test_build_signup_emails_returns_only_the_confirmation_without_a_notify_address():
    messages = build_signup_emails(
        signup_email=SIGNER, context=waitlist_email_context()
    )
    assert len(messages) == 1
    assert messages[0].to == SIGNER
    assert messages[0].subject == CONFIRMATION_SUBJECT


def test_build_signup_emails_adds_the_owner_notification_when_set():
    messages = build_signup_emails(
        signup_email=SIGNER, context=waitlist_email_context(notify_address=OWNER_ADDRESS)
    )
    assert [message.to for message in messages] == [SIGNER, OWNER_ADDRESS]
    assert messages[1].subject == f"{OWNER_SUBJECT_PREFIX}{SIGNER}"


def test_deliver_sends_every_message_through_the_backend():
    email_backend = RecordingEmailBackend()
    messages = build_signup_emails(
        signup_email=SIGNER, context=waitlist_email_context(notify_address=OWNER_ADDRESS)
    )
    assert asyncio.run(deliver_emails(email_backend, messages)) is True
    assert [sent.to for sent in email_backend.sent] == [SIGNER, OWNER_ADDRESS]
    assert email_backend.sent[0].subject == CONFIRMATION_SUBJECT
    assert email_backend.sent[0].headers[LIST_UNSUBSCRIBE_HEADER] == (
        f"<{unsubscribe_url_of(messages[0])}>"
    )


def test_deliver_swallows_a_failed_send_logs_it_and_continues(caplog):
    messages = build_signup_emails(
        signup_email=SIGNER, context=waitlist_email_context(notify_address=OWNER_ADDRESS)
    )
    with caplog.at_level(logging.ERROR):
        assert asyncio.run(deliver_emails(FailingEmailBackend(), messages)) is False
    failures = [record for record in caplog.records if record.levelno == logging.ERROR]
    assert len(failures) == len(messages)
    logged = [record.getMessage() for record in failures]
    for recipient in (SIGNER, OWNER_ADDRESS):
        assert sum(recipient in text for text in logged) == 1


def test_signup_email_task_is_a_background_task_that_delivers_when_awaited():
    email_backend = RecordingEmailBackend()
    task = waitlist_signup_email_task(
        email_backend=email_backend,
        signup_email=SIGNER,
        context=waitlist_email_context(notify_address=OWNER_ADDRESS),
    )
    assert isinstance(task, BackgroundTask)
    assert email_backend.sent == []
    asyncio.run(task())
    assert [sent.to for sent in email_backend.sent] == [SIGNER, OWNER_ADDRESS]
