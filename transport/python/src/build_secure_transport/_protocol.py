from __future__ import annotations

import base64
import hashlib
import json
import uuid
from datetime import datetime, timezone
from typing import Any

from .exceptions import InvalidInputError, ProtocolError

PROTOCOL_VERSION = 1
FRAME_TYPES = {"data", "close"}
SENDERS = {"client", "device"}
SESSION_KEY_BYTES = 32
NONCE_BYTES = 24


def b64encode(data: bytes) -> str:
    return base64.b64encode(data).decode("ascii").rstrip("=")


def b64decode(data: str) -> bytes:
    if not isinstance(data, str) or not data:
        raise InvalidInputError("expected non-empty base64 string")
    padding = "=" * (-len(data) % 4)
    try:
        return base64.b64decode(data + padding, validate=True)
    except Exception as exc:  # pragma: no cover - base64 errors vary
        raise InvalidInputError("invalid base64 input") from exc


def canonical_json_bytes(value: Any) -> bytes:
    try:
        return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    except (TypeError, ValueError) as exc:
        raise InvalidInputError("value is not JSON serializable") from exc


def parse_json_bytes(data: bytes) -> Any:
    try:
        return json.loads(data.decode("utf-8"))
    except Exception as exc:
        raise ProtocolError("ciphertext did not contain valid UTF-8 JSON") from exc


def utc_now_rfc3339() -> str:
    return datetime.now(tz=timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z")


def new_message_id() -> str:
    return str(uuid.uuid4())


def fingerprint_public_key(public_key_b64: str) -> str:
    return hashlib.sha256(b64decode(public_key_b64)).hexdigest()


def require_dict(value: Any, field_name: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise InvalidInputError(f"{field_name} must be an object")
    return value


def require_string(value: Any, field_name: str) -> str:
    if not isinstance(value, str) or not value:
        raise InvalidInputError(f"{field_name} must be a non-empty string")
    return value


def validate_session_key(session_key_b64: str) -> bytes:
    session_key = b64decode(session_key_b64)
    if len(session_key) != SESSION_KEY_BYTES:
        raise InvalidInputError("session key must be 32 bytes")
    return session_key


def validate_envelope_shape(envelope: dict[str, Any]) -> dict[str, Any]:
    env = require_dict(envelope, "envelope")
    version = env.get("version", PROTOCOL_VERSION)
    if version != PROTOCOL_VERSION:
        raise ProtocolError(f"unsupported envelope version: {version}")

    session_id = require_string(env.get("session_id"), "session_id")
    route_to = require_string(env.get("route_to"), "route_to")
    nonce = b64decode(require_string(env.get("nonce"), "nonce"))
    ciphertext = b64decode(require_string(env.get("ciphertext"), "ciphertext"))

    if len(nonce) != NONCE_BYTES:
        raise InvalidInputError("nonce must decode to 24 bytes")
    if not ciphertext:
        raise InvalidInputError("ciphertext must not be empty")

    return {
        "version": version,
        "session_id": session_id,
        "route_to": route_to,
        "nonce": nonce,
        "ciphertext": ciphertext,
    }


def validate_frame(frame: dict[str, Any], expected_session_id: str) -> dict[str, Any]:
    if not isinstance(frame, dict):
        raise ProtocolError("inner frame must be an object")

    session_id = require_string(frame.get("session_id"), "frame.session_id")
    if session_id != expected_session_id:
        raise ProtocolError("inner and outer session_id did not match")

    frame_type = require_string(frame.get("frame_type"), "frame.frame_type")
    sender = require_string(frame.get("sender"), "frame.sender")
    message_id = require_string(frame.get("message_id"), "frame.message_id")
    created_at = require_string(frame.get("created_at"), "frame.created_at")

    if frame_type not in FRAME_TYPES:
        raise ProtocolError(f"unsupported frame_type: {frame_type}")
    if sender not in SENDERS:
        raise ProtocolError(f"unsupported sender: {sender}")
    if "payload" not in frame:
        raise ProtocolError("frame.payload is required")

    return {
        "session_id": session_id,
        "message_id": message_id,
        "frame_type": frame_type,
        "sender": sender,
        "created_at": created_at,
        "payload": frame["payload"],
    }
