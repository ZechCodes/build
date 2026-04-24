"""Device API controller — thin routing layer over extracted modules."""

from __future__ import annotations

import asyncio
import json
import logging
import time
from collections.abc import AsyncGenerator
from datetime import datetime, timezone
from typing import Any
from uuid import UUID

# Heartbeat freshness multiplier — matches the relay's monitor threshold.
# A device is reported "offline" if its last heartbeat is older than
# heartbeat_interval_s * _HEARTBEAT_TIMEOUT_FACTOR. Keeps the list
# endpoint's truth aligned with the relay's internal watchdog so the
# browser doesn't see "online" for a silently-disconnected device.
_HEARTBEAT_TIMEOUT_FACTOR = 2.5


def _effective_device_status(device: "Device", now: datetime) -> str:
    """Return the device's effective status, downgrading to "offline"
    when its heartbeat is stale."""
    if device.status != "online":
        return device.status
    if device.last_heartbeat_at is None:
        # Marked online with no heartbeat timestamp yet — could be the
        # connect-window edge case. Trust the DB.
        return device.status
    interval = device.heartbeat_interval_s or 30
    elapsed = (now - device.last_heartbeat_at).total_seconds()
    if elapsed > interval * _HEARTBEAT_TIMEOUT_FACTOR:
        return "offline"
    return "online"

from litestar import Controller, Request, get, patch, post, delete
from litestar.exceptions import NotFoundException
from litestar.response import Response, Template
from litestar.response.sse import ServerSentEvent, ServerSentEventMessage
from sqlalchemy import select, update
from sqlalchemy.ext.asyncio import AsyncSession

from skrift.auth.guards import auth_guard, Permission

from build_app.models import Device
from build_app.devices._state import (
    E2ESession,
    PendingRegistration,
    _e2e_sessions,
    _pending_registrations,
    PENDING_EXPIRY_S,
    cleanup_expired_pending,
)
from build_app.devices._crypto import load_public_key
from build_app.devices._helpers import (
    create_device_record,
    notify_device_event,
    set_device_status,
)
from build_app.devices._relay import (
    send_to_device,
    disconnect_device,
    is_device_connected,
)

logger = logging.getLogger(__name__)


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
        import secrets

        cleanup_expired_pending()

        name = data.get("name", "").strip()
        public_key_b64 = data.get("public_key", "").strip()
        if not name or not public_key_b64:
            return Response(
                content={"error": "name and public_key are required"},
                status_code=400,
            )

        # Validate the key format.
        try:
            load_public_key(public_key_b64)
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
        cleanup_expired_pending()
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
        """Approve a pending device registration."""
        cleanup_expired_pending()
        pending = _pending_registrations.get(code)
        if not pending:
            return Response(
                content={"error": "registration not found or expired"},
                status_code=404,
            )

        user_id = UUID(request.session["user_id"])

        device = await create_device_record(
            db_session, pending.name, pending.public_key_b64, user_id,
        )
        if not device:
            return Response(
                content={"error": f"device '{pending.name}' already exists"},
                status_code=409,
            )

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

        await notify_device_event(user_id, "authorized", device.id, pending.name)

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
        """SSE stream for a pending device registration."""
        pending = _pending_registrations.get(code)
        if not pending:
            raise NotFoundException(detail="Registration not found or expired")

        async def generate() -> AsyncGenerator[ServerSentEventMessage, None]:
            yield ServerSentEventMessage(
                data=json.dumps({"type": "waiting", "device_name": pending.name}),
                event="status",
            )

            while True:
                try:
                    result = await asyncio.wait_for(pending.result_queue.get(), timeout=15.0)
                    yield ServerSentEventMessage(
                        data=json.dumps(result),
                        event="notification",
                    )
                    return
                except asyncio.TimeoutError:
                    if time.time() - pending.created_at > PENDING_EXPIRY_S:
                        yield ServerSentEventMessage(
                            data=json.dumps({"type": "expired"}),
                            event="notification",
                        )
                        _pending_registrations.pop(code, None)
                        return
                    yield ServerSentEventMessage(comment="keepalive")

        return ServerSentEvent(generate())

    @delete("/pending/{code:str}", status_code=200)
    async def dismiss_pending(self, code: str) -> Response:
        """Dismiss/clean up a pending registration after the device receives approval."""
        _pending_registrations.pop(code, None)
        return Response(content={"ok": True}, status_code=200)

    # ------------------------------------------------------------------
    # Direct authorization (admin-initiated)
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

        try:
            load_public_key(public_key_b64)
        except Exception:
            return Response(
                content={"error": "invalid Ed25519 public key"},
                status_code=400,
            )

        user_id = UUID(request.session["user_id"])

        device = await create_device_record(db_session, name, public_key_b64, user_id)
        if not device:
            return Response(
                content={"error": f"device '{name}' already exists"},
                status_code=409,
            )

        logger.info("Device authorized: %s (id=%s) for user %s", name, device.id, user_id)

        await notify_device_event(user_id, "authorized", device.id, name)

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
        now = datetime.now(timezone.utc)
        return [
            {
                "id": str(d.id),
                "name": d.name,
                "status": _effective_device_status(d, now),
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
    # Device management (rename, restart, revoke)
    # ------------------------------------------------------------------

    @patch(
        "/{device_id:uuid}",
        guards=[auth_guard, Permission("administrator")],
    )
    async def update_device(
        self,
        request: Request,
        db_session: AsyncSession,
        device_id: UUID,
        data: dict[str, Any],
    ) -> Response:
        """Rename a device."""
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

        name = data.get("name", "").strip()
        if not name:
            return Response(content={"error": "name required"}, status_code=400)

        await db_session.execute(
            update(Device)
            .where(Device.id == device_id)
            .values(name=name)
        )
        await db_session.commit()

        await notify_device_event(
            user_id, "renamed", device_id, name,
        )

        return Response(content={"ok": True, "name": name})

    @post(
        "/{device_id:uuid}/restart",
        guards=[auth_guard, Permission("administrator")],
    )
    async def restart_device(
        self,
        request: Request,
        db_session: AsyncSession,
        device_id: UUID,
    ) -> Response:
        """Send a restart command to a connected device."""
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

        try:
            await send_to_device(device_id, {"type": "restart"})
        except Exception as exc:
            logger.error("Failed to send restart to device %s: %s", device_id, exc)
            return Response(
                content={"error": "Failed to send restart command"},
                status_code=502,
            )

        return Response(content={"ok": True})

    @delete(
        "/{device_id:uuid}",
        guards=[auth_guard, Permission("administrator")],
        status_code=200,
    )
    async def revoke_device(
        self,
        request: Request,
        db_session: AsyncSession,
        device_id: UUID,
    ) -> Response:
        """Revoke a device — disconnect, clean up sessions, delete record."""
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

        device_name = device.name

        # Disconnect device via relay.
        try:
            await disconnect_device(device_id)
        except Exception:
            pass

        # Clean up E2EE sessions for this device.
        stale_sessions = [
            sid for sid, s in _e2e_sessions.items()
            if s.device_id == device_id
        ]
        for sid in stale_sessions:
            _e2e_sessions.pop(sid, None)

        # Delete from DB.
        await db_session.delete(device)
        await db_session.commit()

        await notify_device_event(
            user_id, "revoked", device_id, device_name,
        )

        return Response(content={"ok": True})

    # ------------------------------------------------------------------
    # E2EE relay endpoints (browser -> device)
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
        """Return the X25519 transport public key for a device."""
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
        """Browser sends session_init to bootstrap an E2EE session."""
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

        # Register the session.
        _e2e_sessions[session_id] = E2ESession(
            session_id=session_id,
            device_id=device_id,
            owner_user_id=user_id,
        )

        # Forward session_init to device via relay.
        try:
            await send_to_device(device_id, {
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
        """Browser sends an encrypted envelope to a device via the relay."""
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

        # Check session expiry.
        if session.expired:
            _e2e_sessions.pop(session_id, None)
            return Response(content={"error": "session expired"}, status_code=410)

        try:
            await send_to_device(session.device_id, {
                "type": "e2ee_envelope",
                "session_id": session_id,
                "envelope": envelope,
            })
        except Exception:
            return Response(content={"error": "failed to reach device"}, status_code=503)

        session.touch()
        return Response(content={"ok": True}, status_code=200)

    # Note: Device WebSocket endpoint has been moved to the build-relay service.
    # Devices now connect to the relay at wss://relay.getbuild.ing/ws/device.
    # Communication happens via Redis streams (relay:device-events, relay:device-commands).
