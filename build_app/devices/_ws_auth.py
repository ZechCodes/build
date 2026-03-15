"""WebSocket authentication — verify device identity before accepting messages."""

from __future__ import annotations

import logging
import time
from uuid import UUID

from litestar.connection import WebSocket
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from build_app.models import Device
from build_app.devices._crypto import load_public_key, verify_device_signature

logger = logging.getLogger(__name__)


class AuthResult:
    """Result of WS authentication — either success with device info or failure."""

    __slots__ = ("ok", "device_id", "owner_user_id", "device_name", "heartbeat_interval", "error")

    def __init__(
        self,
        *,
        ok: bool,
        device_id: UUID | None = None,
        owner_user_id: UUID | None = None,
        device_name: str = "",
        heartbeat_interval: int = 30,
        error: str = "",
    ):
        self.ok = ok
        self.device_id = device_id
        self.owner_user_id = owner_user_id
        self.device_name = device_name
        self.heartbeat_interval = heartbeat_interval
        self.error = error

    @classmethod
    def failure(cls, error: str) -> AuthResult:
        return cls(ok=False, error=error)


async def authenticate_device_ws(
    socket: WebSocket,
    db_session: AsyncSession,
) -> AuthResult:
    """Authenticate a device WebSocket connection.

    Validates headers, looks up the device, verifies the Ed25519 signature,
    and checks timestamp freshness. Does NOT call socket.accept() — the caller
    should accept only after auth succeeds.

    Returns an AuthResult with device info on success, or an error message on failure.
    """
    device_id_str = socket.headers.get("x-device-id", "")
    timestamp_str = socket.headers.get("x-timestamp", "")
    signature_b64 = socket.headers.get("x-signature", "")

    if not device_id_str or not timestamp_str or not signature_b64:
        return AuthResult.failure("missing auth headers")

    try:
        device_id = UUID(device_id_str)
    except ValueError:
        return AuthResult.failure("invalid device id")

    # Look up device in DB.
    result = await db_session.execute(
        select(Device).where(Device.id == device_id, Device.approved.is_(True))
    )
    device = result.scalar_one_or_none()

    if not device:
        return AuthResult.failure("device not found or not approved")

    # Verify signature.
    try:
        pub_key = load_public_key(device.public_key)
    except Exception:
        return AuthResult.failure("invalid stored public key")

    if not verify_device_signature(pub_key, timestamp_str, signature_b64):
        return AuthResult.failure("invalid signature")

    # Check timestamp freshness (5 min window).
    try:
        ts = float(timestamp_str)
        if abs(time.time() - ts) > 300:
            return AuthResult.failure("timestamp expired")
    except ValueError:
        return AuthResult.failure("invalid timestamp")

    return AuthResult(
        ok=True,
        device_id=device.id,
        owner_user_id=device.owner_user_id,
        device_name=device.name,
        heartbeat_interval=device.heartbeat_interval_s,
    )
