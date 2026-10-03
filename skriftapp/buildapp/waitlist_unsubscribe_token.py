"""The signed token that authorises a waitlist removal, minted into every email and read
back by the unsubscribe routes; it carries the normalised address so removal needs no
column and no session. It lasts five years because an unsubscribe link must still work
whenever someone digs the email out, and it is safe to hand out for that long: it grants
nothing beyond removing that address from the waitlist and withdrawing its account
product-email consent. Repeated use has no additional effect."""

from __future__ import annotations

from skrift.auth.tokens import create_signed_token, verify_signed_token

from buildapp.waitlist_address import normalize_waitlist_address

UNSUBSCRIBE_TOKEN_PURPOSE = "waitlist_unsubscribe"
UNSUBSCRIBE_TOKEN_TTL_SECONDS = 5 * 365 * 24 * 60 * 60
UNSUBSCRIBE_PATH_PREFIX = "/waitlist/unsubscribe/"


def mint_unsubscribe_token(email: str, secret_key: str) -> str:
    return create_signed_token(
        {"purpose": UNSUBSCRIBE_TOKEN_PURPOSE, "email": email},
        secret_key,
        UNSUBSCRIBE_TOKEN_TTL_SECONDS,
    )


def read_unsubscribe_token(token: str, secret_key: str) -> str | None:
    payload = verify_signed_token(token, secret_key)
    if payload is None:
        return None
    if payload.get("purpose") != UNSUBSCRIBE_TOKEN_PURPOSE:
        return None
    email = payload.get("email")
    if not isinstance(email, str):
        return None
    return normalize_waitlist_address(email)


def unsubscribe_path(token: str) -> str:
    return f"{UNSUBSCRIBE_PATH_PREFIX}{token}"


def unsubscribe_url(base_url: str, token: str) -> str:
    return f"{base_url.rstrip('/')}{unsubscribe_path(token)}"
