"""The three pages the unsubscribe link leads to — confirm, removed, and expired — each
one panel of copy rendered through the landing shell so it carries the site's head,
stylesheet and footer. The copy constants that carry markup are this module's own
strings; the address and the token are the only runtime values, and both are escaped."""

from __future__ import annotations

from html import escape

from buildapp.landing_page import fill_slots, read_landing_file, render_shell
from buildapp.waitlist_unsubscribe_token import unsubscribe_path

PANEL_NAME = "unsubscribe.html"

CONFIRM_TITLE = "Unsubscribe — Build"
CONFIRM_HEADING_TEMPLATE = 'Remove <span class="title-accent unsubscribe-address">{email}</span> from the list?'
CONFIRM_MESSAGE = "You’ll stop getting Build email. You can join again any time."
CONFIRM_BUTTON_LABEL = "REMOVE ME"

REMOVED_TITLE = "Unsubscribed — Build"
REMOVED_HEADING = "You’re off the list."
REMOVED_MESSAGE = (
    "Nothing else will arrive. Changed your mind? Join again from the "
    '<a href="/">home page</a>.'
)

INVALID_TITLE = "Link expired — Build"
INVALID_HEADING = "This link is no longer valid."
INVALID_MESSAGE = (
    "Links expire. Use the unsubscribe link in the most recent email you have from us."
)

NO_ACTION = ""
NO_DESCRIPTION = ""
NO_SCRIPTS = ""


def render_unsubscribe_page(
    *, title: str, heading: str, message: str, action: str
) -> str:
    return render_shell(
        title=title,
        description=NO_DESCRIPTION,
        body=fill_slots(
            read_landing_file(PANEL_NAME),
            {"heading": heading, "message": message, "action": action},
        ),
        scripts=NO_SCRIPTS,
    )


def render_confirm_page(email: str, token: str) -> str:
    action = (
        f'<form method="post" action="{escape(unsubscribe_path(token))}">'
        f'<button class="button-primary" type="submit">{CONFIRM_BUTTON_LABEL}</button>'
        "</form>"
    )
    return render_unsubscribe_page(
        title=CONFIRM_TITLE,
        heading=CONFIRM_HEADING_TEMPLATE.format(email=escape(email)),
        message=CONFIRM_MESSAGE,
        action=action,
    )


def render_removed_page() -> str:
    return render_unsubscribe_page(
        title=REMOVED_TITLE,
        heading=REMOVED_HEADING,
        message=REMOVED_MESSAGE,
        action=NO_ACTION,
    )


def render_invalid_page() -> str:
    return render_unsubscribe_page(
        title=INVALID_TITLE,
        heading=INVALID_HEADING,
        message=INVALID_MESSAGE,
        action=NO_ACTION,
    )
