"""What time it is, in one place.

Every row this app stamps — an invite issued, a device seen, a subscription made, a
transport session reported — is stamped from here, so "now" is always timezone-aware
UTC and a test that needs to move time has one function to patch."""

from __future__ import annotations

from datetime import datetime, timezone


def utc_now() -> datetime:
    return datetime.now(tz=timezone.utc)
