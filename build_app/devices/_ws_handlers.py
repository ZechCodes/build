"""WebSocket message dispatch — handles heartbeat, transport_key, E2EE relay, status."""

from __future__ import annotations

import base64
import json
import logging
import sys
import time
from typing import Any
from uuid import UUID

from litestar.connection import WebSocket
from sqlalchemy import select, update
from sqlalchemy.ext.asyncio import AsyncSession

from skrift.lib.notifications import notify_user, NotificationMode

from build_app.models import Device
from build_app.devices._state import (
    ConnectedDevice,
    E2ESession,
    _connected_devices,
    _e2e_sessions,
    MAX_ENVELOPE_SIZE,
)
from build_app.devices._crypto import normalize_b64_padding
from build_app.devices._helpers import (
    record_heartbeat,
    notify_device_event,
)

logger = logging.getLogger(__name__)

# Valid route_to values for E2EE envelopes.
_VALID_ROUTES = {"client", "device"}


async def handle_ws_message(
    msg: dict[str, Any],
    *,
    socket: WebSocket,
    conn: ConnectedDevice,
    device_id: UUID,
    owner_user_id: UUID,
    device_name: str,
    session_maker: Any,
) -> None:
    """Dispatch a single WS message from a device."""
    msg_type = msg.get("type")
    rid = msg.get("rid")

    handler = _HANDLERS.get(msg_type)
    if handler:
        await handler(
            msg, rid=rid, socket=socket, conn=conn,
            device_id=device_id, owner_user_id=owner_user_id,
            device_name=device_name, session_maker=session_maker,
        )
    else:
        if rid:
            await socket.send_json({
                "type": "response", "rid": rid,
                "ok": False, "error": f"unknown message type: {msg_type}",
            })


# ---------------------------------------------------------------------------
# Individual handlers
# ---------------------------------------------------------------------------

async def _handle_heartbeat(
    msg: dict, *, rid: str | None, socket: WebSocket, conn: ConnectedDevice,
    device_id: UUID, session_maker: Any, **_: Any,
) -> None:
    conn.last_heartbeat = time.time()
    async with session_maker() as db_session:
        result = await db_session.execute(
            select(Device).where(Device.id == device_id)
        )
        dev = result.scalar_one_or_none()
        if dev:
            await record_heartbeat(db_session, dev)

    if rid:
        await socket.send_json({"type": "response", "rid": rid, "ok": True})


async def _handle_transport_key(
    msg: dict, *, rid: str | None, socket: WebSocket,
    device_id: UUID, owner_user_id: UUID, device_name: str,
    session_maker: Any, **_: Any,
) -> None:
    transport_key_b64 = msg.get("transport_public_key", "").strip()
    if not transport_key_b64:
        if rid:
            await socket.send_json({
                "type": "response", "rid": rid,
                "ok": False, "error": "transport_public_key required",
            })
        return

    # Validate it's plausible base64 (32 bytes decoded).
    try:
        padded = normalize_b64_padding(transport_key_b64)
        raw = base64.b64decode(padded)
        if len(raw) != 32:
            raise ValueError("must be 32 bytes")
    except Exception:
        if rid:
            await socket.send_json({
                "type": "response", "rid": rid,
                "ok": False, "error": "invalid X25519 public key",
            })
        return

    async with session_maker() as db_session:
        await db_session.execute(
            update(Device)
            .where(Device.id == device_id)
            .values(transport_public_key=transport_key_b64)
        )
        await db_session.commit()

    logger.info("Device %s uploaded transport key", device_id)

    # Notify browser clients that the device is E2EE-ready.
    await notify_device_event(
        owner_user_id, "e2ee-ready",
        device_id, device_name,
    )

    if rid:
        await socket.send_json({"type": "response", "rid": rid, "ok": True})


async def _handle_e2ee_relay(
    msg: dict, *, rid: str | None, socket: WebSocket,
    device_id: UUID, **_: Any,
) -> None:
    """Handle both session_accept and e2ee_envelope — they have identical relay logic."""
    session_id = msg.get("session_id")
    envelope = msg.get("envelope")

    if not session_id or not envelope:
        if rid:
            await socket.send_json({
                "type": "response", "rid": rid,
                "ok": False, "error": "session_id and envelope required",
            })
        return

    # Validate envelope size.
    envelope_size = sys.getsizeof(json.dumps(envelope)) if isinstance(envelope, dict) else len(str(envelope))
    if envelope_size > MAX_ENVELOPE_SIZE:
        if rid:
            await socket.send_json({
                "type": "response", "rid": rid,
                "ok": False, "error": "envelope too large",
            })
        return

    session = _e2e_sessions.get(session_id)
    if not session or session.device_id != device_id:
        if rid:
            await socket.send_json({
                "type": "response", "rid": rid,
                "ok": False, "error": "unknown session",
            })
        return

    # Check session expiry.
    if session.expired:
        _e2e_sessions.pop(session_id, None)
        if rid:
            await socket.send_json({
                "type": "response", "rid": rid,
                "ok": False, "error": "session expired",
            })
        return

    session.touch()
    # Forward as ephemeral notification to the browser.
    await notify_user(
        str(session.owner_user_id),
        "build:e2ee:envelope",
        mode=NotificationMode.EPHEMERAL,
        push_notify=False,
        session_id=session_id,
        envelope=envelope,
    )
    if rid:
        await socket.send_json({"type": "response", "rid": rid, "ok": True})


async def _handle_status(
    msg: dict, *, rid: str | None, socket: WebSocket,
    device_id: UUID, owner_user_id: UUID, device_name: str,
    **_: Any,
) -> None:
    await notify_device_event(
        owner_user_id, "status",
        device_id, device_name,
        **{k: v for k, v in msg.items() if k not in ("type", "rid")},
    )
    if rid:
        await socket.send_json({"type": "response", "rid": rid, "ok": True})


# Handler dispatch table.
_HANDLERS: dict[str, Any] = {
    "heartbeat": _handle_heartbeat,
    "transport_key": _handle_transport_key,
    "session_accept": _handle_e2ee_relay,
    "e2ee_envelope": _handle_e2ee_relay,
    "status": _handle_status,
}
