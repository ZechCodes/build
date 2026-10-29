"""Rust jsonwebtoken and hosted PyJWT accept the same signed fixed vectors."""

import json
from datetime import datetime, timedelta, timezone
from pathlib import Path
from uuid import UUID

import jwt
import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from litestar.exceptions import NotAuthorizedException

from buildapp.bridge_http_proofs import verify
from buildapp.models import Device

VECTOR = json.loads(
    (Path(__file__).parents[2] / "fixtures/bridge-http/device-proofs.json").read_text()
)
NOW = datetime.fromtimestamp(VECTOR["now"], timezone.utc)
DEVICE = Device(
    id=UUID("aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"),
    identity_public_key_b64=VECTOR["identity_public_key_b64"],
)


@pytest.mark.parametrize("purpose", ["registration", "discovery", "introspection"])
def test_rust_signed_standard_eddsa_vectors_verify(purpose):
    claims = verify(VECTOR[purpose], DEVICE, purpose, "https://getbuild.ing", NOW)
    assert claims["iss"] == str(DEVICE.id)
    assert claims["exp"] - claims["iat"] == 30
    for other in {"registration", "discovery", "introspection"} - {purpose}:
        with pytest.raises(NotAuthorizedException):
            verify(VECTOR[purpose], DEVICE, other, "https://getbuild.ing", NOW)


@pytest.mark.parametrize(
    "claim,value",
    [
        ("aud", "https://evil.example"),
        ("iss", "bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb"),
        ("sub", "other"),
        ("exp", 1800000031),
        ("iat", 1800000006),
        ("iat", True),
        ("jti", "tiny"),
    ],
)
def test_signed_wrong_claims_refused(claim, value):
    claims = verify(
        VECTOR["discovery"], DEVICE, "discovery", "https://getbuild.ing", NOW
    )
    claims[claim] = value
    proof = jwt.encode(
        claims,
        Ed25519PrivateKey.from_private_bytes(bytes([7]) * 32),
        algorithm="EdDSA",
        headers={"typ": "build-http-discovery+jwt"},
    )
    with pytest.raises(NotAuthorizedException):
        verify(proof, DEVICE, "discovery", "https://getbuild.ing", NOW)


@pytest.mark.parametrize(
    "header",
    [
        {"typ": "build-http-registration+jwt"},
        {"typ": "build-http-discovery+jwt", "jku": "https://evil.example"},
        {"typ": "build-http-discovery+jwt", "crit": ["x"]},
        {"typ": "build-http-discovery+jwt", "kid": "attacker"},
    ],
)
def test_remote_keys_critical_headers_and_type_substitution_refused(header):
    claims = verify(
        VECTOR["discovery"], DEVICE, "discovery", "https://getbuild.ing", NOW
    )
    proof = jwt.encode(
        claims,
        Ed25519PrivateKey.from_private_bytes(bytes([7]) * 32),
        algorithm="EdDSA",
        headers=header,
    )
    with pytest.raises(NotAuthorizedException):
        verify(proof, DEVICE, "discovery", "https://getbuild.ing", NOW)


def test_expiry_boundary_matches_replay_record_purge_and_pinned_key():
    with pytest.raises(NotAuthorizedException):
        verify(
            VECTOR["discovery"],
            DEVICE,
            "discovery",
            "https://getbuild.ing",
            NOW + timedelta(seconds=35, microseconds=1),
        )
    wrong = Device(
        id=DEVICE.id,
        identity_public_key_b64="AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=",
    )
    with pytest.raises(NotAuthorizedException):
        verify(VECTOR["discovery"], wrong, "discovery", "https://getbuild.ing", NOW)


def test_real_bridge_unpadded_paired_public_key_verifies():
    device = Device(
        id=DEVICE.id,
        identity_public_key_b64=VECTOR["identity_public_key_b64"].rstrip("="),
    )
    assert verify(
        VECTOR["discovery"], device, "discovery", "https://getbuild.ing", NOW
    )["sub"] == str(DEVICE.id)
