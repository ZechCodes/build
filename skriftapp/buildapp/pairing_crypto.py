"""Pure crypto helpers for device pairing — the api side of the bridge's flow.

These mirror the Rust bridge (`bridge/src/transport.rs` + `bridge/src/pairing.rs`)
byte-for-byte so a signature the bridge produces verifies here and the fingerprints
shown on both sides match:

- ``fingerprint`` == ``transport::fingerprint_identity_key`` (SHA-256 hex of the
  decoded Ed25519 public key).
- ``registration_challenge`` == ``pairing::registration_challenge`` (same format).
- ``hash_code`` == ``pairing::hash_pairing_code`` (SHA-256 hex of the raw code).
- ``verify_registration`` checks the Ed25519 signature the bridge made with
  ``transport::sign_message_b64``.

No DB or framework imports — keep this trivially unit-testable.
"""

from __future__ import annotations

import base64
import hashlib

from cryptography.exceptions import InvalidSignature
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey


def b64decode_lenient(value: str) -> bytes:
    """Decode standard base64, tolerating missing padding.

    The bridge encodes public keys unpadded (``STANDARD_NO_PAD``) but signs with
    padded standard base64, and the relay challenge headers are padded — so accept
    either by re-padding before decoding.
    """
    stripped = value.strip().rstrip("=")
    padding = "=" * ((-len(stripped)) % 4)
    return base64.b64decode(stripped + padding)


def fingerprint(identity_public_key_b64: str) -> str:
    """SHA-256 hex of the decoded Ed25519 identity public key (for the human compare)."""
    return hashlib.sha256(b64decode_lenient(identity_public_key_b64)).hexdigest()


def hash_code(code: str) -> str:
    """SHA-256 hex of a raw pairing code — what we store and compare against."""
    return hashlib.sha256(code.encode("utf-8")).hexdigest()


def registration_challenge(
    device_id: str,
    identity_public_key_b64: str,
    transport_public_key_b64: str,
    pairing_code_hash: str,
) -> str:
    """The canonical message the bridge signs at registration. Binds every field so a
    captured signature cannot be replayed onto a different registration."""
    return (
        f"register.{device_id}.{identity_public_key_b64}."
        f"{transport_public_key_b64}.{pairing_code_hash}"
    )


def verify_registration(
    identity_public_key_b64: str,
    challenge: str,
    signature_b64: str,
) -> bool:
    """Verify an Ed25519 signature over ``challenge`` against the identity public key.
    Proof the registrant holds the identity private key. Never raises."""
    try:
        public_key = Ed25519PublicKey.from_public_bytes(
            b64decode_lenient(identity_public_key_b64)
        )
        public_key.verify(b64decode_lenient(signature_b64), challenge.encode("utf-8"))
        return True
    except (InvalidSignature, ValueError):
        return False
