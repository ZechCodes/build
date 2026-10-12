from __future__ import annotations

from typing import Any

from nacl import encoding, public, secret, signing, utils
from nacl.exceptions import BadSignatureError, CryptoError

from ._protocol import (
    NONCE_BYTES,
    PROTOCOL_VERSION,
    b64decode,
    b64encode,
    canonical_json_bytes,
    fingerprint_public_key,
    new_message_id,
    parse_json_bytes,
    require_dict,
    require_string,
    utc_now_rfc3339,
    validate_envelope_shape,
    validate_frame,
    validate_session_key,
)
from .exceptions import DecryptionError, ProtocolError, VerificationError


def generate_transport_keypair() -> dict[str, str]:
    private_key = public.PrivateKey.generate()
    return {
        "public_key_b64": b64encode(bytes(private_key.public_key)),
        "private_key_b64": b64encode(bytes(private_key)),
    }


def generate_identity_keypair() -> dict[str, str]:
    signing_key = signing.SigningKey.generate()
    return {
        "public_key_b64": b64encode(bytes(signing_key.verify_key)),
        "private_key_b64": b64encode(bytes(signing_key)),
    }


def generate_session_key() -> str:
    return b64encode(utils.random(secret.SecretBox.KEY_SIZE))


def sign_transport_key(identity_private_key_b64: str, transport_public_key_b64: str) -> str:
    signer = signing.SigningKey(b64decode(identity_private_key_b64))
    signature = signer.sign(b64decode(transport_public_key_b64)).signature
    return b64encode(signature)


def verify_transport_key_signature(
    identity_public_key_b64: str,
    transport_public_key_b64: str,
    signature_b64: str,
) -> None:
    verify_key = signing.VerifyKey(b64decode(identity_public_key_b64))
    try:
        verify_key.verify(b64decode(transport_public_key_b64), b64decode(signature_b64))
    except BadSignatureError as exc:
        raise VerificationError("transport key signature verification failed") from exc


def fingerprint_identity_key(identity_public_key_b64: str) -> str:
    return fingerprint_public_key(identity_public_key_b64)


def _secretbox_encrypt_payload(session_key: bytes, payload: bytes, nonce: bytes | None = None) -> dict[str, str]:
    box = secret.SecretBox(session_key)
    final_nonce = nonce or utils.random(NONCE_BYTES)
    encrypted = box.encrypt(payload, final_nonce, encoder=encoding.RawEncoder)
    return {
        "nonce": b64encode(final_nonce),
        "ciphertext": b64encode(encrypted.ciphertext),
    }


def open_session_init(device_transport_private_key_b64: str, session_init: dict[str, Any]) -> dict[str, str]:
    payload = require_dict(session_init, "session_init")
    session_id = require_string(payload.get("session_id"), "session_id")
    device_id = require_string(payload.get("device_id"), "device_id")
    wrapped_session_key = b64decode(require_string(payload.get("wrapped_session_key"), "wrapped_session_key"))

    private_key = public.PrivateKey(b64decode(device_transport_private_key_b64))
    sealed_box = public.SealedBox(private_key)

    try:
        session_key = sealed_box.decrypt(wrapped_session_key)
    except CryptoError as exc:
        raise DecryptionError("failed to unwrap session key from session_init") from exc

    if len(session_key) != secret.SecretBox.KEY_SIZE:
        raise ProtocolError("wrapped session key did not decrypt to 32 bytes")

    return {
        "session_key_b64": b64encode(session_key),
        "session_id": session_id,
        "device_id": device_id,
    }


def build_session_accept(
    session_key_b64: str,
    session_id: str,
    route_to: str,
    nonce_b64: str | None = None,
) -> dict[str, Any]:
    session_key = validate_session_key(session_key_b64)
    require_string(session_id, "session_id")
    require_string(route_to, "route_to")
    nonce = b64decode(nonce_b64) if nonce_b64 else None
    if nonce is not None and len(nonce) != NONCE_BYTES:
        raise ProtocolError("provided nonce must be 24 bytes")

    encrypted = _secretbox_encrypt_payload(
        session_key,
        canonical_json_bytes({"session_id": session_id}),
        nonce=nonce,
    )
    return {
        "version": PROTOCOL_VERSION,
        "session_id": session_id,
        "route_to": route_to,
        "nonce": encrypted["nonce"],
        "ciphertext": encrypted["ciphertext"],
    }


def encrypt_frame(
    session_key_b64: str,
    outer_fields: dict[str, Any],
    frame_fields: dict[str, Any],
    nonce_b64: str | None = None,
) -> dict[str, Any]:
    session_key = validate_session_key(session_key_b64)
    outer = require_dict(outer_fields, "outer_fields")
    frame = require_dict(frame_fields, "frame_fields")

    session_id = require_string(outer.get("session_id"), "outer_fields.session_id")
    route_to = require_string(outer.get("route_to"), "outer_fields.route_to")
    sender = require_string(frame.get("sender"), "frame_fields.sender")
    frame_type = require_string(frame.get("frame_type"), "frame_fields.frame_type")

    inner_frame = {
        "session_id": session_id,
        "message_id": frame.get("message_id") or new_message_id(),
        "frame_type": frame_type,
        "sender": sender,
        "created_at": frame.get("created_at") or utc_now_rfc3339(),
        "payload": frame.get("payload"),
    }
    validate_frame(inner_frame, session_id)

    nonce = b64decode(nonce_b64) if nonce_b64 else None
    if nonce is not None and len(nonce) != NONCE_BYTES:
        raise ProtocolError("provided nonce must be 24 bytes")

    encrypted = _secretbox_encrypt_payload(session_key, canonical_json_bytes(inner_frame), nonce=nonce)
    return {
        "version": PROTOCOL_VERSION,
        "session_id": session_id,
        "route_to": route_to,
        "nonce": encrypted["nonce"],
        "ciphertext": encrypted["ciphertext"],
    }


def decrypt_envelope(session_key_b64: str, envelope: dict[str, Any]) -> dict[str, Any]:
    session_key = validate_session_key(session_key_b64)
    parsed = validate_envelope_shape(envelope)
    box = secret.SecretBox(session_key)

    try:
        plaintext = box.decrypt(parsed["ciphertext"], parsed["nonce"], encoder=encoding.RawEncoder)
    except CryptoError as exc:
        raise DecryptionError("envelope decryption failed") from exc

    frame = parse_json_bytes(plaintext)
    return validate_frame(frame, parsed["session_id"])


def build_close_frame(
    session_key_b64: str,
    session_id: str,
    route_to: str,
    sender: str,
    payload: Any = None,
    nonce_b64: str | None = None,
) -> dict[str, Any]:
    return encrypt_frame(
        session_key_b64,
        outer_fields={"session_id": session_id, "route_to": route_to},
        frame_fields={
            "frame_type": "close",
            "sender": sender,
            "payload": payload,
        },
        nonce_b64=nonce_b64,
    )
