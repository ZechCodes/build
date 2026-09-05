"""The one email an invite produces: verbatim copy, the link carried as the message's
action rather than buried in a paragraph, no unsubscribe anything (this is not a
mailing — it is a door key), and a background task that delivers it after the response."""

from __future__ import annotations

import asyncio

from litestar.background_tasks import BackgroundTask

from buildapp.email_message import (
    LIST_UNSUBSCRIBE_HEADER,
    LIST_UNSUBSCRIBE_POST_HEADER,
)
from buildapp.email_test_support import RecordingEmailBackend
from buildapp.invite_mail import (
    INVITE_ACTION_LABEL,
    INVITE_HEADING,
    INVITE_PARAGRAPHS,
    INVITE_SUBJECT,
    build_invite_email,
    invite_email_task,
)

INVITED = "invitee@example.com"
INVITE_URL = "https://getbuild.ing/invite/inv_a-raw-token"


def test_the_subject_and_copy_are_verbatim():
    assert INVITE_SUBJECT == "Your Build invite"
    assert INVITE_HEADING == "You’re in."
    assert INVITE_PARAGRAPHS == (
        "Build turns an issue into shipped code, on your machine.",
        "This invite is for {email} and works once, for the next 14 days.",
    )


def test_the_message_names_the_address_it_was_sent_to():
    message = build_invite_email(to=INVITED, invite_url=INVITE_URL)
    assert message.to == INVITED
    assert message.subject == INVITE_SUBJECT
    for body in (message.text_body, message.html_body):
        assert INVITE_HEADING in body
        assert INVITE_PARAGRAPHS[1].format(email=INVITED) in body


def test_the_invite_url_is_the_messages_action_in_both_bodies():
    message = build_invite_email(to=INVITED, invite_url=INVITE_URL)
    assert f'href="{INVITE_URL}"' in message.html_body
    assert INVITE_ACTION_LABEL == "ACCEPT INVITE"
    assert INVITE_ACTION_LABEL in message.html_body
    assert message.text_body.splitlines()[-1] == f"{INVITE_ACTION_LABEL}: {INVITE_URL}"


def test_an_invite_carries_no_unsubscribe_because_it_is_not_a_mailing():
    message = build_invite_email(to=INVITED, invite_url=INVITE_URL)
    assert message.headers == {}
    assert LIST_UNSUBSCRIBE_HEADER not in message.headers
    assert LIST_UNSUBSCRIBE_POST_HEADER not in message.headers
    for body in (message.text_body, message.html_body):
        assert "unsubscribe" not in body.lower()


def test_the_task_is_a_background_task_that_delivers_when_awaited():
    email_backend = RecordingEmailBackend()
    task = invite_email_task(email_backend, INVITED, INVITE_URL)
    assert isinstance(task, BackgroundTask)
    assert email_backend.sent == []
    asyncio.run(task())
    assert [sent.to for sent in email_backend.sent] == [INVITED]
    assert email_backend.sent[0].subject == INVITE_SUBJECT
    assert INVITE_URL in email_backend.sent[0].text_body
