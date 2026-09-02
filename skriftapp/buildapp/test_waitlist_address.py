"""Tests for the pure waitlist address normaliser: what shape of address the prelaunch
landing page accepts, and the exact string that reaches the database."""

from __future__ import annotations

from buildapp.waitlist_address import MAX_WAITLIST_ADDRESS_LENGTH, normalize_waitlist_address


def test_lowercases_and_strips_surrounding_whitespace():
    assert normalize_waitlist_address("  Alice@Example.COM ") == "alice@example.com"


def test_accepts_plus_addressing_and_subdomains():
    assert (
        normalize_waitlist_address("alice+beta@mail.example.co.uk")
        == "alice+beta@mail.example.co.uk"
    )


def test_rejects_empty_and_whitespace_only():
    assert normalize_waitlist_address("") is None
    assert normalize_waitlist_address("   ") is None


def test_rejects_address_without_at_sign():
    assert normalize_waitlist_address("alice.example.com") is None


def test_rejects_address_without_a_dotted_domain():
    assert normalize_waitlist_address("alice@localhost") is None


def test_rejects_address_with_internal_whitespace():
    assert normalize_waitlist_address("ali ce@example.com") is None
    assert normalize_waitlist_address("alice@exa mple.com") is None


def test_rejects_local_parts_that_break_a_mail_header():
    for raw in ("a<b>@example.com", 'a"b@example.com', "a,b@example.com", "a(b)@example.com", "a:b@example.com"):
        assert normalize_waitlist_address(raw) is None


def test_rejects_address_with_two_at_signs():
    assert normalize_waitlist_address("alice@@example.com") is None
    assert normalize_waitlist_address("alice@example@com.org") is None


def test_rejects_address_longer_than_the_maximum():
    domain = "@example.com"
    local_part = "a" * (MAX_WAITLIST_ADDRESS_LENGTH - len(domain) + 1)
    assert normalize_waitlist_address(local_part + domain) is None


def test_accepts_address_exactly_at_the_maximum():
    domain = "@example.com"
    local_part = "a" * (MAX_WAITLIST_ADDRESS_LENGTH - len(domain))
    address = local_part + domain
    assert len(address) == MAX_WAITLIST_ADDRESS_LENGTH
    assert normalize_waitlist_address(address) == address
