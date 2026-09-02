"""Waitlist address normalisation: the one place that decides what the prelaunch landing
page accepts as an address and the exact string stored for it."""

from __future__ import annotations

import re

MAX_WAITLIST_ADDRESS_LENGTH = 254
WAITLIST_ADDRESS_PATTERN = re.compile(r"^[^@\s]+@[^@\s.]+(\.[^@\s.]+)+$")


def normalize_waitlist_address(raw: str) -> str | None:
    normalized = raw.strip().lower()
    if not normalized or len(normalized) > MAX_WAITLIST_ADDRESS_LENGTH:
        return None
    if WAITLIST_ADDRESS_PATTERN.fullmatch(normalized) is None:
        return None
    return normalized
