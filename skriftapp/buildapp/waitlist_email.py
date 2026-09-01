"""Waitlist email normalisation: the one place that decides what the prelaunch landing
page accepts as an address and the exact string stored for it."""

from __future__ import annotations

import re

MAX_WAITLIST_EMAIL_LENGTH = 254
WAITLIST_EMAIL_PATTERN = re.compile(r"^[^@\s]+@[^@\s.]+(\.[^@\s.]+)+$")


def normalize_waitlist_email(raw: str) -> str | None:
    normalized = raw.strip().lower()
    if not normalized or len(normalized) > MAX_WAITLIST_EMAIL_LENGTH:
        return None
    if WAITLIST_EMAIL_PATTERN.fullmatch(normalized) is None:
        return None
    return normalized
