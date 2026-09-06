"""Waitlist address normalisation: the one place that decides what the prelaunch landing
page accepts as an address and the exact string stored for it.

Two rules live here, and they answer different questions. ``canonical_address`` is the
casing rule — are these two strings the same address — and applies to any address at
all. ``normalize_waitlist_address`` adds the judgement on top: is this an address we
would send to. An invite is *issued* under the second rule (it is a send) and *matched*
under the first (a dev account at ``@localhost`` still has an identity)."""

from __future__ import annotations

import re

MAX_WAITLIST_ADDRESS_LENGTH = 254
WAITLIST_ADDRESS_PATTERN = re.compile(r"^[a-z0-9!#$%&'*+/=?^_`{|}~.-]+@[^@\s.]+(\.[^@\s.]+)+$")


def canonical_address(raw: str) -> str:
    """The comparable form of an address: trimmed and lowercased, nothing else."""
    return raw.strip().lower()


def normalize_waitlist_address(raw: str) -> str | None:
    normalized = canonical_address(raw)
    if not normalized or len(normalized) > MAX_WAITLIST_ADDRESS_LENGTH:
        return None
    if WAITLIST_ADDRESS_PATTERN.fullmatch(normalized) is None:
        return None
    return normalized
