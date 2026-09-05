"""The one hashing rule for every secret this app stores rather than keeps: gateway
tokens and invite tokens alike are held as the SHA-256 hex of the raw string, so a
database read never yields a usable token and a lookup is an equality test."""

from __future__ import annotations

import hashlib


def token_hash(raw: str) -> str:
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()
