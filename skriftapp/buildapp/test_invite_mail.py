"""The one email an invite produces: verbatim copy, the link carried as the message's
action rather than buried in a paragraph, no unsubscribe anything (this is not a
mailing — it is a door key), and one sender both routes reach — inline for the operator
watching the admin page, deferred for the JSON route's script."""

from __future__ import annotations

import asyncio
from html import escape

from litestar.background_tasks import BackgroundTask

from buildapp.email_message import (
    LIST_UNSUBSCRIBE_HEADER,
    LIST_UNSUBSCRIBE_POST_HEADER,
)
from buildapp.email_template import PHONE_TABLET_IMAGE
from buildapp.email_test_support import (
    PUBLIC_BASE_URL,
    FailingEmailBackend,
    RecordingEmailBackend,
)
from buildapp.invite_mail import (
    GETTING_STARTED,
    INVITE_ACTION_LABEL,
    INVITE_CLOSING,
    INVITE_HEADING,
    INVITE_LINK_NOTE,
    INVITE_PARAGRAPHS,
    INVITE_SUBJECT,
    build_invite_email,
    invite_email_task,
    send_invite_email,
)

INVITED = "invitee@example.com"
INVITE_URL = "https://getbuild.ing/invite/inv_a-raw-token"
HERO_URL = f"{PUBLIC_BASE_URL}/landing/email/{PHONE_TABLET_IMAGE.file}"


def invite_email():
    return build_invite_email(
        to=INVITED, invite_url=INVITE_URL, public_base_url=PUBLIC_BASE_URL
    )


INSTALL_COMMAND = 'curl -fsSL "https://getbuild.ing/install.sh" | sh'
STEP_TEXTS = (
    "Install the bridge on the machine where your code lives:",
    "In Build, enter the pairing code the bridge prints.",
    "Check that the fingerprint matches the one the bridge printed, then approve.",
)
DESKTOP_DETAIL = (
    "There’s also a desktop app. It’s optional, and its installer is at "
    "https://getbuild.ing/docs."
)


def test_the_subject_and_copy_are_verbatim():
    assert INVITE_SUBJECT == "You’re in: your Build invite"
    assert INVITE_HEADING == "You’re in."
    assert INVITE_PARAGRAPHS == (
        "Thanks for waiting. Your spot in the Build alpha is ready.",
    )
    assert INVITE_ACTION_LABEL == "Create your account"
    assert INVITE_LINK_NOTE == (
        "This link is just for you ({email}). It works once, for the next 14 days."
    )
    assert GETTING_STARTED.title == "Getting started"
    assert tuple(step.text for step in GETTING_STARTED.steps) == STEP_TEXTS
    assert GETTING_STARTED.steps[0].command == INSTALL_COMMAND
    assert GETTING_STARTED.steps[0].detail == DESKTOP_DETAIL
    assert INVITE_CLOSING == (
        "That’s it. Your agents run on your machine from there.",
        "Stuck, or something broke? Reply to this email. It comes straight to me.",
        "— Zech",
    )


def test_the_text_part_reads_top_to_bottom_as_the_copy_on_the_issue():
    """A snapshot of the whole plain-text part: what an invitee with images and HTML
    off reads, and what Zech signed off on."""
    message = invite_email()
    assert message.text_body == "\n\n".join(
        (
            "Build",
            "You’re in.",
            "Thanks for waiting. Your spot in the Build alpha is ready.",
            f"Create your account: {INVITE_URL}",
            f"This link is just for you ({INVITED}). It works once, for the next 14 days.",
            "Getting started",
            f"1. {STEP_TEXTS[0]}",
            f"   {INSTALL_COMMAND}",
            f"   {DESKTOP_DETAIL}",
            f"2. {STEP_TEXTS[1]}",
            f"3. {STEP_TEXTS[2]}",
            *INVITE_CLOSING,
        )
    )


def test_the_html_part_carries_the_button_then_the_steps_in_order():
    html = invite_email().html_body
    assert f'href="{INVITE_URL}"' in html
    assert f">{INVITE_ACTION_LABEL}</a>" in html
    escaped_command = INSTALL_COMMAND.replace('"', "&quot;")
    positions = [
        html.index(INVITE_HEADING),
        html.index(INVITE_URL),
        html.index(f"just for you ({INVITED})"),
        html.index(GETTING_STARTED.title),
        html.index(escape(STEP_TEXTS[0])),
        html.index(f"<code>{escaped_command}</code>"),
        html.index(escape(DESKTOP_DETAIL)),
        html.index(escape(STEP_TEXTS[1])),
        html.index(escape(STEP_TEXTS[2])),
        html.index(escape(INVITE_CLOSING[1])),
    ]
    assert positions == sorted(positions)
    for number in (1, 2, 3):
        assert f">{number}.</td>" in html


def test_the_devices_sit_between_the_intro_and_the_button():
    html = invite_email().html_body
    assert f'alt="{PHONE_TABLET_IMAGE.alt}"' in html
    positions = [
        html.index(INVITE_HEADING),
        html.index(escape(INVITE_PARAGRAPHS[0])),
        html.index(HERO_URL),
        html.index(f'href="{INVITE_URL}"'),
    ]
    assert positions == sorted(positions)


def test_the_message_names_the_address_it_was_sent_to():
    message = invite_email()
    assert message.to == INVITED
    assert message.subject == INVITE_SUBJECT
    for body in (message.text_body, message.html_body):
        assert INVITE_HEADING in body
        assert INVITE_LINK_NOTE.format(email=INVITED) in body


def test_the_invite_url_is_the_messages_action_in_both_bodies():
    message = invite_email()
    assert f'href="{INVITE_URL}"' in message.html_body
    assert f"{INVITE_ACTION_LABEL}: {INVITE_URL}" in message.text_body.splitlines()


def test_an_invite_carries_no_unsubscribe_because_it_is_not_a_mailing():
    message = invite_email()
    assert message.headers == {}
    assert LIST_UNSUBSCRIBE_HEADER not in message.headers
    assert LIST_UNSUBSCRIBE_POST_HEADER not in message.headers
    for body in (message.text_body, message.html_body):
        assert "unsubscribe" not in body.lower()


def test_sending_delivers_the_invite_through_the_backend_it_was_handed():
    email_backend = RecordingEmailBackend()
    sent = send_invite_email(email_backend, INVITED, INVITE_URL, PUBLIC_BASE_URL)
    assert asyncio.run(sent) is True
    assert [sent.to for sent in email_backend.sent] == [INVITED]
    assert email_backend.sent[0].subject == INVITE_SUBJECT
    assert INVITE_URL in email_backend.sent[0].text_body


def test_a_send_that_fails_is_swallowed_and_reported_so_the_operator_can_resend():
    sent = send_invite_email(FailingEmailBackend(), INVITED, INVITE_URL, PUBLIC_BASE_URL)
    assert asyncio.run(sent) is False


def test_the_task_is_the_same_send_deferred_until_after_the_response():
    email_backend = RecordingEmailBackend()
    task = invite_email_task(email_backend, INVITED, INVITE_URL, PUBLIC_BASE_URL)
    assert isinstance(task, BackgroundTask)
    assert task.fn is send_invite_email
    assert email_backend.sent == []
    asyncio.run(task())
    assert [sent.to for sent in email_backend.sent] == [INVITED]
