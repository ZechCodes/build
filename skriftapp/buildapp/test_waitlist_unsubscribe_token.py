"""Tests for the signed unsubscribe token: it round-trips the normalised address, it is
the only authorisation the unsubscribe routes accept, and every way of presenting a token
the app did not sign for this purpose reads as None."""

from __future__ import annotations

from skrift.auth.tokens import create_signed_token

from buildapp.waitlist_unsubscribe_token import (
    UNSUBSCRIBE_PATH_PREFIX,
    UNSUBSCRIBE_TOKEN_PURPOSE,
    UNSUBSCRIBE_TOKEN_TTL_SECONDS,
    mint_unsubscribe_token,
    read_unsubscribe_token,
    unsubscribe_path,
    unsubscribe_url,
)

SIGNING_KEY = "waitlist-unsubscribe-signing-key"
OTHER_SIGNING_KEY = "a-different-signing-key"
SIGNER_ADDRESS = "alice@example.com"
FIVE_YEARS_IN_SECONDS = 157_680_000


def test_round_trip_returns_the_normalised_address():
    token = mint_unsubscribe_token(SIGNER_ADDRESS, SIGNING_KEY)
    assert read_unsubscribe_token(token, SIGNING_KEY) == SIGNER_ADDRESS


def test_mixed_case_address_round_trips_lowercased():
    token = mint_unsubscribe_token("  Alice@Example.COM ", SIGNING_KEY)
    assert read_unsubscribe_token(token, SIGNING_KEY) == SIGNER_ADDRESS


def test_tampered_payload_reads_as_none():
    token = mint_unsubscribe_token(SIGNER_ADDRESS, SIGNING_KEY)
    payload, signature = token.split(".")
    flipped = "A" if payload[5] != "A" else "B"
    tampered = f"{payload[:5]}{flipped}{payload[6:]}.{signature}"
    assert read_unsubscribe_token(tampered, SIGNING_KEY) is None


def test_truncated_token_reads_as_none():
    token = mint_unsubscribe_token(SIGNER_ADDRESS, SIGNING_KEY)
    assert read_unsubscribe_token(token[: len(token) // 2], SIGNING_KEY) is None


def test_token_signed_with_another_secret_reads_as_none():
    token = mint_unsubscribe_token(SIGNER_ADDRESS, OTHER_SIGNING_KEY)
    assert read_unsubscribe_token(token, SIGNING_KEY) is None


def test_token_with_another_purpose_reads_as_none():
    token = create_signed_token(
        {"purpose": "oauth_link_verify", "email": SIGNER_ADDRESS},
        SIGNING_KEY,
        UNSUBSCRIBE_TOKEN_TTL_SECONDS,
    )
    assert read_unsubscribe_token(token, SIGNING_KEY) is None


def test_token_with_non_string_email_reads_as_none():
    token = create_signed_token(
        {"purpose": UNSUBSCRIBE_TOKEN_PURPOSE, "email": 42},
        SIGNING_KEY,
        UNSUBSCRIBE_TOKEN_TTL_SECONDS,
    )
    assert read_unsubscribe_token(token, SIGNING_KEY) is None


def test_token_with_invalid_address_reads_as_none():
    token = create_signed_token(
        {"purpose": UNSUBSCRIBE_TOKEN_PURPOSE, "email": "not-an-email"},
        SIGNING_KEY,
        UNSUBSCRIBE_TOKEN_TTL_SECONDS,
    )
    assert read_unsubscribe_token(token, SIGNING_KEY) is None


def test_expired_token_reads_as_none():
    token = create_signed_token(
        {"purpose": UNSUBSCRIBE_TOKEN_PURPOSE, "email": SIGNER_ADDRESS},
        SIGNING_KEY,
        -1,
    )
    assert read_unsubscribe_token(token, SIGNING_KEY) is None


def test_ttl_is_five_years():
    assert UNSUBSCRIBE_TOKEN_TTL_SECONDS == FIVE_YEARS_IN_SECONDS


def test_unsubscribe_url_joins_base_and_path_without_a_double_slash():
    token = mint_unsubscribe_token(SIGNER_ADDRESS, SIGNING_KEY)
    expected = f"https://getbuild.ing{unsubscribe_path(token)}"
    assert unsubscribe_url("https://getbuild.ing", token) == expected
    assert unsubscribe_url("https://getbuild.ing/", token) == expected


def test_path_is_under_the_waitlist_prefix():
    token = mint_unsubscribe_token(SIGNER_ADDRESS, SIGNING_KEY)
    assert unsubscribe_path(token) == f"{UNSUBSCRIBE_PATH_PREFIX}{token}"
    assert unsubscribe_path(token).startswith("/waitlist/unsubscribe/")
