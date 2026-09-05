"""The three pages the unsubscribe link leads to — confirm, removed, and expired — each
one panel of copy rendered through the landing shell. This module owns the copy; every
tag and class name it needs comes from a fragment in the landing directory, and the
address and the token are the only runtime values, both escaped."""

from __future__ import annotations

from html import escape

from buildapp.landing_page import (
    HOME_LINK,
    fill_slots,
    read_landing_file,
    render_panel_page,
)
from buildapp.waitlist_unsubscribe_token import unsubscribe_path

ADDRESS_FRAGMENT_NAME = "unsubscribe-address.html"
FORM_FRAGMENT_NAME = "unsubscribe-form.html"

CONFIRM_TITLE = "Unsubscribe — Build"
CONFIRM_HEADING_TEMPLATE = "Remove {address} from the list?"
CONFIRM_MESSAGE = "You’ll stop getting Build email. You can join again any time."
CONFIRM_BUTTON_LABEL = "REMOVE ME"

REMOVED_TITLE = "Unsubscribed — Build"
REMOVED_HEADING = "You’re off the list."
REMOVED_MESSAGE_TEMPLATE = (
    "Nothing else will arrive. Changed your mind? Join again from the {home_link}."
)

INVALID_TITLE = "Link expired — Build"
INVALID_HEADING = "This link is no longer valid."
INVALID_MESSAGE = (
    "Links expire. Use the unsubscribe link in the most recent email you have from us."
)

NO_ACTION = ""


def render_address(email: str) -> str:
    return fill_slots(read_landing_file(ADDRESS_FRAGMENT_NAME), {"email": escape(email)})


def render_remove_form(token: str) -> str:
    return fill_slots(
        read_landing_file(FORM_FRAGMENT_NAME),
        {
            "action_path": escape(unsubscribe_path(token)),
            "button_label": CONFIRM_BUTTON_LABEL,
        },
    )


def render_home_link() -> str:
    """The shared link renderer, so the anchor in this page's copy is the same anchor
    every other panel page renders."""
    return HOME_LINK.render()


def render_confirm_page(email: str, token: str) -> str:
    return render_panel_page(
        title=CONFIRM_TITLE,
        heading=CONFIRM_HEADING_TEMPLATE.format(address=render_address(email)),
        message=CONFIRM_MESSAGE,
        action=render_remove_form(token),
    )


def render_removed_page() -> str:
    return render_panel_page(
        title=REMOVED_TITLE,
        heading=REMOVED_HEADING,
        message=REMOVED_MESSAGE_TEMPLATE.format(home_link=render_home_link()),
        action=NO_ACTION,
    )


def render_invalid_page() -> str:
    return render_panel_page(
        title=INVALID_TITLE,
        heading=INVALID_HEADING,
        message=INVALID_MESSAGE,
        action=NO_ACTION,
    )
