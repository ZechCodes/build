"""The C901 ratchet only goes down (CLAUDE.md "Complexity gates").

``# noqa: C901`` is how a function that was already over the cap when the gate
landed stays in the tree. Prose alone would not hold that line — ruff is happy
either way — so the count is asserted here: adding one fails this test, and
retiring one is a deliberate edit of the number below.
"""

from __future__ import annotations

from pathlib import Path

BUILDAPP = Path(__file__).parent

# Two, both on the transport-report path, both measured when the gate landed:
# transport_controller.TransportController.report (11) and
# transport_report.apply_event (13).
RATCHETED_FUNCTIONS = 2


def test_no_new_c901_suppressions():
    # This file names the marker to look for, so it never counts itself.
    sources = (p for p in sorted(BUILDAPP.rglob("*.py")) if p != Path(__file__))
    found = [
        f"{path.name}:{number}"
        for path in sources
        for number, line in enumerate(path.read_text().splitlines(), start=1)
        if "noqa: C901" in line
    ]
    assert len(found) == RATCHETED_FUNCTIONS, (
        "the C901 ratchet moved — split the function instead of suppressing it, "
        f"or lower RATCHETED_FUNCTIONS when retiring one. Found: {found}"
    )
