"""Tests for the pairing crypto helpers — including cross-language compatibility with
the Rust bridge (same SHA-256 constants and challenge format the Rust tests assert)."""

from __future__ import annotations

import base64

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from buildapp import pairing_crypto


def _b64_nopad(raw: bytes) -> str:
    return base64.b64encode(raw).decode().rstrip("=")


def test_hash_code_matches_rust_known_answer():
    # SHA-256("abc") — identical to the Rust hash_pairing_code known-answer test.
    assert (
        pairing_crypto.hash_code("abc")
        == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    )


def test_fingerprint_matches_rust_known_answer():
    # SHA-256 of 32 zero bytes — identical to the Rust fingerprint_identity_key test.
    zero_pub = _b64_nopad(bytes(32))
    assert (
        pairing_crypto.fingerprint(zero_pub)
        == "66687aadf862bd776c8fc18b8e9f8e20089714856ee233b3902a591d0d5f2925"
    )


def test_registration_challenge_format():
    assert (
        pairing_crypto.registration_challenge("d", "idpub", "tppub", "h")
        == "register.d.idpub.tppub.h"
    )


def test_verify_registration_roundtrip():
    priv = Ed25519PrivateKey.generate()
    pub_raw = priv.public_key().public_bytes_raw()
    pub_b64 = _b64_nopad(pub_raw)
    challenge = pairing_crypto.registration_challenge("dev-1", pub_b64, "tp", "codehash")
    # Sign with padded standard base64 (matching transport::sign_message_b64).
    sig_b64 = base64.b64encode(priv.sign(challenge.encode())).decode()

    assert pairing_crypto.verify_registration(pub_b64, challenge, sig_b64) is True


def test_verify_registration_rejects_tampered_challenge():
    priv = Ed25519PrivateKey.generate()
    pub_b64 = _b64_nopad(priv.public_key().public_bytes_raw())
    challenge = pairing_crypto.registration_challenge("dev-1", pub_b64, "tp", "codehash")
    sig_b64 = base64.b64encode(priv.sign(challenge.encode())).decode()

    forged = pairing_crypto.registration_challenge("dev-1", pub_b64, "tp", "OTHER")
    assert pairing_crypto.verify_registration(pub_b64, forged, sig_b64) is False


def test_verify_registration_rejects_wrong_key():
    priv = Ed25519PrivateKey.generate()
    other = Ed25519PrivateKey.generate()
    pub_b64 = _b64_nopad(priv.public_key().public_bytes_raw())
    other_pub_b64 = _b64_nopad(other.public_key().public_bytes_raw())
    challenge = pairing_crypto.registration_challenge("dev-1", pub_b64, "tp", "codehash")
    sig_b64 = base64.b64encode(priv.sign(challenge.encode())).decode()

    assert pairing_crypto.verify_registration(other_pub_b64, challenge, sig_b64) is False


def test_b64decode_lenient_handles_padded_and_unpadded():
    raw = bytes(range(32))
    assert pairing_crypto.b64decode_lenient(_b64_nopad(raw)) == raw
    assert pairing_crypto.b64decode_lenient(base64.b64encode(raw).decode()) == raw
