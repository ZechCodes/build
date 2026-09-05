"""The shell every public HTML page renders through: one head, one stylesheet link, one
footer bar, and the single `{{slot}}` substitution primitive that fills them. Skrift's
Jinja engine only covers framework and theme template directories, so these pages stay
engine-free and CSP-clean — plain files with named slots.

Also the one link renderer every panel page uses, in either style — because a link on
the unsubscribe page and a link on an invite page are the same anchor, and were two."""

from __future__ import annotations

import re
from dataclasses import dataclass
from html import escape
from pathlib import Path

LANDING_DIR = Path(__file__).parent / "landing"
SHELL_NAME = "shell.html"
#: The one-panel body every short public page uses — unsubscribe, and every invite
#: outcome. Heading, one line of copy, and one action slot.
PANEL_NAME = "panel.html"
#: The two link styles: a plain anchor, and the landing's primary button.
LINK_FRAGMENT_NAME = "panel-link.html"
BUTTON_FRAGMENT_NAME = "panel-button.html"
NO_DESCRIPTION = ""
NO_SCRIPTS = ""

HOME_PATH = "/"
HOME_LINK_LABEL = "home page"


def read_landing_file(name: str) -> str:
    return (LANDING_DIR / name).read_text()


SLOT_PLACEHOLDER_PATTERN = re.compile(r"\{\{(\w+)\}\}")


def slot_placeholder(slot_name: str) -> str:
    return f"{{{{{slot_name}}}}}"


def fill_slots(template: str, slots: dict[str, str]) -> str:
    return SLOT_PLACEHOLDER_PATTERN.sub(
        lambda match: slots.get(match.group(1), match.group(0)), template
    )


@dataclass(frozen=True)
class Link:
    """One link on a panel page. ``primary`` picks the button style; both styles are
    fragments in the landing directory, so the markup stays out of Python."""

    label: str
    href: str
    primary: bool = False

    def render(self) -> str:
        fragment = BUTTON_FRAGMENT_NAME if self.primary else LINK_FRAGMENT_NAME
        return fill_slots(
            read_landing_file(fragment),
            {"href": escape(self.href), "label": escape(self.label)},
        )


#: Back to the front page. One value, because both the unsubscribe pages and the invite
#: pages offer it.
HOME_LINK = Link(label=HOME_LINK_LABEL, href=HOME_PATH)


def render_shell(*, title: str, description: str, body: str, scripts: str) -> str:
    return fill_slots(
        read_landing_file(SHELL_NAME),
        {
            "title": title,
            "description": description,
            "body": body,
            "scripts": scripts,
        },
    )


def render_panel_page(*, title: str, heading: str, message: str, action: str) -> str:
    """One panel of copy in the landing shell — no description, no scripts. Every
    runtime value in ``heading``, ``message`` and ``action`` is already escaped by its
    caller, which owns the copy."""
    return render_shell(
        title=title,
        description=NO_DESCRIPTION,
        body=fill_slots(
            read_landing_file(PANEL_NAME),
            {"heading": heading, "message": message, "action": action},
        ),
        scripts=NO_SCRIPTS,
    )
