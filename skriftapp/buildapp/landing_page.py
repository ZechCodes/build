"""The shell every public HTML page renders through: one head, one stylesheet link, one
footer bar, and the single `{{slot}}` substitution primitive that fills them. Skrift's
Jinja engine only covers framework and theme template directories, so these pages stay
engine-free and CSP-clean — plain files with named slots."""

from __future__ import annotations

from pathlib import Path

LANDING_DIR = Path(__file__).parent / "landing"
SHELL_NAME = "shell.html"


def read_landing_file(name: str) -> str:
    return (LANDING_DIR / name).read_text()


def fill_slots(template: str, slots: dict[str, str]) -> str:
    filled = template
    for slot_name, value in slots.items():
        filled = filled.replace(f"{{{{{slot_name}}}}}", value)
    return filled


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
