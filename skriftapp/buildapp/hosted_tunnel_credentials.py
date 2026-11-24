"""Protect one locally managed Cloudflare connector secret per generation."""

import base64
import binascii
import os
import secrets
from pathlib import Path
from typing import TypedDict, cast

from cryptography.exceptions import InvalidTag
from cryptography.hazmat.primitives.ciphers.aead import AESGCM

from buildapp.hosted_tunnel_models import HostedTunnelAssignment


class ConnectorCredentials(TypedDict):
    AccountTag: str
    TunnelID: str
    TunnelSecret: str


class _RedactedCredentials(dict[str, str]):
    def __repr__(self):
        return "<Cloudflare connector credentials>"

    __str__ = __repr__


def load_wrapping_key() -> bytes:
    try:
        with Path(os.environ["HOSTED_TUNNEL_WRAPPING_KEY_FILE"]).open("rb") as stream:
            key = base64.b64decode(stream.read(128).strip(), validate=True)
        if len(key) != 32:
            raise ValueError
    except (KeyError, OSError, ValueError, binascii.Error):
        raise ValueError("HOSTED_TUNNEL_WRAPPING_KEY_FILE must contain a base64-encoded 32-byte key") from None
    return key


def _binding(row: HostedTunnelAssignment) -> bytes:
    return (
        f"build-hosted-tunnel-v1:{row.device_id}:{row.owner_user_id}:"
        f"{row.pairing_created_at.isoformat()}:{row.authorization_epoch}:{row.generation}:{row.hostname}"
    ).encode()


def new_encrypted_secret(row: HostedTunnelAssignment, wrapping_key: bytes) -> bytes:
    if len(wrapping_key) != 32:
        raise ValueError("invalid hosted tunnel wrapping key")
    nonce = secrets.token_bytes(12)
    return nonce + AESGCM(wrapping_key).encrypt(nonce, secrets.token_bytes(32), _binding(row))


def decrypt_tunnel_secret(row: HostedTunnelAssignment, wrapping_key: bytes) -> bytes:
    try:
        if len(wrapping_key) != 32:
            raise ValueError
        nonce, ciphertext = row.encrypted_secret[:12], row.encrypted_secret[12:]
        return AESGCM(wrapping_key).decrypt(nonce, ciphertext, _binding(row))
    except (InvalidTag, ValueError):
        raise ValueError("hosted tunnel credential unavailable") from None


def materialize_connector_credentials(
    assignment: HostedTunnelAssignment, *, account_id: str, wrapping_key: bytes,
) -> ConnectorCredentials:
    """Materialize credentials after the caller verifies current claim authority."""
    if assignment.state != "ready" or assignment.tunnel_id is None or not account_id:
        raise ValueError("hosted tunnel credential unavailable")
    secret = decrypt_tunnel_secret(assignment, wrapping_key)
    return cast(ConnectorCredentials, _RedactedCredentials(
        AccountTag=account_id,
        TunnelID=str(assignment.tunnel_id),
        TunnelSecret=base64.b64encode(secret).decode("ascii"),
    ))
