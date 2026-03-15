"""Cryptographic helpers — key loading and signature verification."""

from __future__ import annotations

import base64

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
from cryptography.hazmat.primitives.serialization import load_der_public_key


def load_public_key(public_key_b64: str) -> Ed25519PublicKey:
    """Load an Ed25519 public key from base64 (raw 32-byte or DER)."""
    raw = base64.b64decode(public_key_b64)
    if len(raw) == 32:
        return Ed25519PublicKey.from_public_bytes(raw)
    return load_der_public_key(raw)  # type: ignore[return-value]


def verify_device_signature(
    public_key: Ed25519PublicKey,
    timestamp_str: str,
    signature_b64: str,
    path: str = "/api/devices/ws",
) -> bool:
    """Verify an Ed25519 signature over ``{timestamp}.GET.{path}``."""
    message = f"{timestamp_str}.GET.{path}".encode()
    try:
        signature = base64.b64decode(signature_b64)
        public_key.verify(signature, message)
        return True
    except Exception:
        return False


def normalize_b64_padding(b64: str) -> str:
    """Add missing ``=`` padding to a base64 string."""
    return b64 + "=" * (-len(b64) % 4)
