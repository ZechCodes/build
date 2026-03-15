"""Device API — registration, authorization, listing, WebSocket, heartbeat monitoring, and E2EE relay."""

from __future__ import annotations

import asyncio
import base64
import json
import logging
import secrets
import time
from collections.abc import AsyncGenerator
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any
from uuid import UUID

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
from litestar import Controller, Request, get, post, delete, websocket
from litestar.connection import WebSocket
from litestar.exceptions import NotAuthorizedException, NotFoundException
from litestar.response import Response, Template
from litestar.response.sse import ServerSentEvent, ServerSentEventMessage
from sqlalchemy import select, update
from sqlalchemy.ext.asyncio import AsyncSession

from skrift.auth.guards import auth_guard, Permission
from skrift.lib.notifications import notify_user, NotificationMode

from build_app.models import Device


# ---------------------------------------------------------------------------
# E2EE session registry — maps session_id to routing info
# ---------------------------------------------------------------------------
@dataclass
class E2ESession:
    """Tracks an active E2EE session between a browser and a device."""
    session_id: str
    device_id: UUID
    owner_user_id: UUID
    created_at: float = field(default_factory=time.time)


_e2e_sessions: dict[str, E2ESession] = {}


def get_e2e_sessions() -> dict[str, E2ESession]:
    """Expose for testing."""
    return _e2e_sessions

logger = logging.getLogger(__name__)

# How many seconds without a heartbeat before we consider the device offline.
HEARTBEAT_TIMEOUT_MULTIPLIER = 2.5
# Max missed windows to keep per device (rolling).
MAX_MISSED_WINDOWS = 100
# How long a pending registration is valid (10 minutes).
PENDING_EXPIRY_S = 600


# ---------------------------------------------------------------------------
# In-memory pending registrations (short-lived, for auth flow)
# ---------------------------------------------------------------------------
@dataclass
class PendingRegistration:
    code: str
    name: str
    public_key_b64: str
    created_at: float = field(default_factory=time.time)
    result_queue: asyncio.Queue = field(default_factory=asyncio.Queue)


_pending_registrations: dict[str, PendingRegistration] = {}


def _cleanup_expired_pending() -> None:
    """Remove expired pending registrations."""
    now = time.time()
    expired = [
        code for code, reg in _pending_registrations.items()
        if now - reg.created_at > PENDING_EXPIRY_S
    ]
    for code in expired:
        _pending_registrations.pop(code, None)


# ---------------------------------------------------------------------------
# In-memory registry of connected devices (for heartbeat monitor)
# ---------------------------------------------------------------------------
@dataclass
class ConnectedDevice:
    device_id: UUID
    owner_user_id: UUID
    socket: WebSocket
    last_heartbeat: float = field(default_factory=time.time)
    heartbeat_interval: int = 30


_connected_devices: dict[UUID, ConnectedDevice] = {}


def get_connected_devices() -> dict[UUID, ConnectedDevice]:
    """Expose registry for testing."""
    return _connected_devices


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _load_public_key(public_key_b64: str) -> Ed25519PublicKey:
    """Load an Ed25519 public key from base64."""
    raw = base64.b64decode(public_key_b64)
    from cryptography.hazmat.primitives.serialization import load_der_public_key
    if len(raw) == 32:
        # Raw 32-byte key — wrap in SubjectPublicKeyInfo
        from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey as _K
        return _K.from_public_bytes(raw)
    return load_der_public_key(raw)  # type: ignore[return-value]


def _verify_device_signature(
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


async def _set_device_status(
    db_session: AsyncSession,
    device: Device,
    status: str,
) -> None:
    """Update device status in the DB."""
    await db_session.execute(
        update(Device)
        .where(Device.id == device.id)
        .values(status=status)
    )
    await db_session.commit()


async def _notify_device_event(
    owner_user_id: UUID,
    event_type: str,
    device_id: UUID,
    device_name: str,
    **extra: Any,
) -> None:
    """Fire a time-series notification for the dashboard."""
    await notify_user(
        str(owner_user_id),
        f"build:device:{event_type}",
        mode=NotificationMode.TIMESERIES,
        push_notify=False,
        device_id=str(device_id),
        device_name=device_name,
        **extra,
    )


async def _record_heartbeat(
    db_session: AsyncSession,
    device: Device,
) -> None:
    """Record a heartbeat for a device."""
    now = datetime.now(timezone.utc)
    await db_session.execute(
        update(Device)
        .where(Device.id == device.id)
        .values(last_heartbeat_at=now)
    )
    await db_session.commit()


async def _record_missed_window(
    db_session: AsyncSession,
    device: Device,
    window_start: float,
    window_end: float,
) -> None:
    """Append a missed-heartbeat window to the device record."""
    result = await db_session.execute(
        select(Device).where(Device.id == device.id)
    )
    dev = result.scalar_one()
    windows = dev.get_missed_windows()
    windows.append({
        "start": datetime.fromtimestamp(window_start, tz=timezone.utc).isoformat(),
        "end": datetime.fromtimestamp(window_end, tz=timezone.utc).isoformat(),
    })
    # Keep only the most recent windows.
    if len(windows) > MAX_MISSED_WINDOWS:
        windows = windows[-MAX_MISSED_WINDOWS:]
    dev.set_missed_windows(windows)
    await db_session.commit()


# ---------------------------------------------------------------------------
# Background heartbeat monitor task
# ---------------------------------------------------------------------------
_monitor_task: asyncio.Task | None = None


async def _heartbeat_monitor_loop() -> None:
    """Periodically check connected devices for missed heartbeats."""
    while True:
        await asyncio.sleep(10)  # Check every 10 seconds
        now = time.time()
        for device_id, conn in list(_connected_devices.items()):
            timeout = conn.heartbeat_interval * HEARTBEAT_TIMEOUT_MULTIPLIER
            elapsed = now - conn.last_heartbeat
            if elapsed > timeout:
                logger.warning(
                    "Device %s missed heartbeat (%.1fs since last)",
                    device_id, elapsed,
                )
                await _notify_device_event(
                    conn.owner_user_id,
                    "heartbeat-missed",
                    device_id,
                    "",  # name not cached here; event consumers can look it up
                    elapsed_s=round(elapsed, 1),
                )


def start_heartbeat_monitor() -> None:
    """Start the background heartbeat monitor (idempotent)."""
    global _monitor_task
    if _monitor_task is None or _monitor_task.done():
        _monitor_task = asyncio.create_task(_heartbeat_monitor_loop())


def stop_heartbeat_monitor() -> None:
    """Stop the background heartbeat monitor."""
    global _monitor_task
    if _monitor_task and not _monitor_task.done():
        _monitor_task.cancel()
        _monitor_task = None


# ---------------------------------------------------------------------------
# Controller
# ---------------------------------------------------------------------------

class DeviceApiController(Controller):
    """Device management API."""

    path = "/api/devices"

    # ------------------------------------------------------------------
    # Registration flow (device-initiated, no user session required)
    # ------------------------------------------------------------------

    @post("/register", status_code=201)
    async def register_device(
        self,
        request: Request,
        data: dict[str, Any],
    ) -> Response:
        """Start the device registration flow.

        Called by the device CLI (no user session needed).
        Body: ``{"name": "my-laptop", "public_key": "<base64>"}``
        Returns: ``{"code": "...", "auth_url": "https://..."}``
        """
        _cleanup_expired_pending()

        name = data.get("name", "").strip()
        public_key_b64 = data.get("public_key", "").strip()
        if not name or not public_key_b64:
            return Response(
                content={"error": "name and public_key are required"},
                status_code=400,
            )

        # Validate the key format.
        try:
            _load_public_key(public_key_b64)
        except Exception:
            return Response(
                content={"error": "invalid Ed25519 public key"},
                status_code=400,
            )

        code = secrets.token_urlsafe(32)
        _pending_registrations[code] = PendingRegistration(
            code=code,
            name=name,
            public_key_b64=public_key_b64,
        )

        # Build the approval URL from the request's base URL.
        base = str(request.base_url).rstrip("/")
        auth_url = f"{base}/api/devices/approve/{code}"

        logger.info("Pending registration created: %s (code=%s)", name, code[:8])

        return Response(
            content={"code": code, "auth_url": auth_url},
            status_code=201,
        )

    @get("/approve/{code:str}", guards=[auth_guard, Permission("administrator")])
    async def approve_device_page(
        self,
        request: Request,
        code: str,
    ) -> Template:
        """Render the device approval page for the admin."""
        _cleanup_expired_pending()
        pending = _pending_registrations.get(code)
        if not pending:
            raise NotFoundException(detail="Registration not found or expired")

        return Template(
            "device_approve.html",
            context={
                "code": code,
                "device_name": pending.name,
                "public_key_preview": pending.public_key_b64[:16] + "...",
                "expires_in_s": max(0, int(PENDING_EXPIRY_S - (time.time() - pending.created_at))),
            },
        )

    @post("/approve/{code:str}", guards=[auth_guard, Permission("administrator")])
    async def approve_device_action(
        self,
        request: Request,
        db_session: AsyncSession,
        code: str,
    ) -> Response:
        """Approve a pending device registration.

        Creates the Device record and signals the waiting device client.
        """
        _cleanup_expired_pending()
        pending = _pending_registrations.get(code)
        if not pending:
            return Response(
                content={"error": "registration not found or expired"},
                status_code=404,
            )

        user_id = UUID(request.session["user_id"])

        # Check for duplicate name.
        existing = await db_session.execute(
            select(Device).where(
                Device.name == pending.name,
                Device.owner_user_id == user_id,
            )
        )
        if existing.scalar_one_or_none():
            return Response(
                content={"error": f"device '{pending.name}' already exists"},
                status_code=409,
            )

        device = Device(
            name=pending.name,
            public_key=pending.public_key_b64,
            owner_user_id=user_id,
            approved=True,
            status="offline",
        )
        db_session.add(device)
        await db_session.commit()
        await db_session.refresh(device)

        logger.info(
            "Device approved: %s (id=%s) for user %s via code %s",
            pending.name, device.id, user_id, code[:8],
        )

        # Signal the waiting device client via its SSE stream.
        await pending.result_queue.put({
            "type": "approved",
            "device_id": str(device.id),
            "device_name": device.name,
        })

        # Fire dashboard notification.
        await _notify_device_event(user_id, "authorized", device.id, pending.name)

        # Clean up the pending registration.
        _pending_registrations.pop(code, None)

        return Response(
            content={
                "id": str(device.id),
                "name": device.name,
                "status": device.status,
            },
            status_code=201,
        )

    @get("/pending/{code:str}/events")
    async def pending_device_events(
        self,
        request: Request,
        code: str,
    ) -> ServerSentEvent:
        """SSE stream for a pending device registration.

        The device connects here after calling ``POST /register`` and waits
        for the admin to approve. No user session is required — the pending
        code acts as the auth token.
        """
        pending = _pending_registrations.get(code)
        if not pending:
            raise NotFoundException(detail="Registration not found or expired")

        async def generate() -> AsyncGenerator[ServerSentEventMessage, None]:
            # Send an initial connected event.
            yield ServerSentEventMessage(
                data=json.dumps({"type": "waiting", "device_name": pending.name}),
                event="status",
            )

            while True:
                try:
                    # Wait for approval or keepalive every 15s.
                    result = await asyncio.wait_for(pending.result_queue.get(), timeout=15.0)
                    yield ServerSentEventMessage(
                        data=json.dumps(result),
                        event="notification",
                    )
                    return  # Done — device got its approval.
                except asyncio.TimeoutError:
                    # Check if expired.
                    if time.time() - pending.created_at > PENDING_EXPIRY_S:
                        yield ServerSentEventMessage(
                            data=json.dumps({"type": "expired"}),
                            event="notification",
                        )
                        _pending_registrations.pop(code, None)
                        return
                    # Keepalive.
                    yield ServerSentEventMessage(comment="keepalive")

        return ServerSentEvent(generate())

    @delete("/pending/{code:str}", status_code=200)
    async def dismiss_pending(self, code: str) -> Response:
        """Dismiss/clean up a pending registration after the device receives approval."""
        _pending_registrations.pop(code, None)
        return Response(content={"ok": True}, status_code=200)

    # ------------------------------------------------------------------
    # Direct authorization (admin-initiated, existing endpoint)
    # ------------------------------------------------------------------

    @post(
        "/authorize",
        guards=[auth_guard, Permission("administrator")],
        status_code=201,
    )
    async def authorize_device(
        self,
        request: Request,
        db_session: AsyncSession,
        data: dict[str, Any],
    ) -> Response:
        """Authorize a new device.

        Body: ``{"name": "my-laptop", "public_key": "<base64>"}``
        """
        name = data.get("name", "").strip()
        public_key_b64 = data.get("public_key", "").strip()
        if not name or not public_key_b64:
            return Response(
                content={"error": "name and public_key are required"},
                status_code=400,
            )

        # Validate the key is a real Ed25519 public key.
        try:
            _load_public_key(public_key_b64)
        except Exception:
            return Response(
                content={"error": "invalid Ed25519 public key"},
                status_code=400,
            )

        user_id = UUID(request.session["user_id"])

        # Check for duplicate name.
        existing = await db_session.execute(
            select(Device).where(
                Device.name == name,
                Device.owner_user_id == user_id,
            )
        )
        if existing.scalar_one_or_none():
            return Response(
                content={"error": f"device '{name}' already exists"},
                status_code=409,
            )

        device = Device(
            name=name,
            public_key=public_key_b64,
            owner_user_id=user_id,
            approved=True,
            status="offline",
        )
        db_session.add(device)
        await db_session.commit()
        await db_session.refresh(device)

        logger.info("Device authorized: %s (id=%s) for user %s", name, device.id, user_id)

        await _notify_device_event(
            user_id, "authorized", device.id, name,
        )

        return Response(
            content={
                "id": str(device.id),
                "name": device.name,
                "status": device.status,
            },
            status_code=201,
        )

    @get(
        "/",
        guards=[auth_guard, Permission("administrator")],
    )
    async def list_devices(
        self,
        request: Request,
        db_session: AsyncSession,
    ) -> list[dict[str, Any]]:
        """List all devices for the current admin user."""
        user_id = UUID(request.session["user_id"])
        result = await db_session.execute(
            select(Device)
            .where(Device.owner_user_id == user_id)
            .order_by(Device.created_at.desc())
        )
        devices = result.scalars().all()
        return [
            {
                "id": str(d.id),
                "name": d.name,
                "status": d.status,
                "approved": d.approved,
                "has_transport_key": d.transport_public_key is not None,
                "last_heartbeat_at": d.last_heartbeat_at.isoformat() if d.last_heartbeat_at else None,
                "heartbeat_interval_s": d.heartbeat_interval_s,
                "missed_heartbeat_windows": d.get_missed_windows(),
                "created_at": d.created_at.isoformat(),
            }
            for d in devices
        ]

    # ------------------------------------------------------------------
    # E2EE relay endpoints (browser → device)
    # ------------------------------------------------------------------

    @get(
        "/{device_id:uuid}/transport-key",
        guards=[auth_guard, Permission("administrator")],
    )
    async def get_transport_key(
        self,
        request: Request,
        db_session: AsyncSession,
        device_id: UUID,
    ) -> Response:
        """Return the X25519 transport public key for a device.

        The browser needs this to create a sealed-box session_init.
        """
        user_id = UUID(request.session["user_id"])
        result = await db_session.execute(
            select(Device).where(
                Device.id == device_id,
                Device.owner_user_id == user_id,
            )
        )
        device = result.scalar_one_or_none()
        if not device:
            raise NotFoundException(detail="Device not found")

        if not device.transport_public_key:
            return Response(
                content={"error": "device has no transport key — is it online?"},
                status_code=404,
            )

        return Response(
            content={
                "device_id": str(device.id),
                "transport_public_key": device.transport_public_key,
                "identity_public_key": device.public_key,
            },
            status_code=200,
        )

    @post(
        "/e2ee/session-init",
        guards=[auth_guard, Permission("administrator")],
    )
    async def session_init(
        self,
        request: Request,
        db_session: AsyncSession,
        data: dict[str, Any],
    ) -> Response:
        """Browser sends session_init to bootstrap an E2EE session.

        Body: ``{"device_id": "...", "session_id": "...", "session_init": {...}}``
        The relay registers the session and forwards to the device over WS.
        """
        device_id_str = data.get("device_id", "")
        session_id = data.get("session_id", "")
        session_init_payload = data.get("session_init")

        if not device_id_str or not session_id or not session_init_payload:
            return Response(
                content={"error": "device_id, session_id, and session_init required"},
                status_code=400,
            )

        try:
            device_id = UUID(device_id_str)
        except ValueError:
            return Response(content={"error": "invalid device_id"}, status_code=400)

        user_id = UUID(request.session["user_id"])

        # Verify device belongs to user and is online.
        result = await db_session.execute(
            select(Device).where(
                Device.id == device_id,
                Device.owner_user_id == user_id,
            )
        )
        device = result.scalar_one_or_none()
        if not device:
            return Response(content={"error": "device not found"}, status_code=404)

        conn = _connected_devices.get(device_id)
        if not conn:
            return Response(content={"error": "device is offline"}, status_code=503)

        # Register the session.
        _e2e_sessions[session_id] = E2ESession(
            session_id=session_id,
            device_id=device_id,
            owner_user_id=user_id,
        )

        # Forward session_init to device over WS.
        try:
            await conn.socket.send_json({
                "type": "session_init",
                "session_id": session_id,
                "session_init": session_init_payload,
            })
        except Exception:
            _e2e_sessions.pop(session_id, None)
            return Response(content={"error": "failed to reach device"}, status_code=503)

        return Response(content={"ok": True, "session_id": session_id}, status_code=200)

    @post(
        "/e2ee/send",
        guards=[auth_guard, Permission("administrator")],
    )
    async def e2ee_send(
        self,
        request: Request,
        data: dict[str, Any],
    ) -> Response:
        """Browser sends an encrypted envelope to a device via the relay.

        Body: ``{"session_id": "...", "envelope": {...}}``
        """
        session_id = data.get("session_id", "")
        envelope = data.get("envelope")

        if not session_id or not envelope:
            return Response(
                content={"error": "session_id and envelope required"},
                status_code=400,
            )

        user_id = UUID(request.session["user_id"])
        session = _e2e_sessions.get(session_id)
        if not session or session.owner_user_id != user_id:
            return Response(content={"error": "unknown session"}, status_code=404)

        conn = _connected_devices.get(session.device_id)
        if not conn:
            return Response(content={"error": "device is offline"}, status_code=503)

        try:
            await conn.socket.send_json({
                "type": "e2ee_envelope",
                "session_id": session_id,
                "envelope": envelope,
            })
        except Exception:
            return Response(content={"error": "failed to reach device"}, status_code=503)

        return Response(content={"ok": True}, status_code=200)

    # ------------------------------------------------------------------
    # WebSocket
    # ------------------------------------------------------------------

    @websocket("/ws")
    async def device_ws(self, socket: WebSocket) -> None:
        """WebSocket endpoint for device connections.

        Headers required:
        - ``X-Device-Id``: UUID of the device
        - ``X-Timestamp``: Unix timestamp string
        - ``X-Signature``: Base64 Ed25519 signature over ``{timestamp}.GET./api/devices/ws``
        """
        await socket.accept()

        # --- Authenticate ---
        device_id_str = socket.headers.get("x-device-id", "")
        timestamp_str = socket.headers.get("x-timestamp", "")
        signature_b64 = socket.headers.get("x-signature", "")

        if not device_id_str or not timestamp_str or not signature_b64:
            await socket.send_json({"type": "error", "error": "missing auth headers"})
            await socket.close(code=4001, reason="Missing auth headers")
            return

        try:
            device_id = UUID(device_id_str)
        except ValueError:
            await socket.send_json({"type": "error", "error": "invalid device id"})
            await socket.close(code=4001, reason="Invalid device id")
            return

        # Look up device in DB.
        session_maker = socket.app.state.session_maker_class
        async with session_maker() as db_session:
            result = await db_session.execute(
                select(Device).where(Device.id == device_id, Device.approved.is_(True))
            )
            device = result.scalar_one_or_none()

            if not device:
                await socket.send_json({"type": "error", "error": "device not found or not approved"})
                await socket.close(code=4001, reason="Device not found")
                return

            # Verify signature.
            try:
                pub_key = _load_public_key(device.public_key)
            except Exception:
                await socket.send_json({"type": "error", "error": "invalid stored public key"})
                await socket.close(code=4001, reason="Invalid public key")
                return

            if not _verify_device_signature(pub_key, timestamp_str, signature_b64):
                await socket.send_json({"type": "error", "error": "invalid signature"})
                await socket.close(code=4001, reason="Invalid signature")
                return

            # Check timestamp freshness (5 min window).
            try:
                ts = float(timestamp_str)
                if abs(time.time() - ts) > 300:
                    await socket.send_json({"type": "error", "error": "timestamp expired"})
                    await socket.close(code=4001, reason="Timestamp expired")
                    return
            except ValueError:
                await socket.send_json({"type": "error", "error": "invalid timestamp"})
                await socket.close(code=4001, reason="Invalid timestamp")
                return

            # --- Authenticated! ---
            owner_user_id = device.owner_user_id
            device_name = device.name
            heartbeat_interval = device.heartbeat_interval_s

            # Set online.
            await _set_device_status(db_session, device, "online")
            await _record_heartbeat(db_session, device)

        # Register in connected-devices registry.
        conn = ConnectedDevice(
            device_id=device_id,
            owner_user_id=owner_user_id,
            socket=socket,
            last_heartbeat=time.time(),
            heartbeat_interval=heartbeat_interval,
        )
        _connected_devices[device_id] = conn

        await _notify_device_event(owner_user_id, "online", device_id, device_name)

        await socket.send_json({
            "type": "authenticated",
            "device_id": str(device_id),
            "heartbeat_interval_s": heartbeat_interval,
        })

        # Start heartbeat monitor if not running.
        start_heartbeat_monitor()

        # --- Main message loop ---
        try:
            while True:
                raw = await socket.receive_text()
                try:
                    msg = json.loads(raw)
                except json.JSONDecodeError:
                    await socket.send_json({"type": "error", "error": "invalid JSON"})
                    continue

                msg_type = msg.get("type")
                rid = msg.get("rid")

                if msg_type == "heartbeat":
                    conn.last_heartbeat = time.time()
                    async with session_maker() as db_session:
                        result = await db_session.execute(
                            select(Device).where(Device.id == device_id)
                        )
                        dev = result.scalar_one_or_none()
                        if dev:
                            await _record_heartbeat(db_session, dev)

                    if rid:
                        await socket.send_json({"type": "response", "rid": rid, "ok": True})

                elif msg_type == "transport_key":
                    # Device uploading its X25519 transport public key.
                    transport_key_b64 = msg.get("transport_public_key", "").strip()
                    if not transport_key_b64:
                        if rid:
                            await socket.send_json({
                                "type": "response", "rid": rid,
                                "ok": False, "error": "transport_public_key required",
                            })
                        continue

                    # Validate it's plausible base64 (32 bytes decoded).
                    try:
                        raw = base64.b64decode(transport_key_b64)
                        if len(raw) != 32:
                            raise ValueError("must be 32 bytes")
                    except Exception:
                        if rid:
                            await socket.send_json({
                                "type": "response", "rid": rid,
                                "ok": False, "error": "invalid X25519 public key",
                            })
                        continue

                    async with session_maker() as db_session:
                        await db_session.execute(
                            update(Device)
                            .where(Device.id == device_id)
                            .values(transport_public_key=transport_key_b64)
                        )
                        await db_session.commit()

                    logger.info("Device %s uploaded transport key", device_id)
                    if rid:
                        await socket.send_json({"type": "response", "rid": rid, "ok": True})

                elif msg_type == "session_accept":
                    # Device sending session_accept back to a browser session.
                    session_id = msg.get("session_id")
                    envelope = msg.get("envelope")
                    if not session_id or not envelope:
                        if rid:
                            await socket.send_json({
                                "type": "response", "rid": rid,
                                "ok": False, "error": "session_id and envelope required",
                            })
                        continue

                    session = _e2e_sessions.get(session_id)
                    if not session or session.device_id != device_id:
                        if rid:
                            await socket.send_json({
                                "type": "response", "rid": rid,
                                "ok": False, "error": "unknown session",
                            })
                        continue

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

                elif msg_type == "e2ee_envelope":
                    # Device sending an encrypted envelope to a browser session.
                    session_id = msg.get("session_id")
                    envelope = msg.get("envelope")
                    if not session_id or not envelope:
                        if rid:
                            await socket.send_json({
                                "type": "response", "rid": rid,
                                "ok": False, "error": "session_id and envelope required",
                            })
                        continue

                    session = _e2e_sessions.get(session_id)
                    if not session or session.device_id != device_id:
                        if rid:
                            await socket.send_json({
                                "type": "response", "rid": rid,
                                "ok": False, "error": "unknown session",
                            })
                        continue

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

                elif msg_type == "status":
                    # Device reporting status update (agents, tasks, etc.)
                    async with session_maker() as db_session:
                        await _notify_device_event(
                            owner_user_id, "status",
                            device_id, device_name,
                            **{k: v for k, v in msg.items() if k not in ("type", "rid")},
                        )
                    if rid:
                        await socket.send_json({"type": "response", "rid": rid, "ok": True})

                else:
                    if rid:
                        await socket.send_json({
                            "type": "response", "rid": rid,
                            "ok": False, "error": f"unknown message type: {msg_type}",
                        })

        except Exception:
            # Client disconnected or error.
            pass
        finally:
            # --- Cleanup ---
            _connected_devices.pop(device_id, None)

            # Clean up any E2EE sessions for this device.
            stale_sessions = [
                sid for sid, s in _e2e_sessions.items()
                if s.device_id == device_id
            ]
            for sid in stale_sessions:
                _e2e_sessions.pop(sid, None)

            # Record missed window if last heartbeat was stale.
            now = time.time()
            if now - conn.last_heartbeat > heartbeat_interval * HEARTBEAT_TIMEOUT_MULTIPLIER:
                async with session_maker() as db_session:
                    result = await db_session.execute(
                        select(Device).where(Device.id == device_id)
                    )
                    dev = result.scalar_one_or_none()
                    if dev:
                        await _record_missed_window(
                            db_session, dev, conn.last_heartbeat, now,
                        )

            # Set offline.
            async with session_maker() as db_session:
                await db_session.execute(
                    update(Device)
                    .where(Device.id == device_id)
                    .values(status="offline")
                )
                await db_session.commit()

            await _notify_device_event(
                owner_user_id, "offline", device_id, device_name,
            )
            logger.info("Device %s (%s) disconnected", device_name, device_id)
