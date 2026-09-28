"""Regenerates ``sealed-v1.json``, the cross-language vector for sealed push
content (#200). Python's ``cryptography`` is a third implementation beside the
bridge's (RustCrypto ``p256``, ``hkdf`` and ``aes-gcm``) and the service
worker's (WebCrypto): both of those must open this blob, and the bridge must
reproduce it byte-for-byte from the same fixed ephemeral key and nonce.

Run from the repo root: ``skriftapp/.venv/bin/python fixtures/push/generate.py``.
The scheme is specified in ``planning/v2/Push Content Security Checklist.md``.
"""

import base64
import hashlib
import json
from pathlib import Path

from cryptography.hazmat.primitives import hashes, serialization
from cryptography.hazmat.primitives.asymmetric import ec
from cryptography.hazmat.primitives.ciphers.aead import AESGCM
from cryptography.hazmat.primitives.kdf.hkdf import HKDF

LABEL = b"build-push-v1"
PLAINTEXT_BYTES = 1024


def b64url(data: bytes) -> str:
    return base64.urlsafe_b64encode(data).rstrip(b"=").decode()


def private_key(scalar: int) -> ec.EllipticCurvePrivateKey:
    return ec.derive_private_key(scalar, ec.SECP256R1())


def raw_public(key: ec.EllipticCurvePrivateKey) -> bytes:
    return key.public_key().public_bytes(
        serialization.Encoding.X962, serialization.PublicFormat.UncompressedPoint
    )


def jwk(key: ec.EllipticCurvePrivateKey) -> dict:
    numbers = key.private_numbers()
    return {
        "kty": "EC",
        "crv": "P-256",
        "x": b64url(numbers.public_numbers.x.to_bytes(32, "big")),
        "y": b64url(numbers.public_numbers.y.to_bytes(32, "big")),
        "d": b64url(numbers.private_value.to_bytes(32, "big")),
    }


def main() -> None:
    recipient = private_key(0x1D0C_5EA1_ED00_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0001)
    ephemeral = private_key(0xE0E0_E0E0_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0000_0002)
    nonce = bytes(range(1, 13))
    endpoint = "https://fcm.googleapis.com/fcm/send/fixture-endpoint"
    subscription_id = b64url(hashlib.sha256(endpoint.encode()).digest())
    kind = "agent"
    entity_id = "run-fixture"
    plaintext = json.dumps(
        {
            "v": 1,
            "title": "Banner fade fixer",
            "body": "Fixed on build/banner-fade, ready for review — ünïcode ✓",
            "url": "/app/#/device/dev-1/project/proj-1/workspace/ws-1?agent=agent-1",
            "iat": 1_790_000_000,
        },
        separators=(",", ":"),
        ensure_ascii=False,
    ).encode()
    # Every plaintext is padded with trailing spaces to exactly 1024 bytes.
    plaintext = plaintext.ljust(PLAINTEXT_BYTES, b" ")

    recipient_public = raw_public(recipient)
    ephemeral_public = raw_public(ephemeral)
    shared = ephemeral.exchange(ec.ECDH(), recipient.public_key())
    key = HKDF(
        algorithm=hashes.SHA256(), length=32, salt=None, info=LABEL + ephemeral_public + recipient_public
    ).derive(shared)
    aad = b"\0".join([LABEL, subscription_id.encode(), kind.encode(), entity_id.encode()])
    sealed = AESGCM(key).encrypt(nonce, plaintext, aad)
    blob = b64url(b"\x01" + ephemeral_public + nonce + sealed)

    vector = {
        "endpoint": endpoint,
        "subscription_id": subscription_id,
        "kind": kind,
        "entity_id": entity_id,
        "recipient_private_jwk": jwk(recipient),
        "recipient_public_key": b64url(recipient_public),
        "ephemeral_private_jwk": jwk(ephemeral),
        "nonce": b64url(nonce),
        "plaintext": plaintext.decode(),
        "blob": blob,
    }
    out = Path(__file__).with_name("sealed-v1.json")
    out.write_text(json.dumps(vector, indent=2, ensure_ascii=False) + "\n")


if __name__ == "__main__":
    main()
