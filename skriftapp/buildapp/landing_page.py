"""The shell every public HTML page renders through: one head, one stylesheet link, one
footer bar, and the single `{{slot}}` substitution primitive that fills them. Skrift's
Jinja engine only covers framework and theme template directories, so these pages stay
engine-free and CSP-clean — plain files with named slots."""

from __future__ import annotations

import re
from pathlib import Path

LANDING_DIR = Path(__file__).parent / "landing"
SHELL_NAME = "shell.html"
#: The one-panel body every short public page uses — unsubscribe, and every invite
#: outcome. Heading, one line of copy, and one action slot.
PANEL_NAME = "panel.html"
NO_DESCRIPTION = ""
NO_SCRIPTS = ""


def read_landing_file(name: str) -> str:
    return (LANDING_DIR / name).read_text()


SLOT_PLACEHOLDER_PATTERN = re.compile(r"\{\{(\w+)\}\}")


def slot_placeholder(slot_name: str) -> str:
    return f"{{{{{slot_name}}}}}"


def fill_slots(template: str, slots: dict[str, str]) -> str:
    return SLOT_PLACEHOLDER_PATTERN.sub(
        lambda match: slots.get(match.group(1), match.group(0)), template
    )


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
