"""Device registration, approval, and the browser→relay gateway-token handshake.

Three audiences, three guard styles:
- **public** (bridge-facing): ``/api/devices/register`` + ``/status`` — no session, but
  registration requires an Ed25519 signature proving key possession.
- **authenticated** (browser/session): ``/api/devices/...`` lookup/approve/list/revoke +
  ``/api/gateway-token`` — guarded by ``auth_guard``.
- **internal** (relay-facing, localhost-only): ``/internal/...`` — the relay reads device
  keys/approval and validates gateway tokens.
"""

from __future__ import annotations

import hashlib
import secrets
from datetime import datetime, timedelta, timezone
from uuid import UUID

from litestar import Controller, Request, get, post
from litestar.exceptions import (
    ClientException,
    NotAuthorizedException,
    NotFoundException,
)
from litestar.response import Response
from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import AsyncSession

from skrift.auth.guards import auth_guard
from skrift.auth.session_keys import SESSION_USER_ID
from skrift.lib.client_ip import get_client_ip

from buildapp import pairing_crypto
from buildapp.models import Device, EphemeralToken

# Pending registrations that are never approved get cleaned up after this long.
PENDING_TTL = timedelta(minutes=15)
# Gateway tokens are short-lived; the SPA re-mints on (re)connect.
GATEWAY_TOKEN_TTL = timedelta(minutes=5)


def _now() -> datetime:
    return datetime.now(tz=timezone.utc)


def _require_user(request: Request) -> UUID:
    user_id = request.session.get(SESSION_USER_ID)
    if not user_id:
        raise NotAuthorizedException("Authentication required")
    return UUID(user_id)


def _require_localhost(request: Request) -> None:
    if get_client_ip(request.scope) not in ("127.0.0.1", "::1"):
        raise NotFoundException()  # don't reveal the internal route exists


def _token_hash(raw: str) -> str:
    return hashlib.sha256(raw.encode("utf-8")).hexdigest()


class DevicesController(Controller):
    """Device pairing + gateway-token endpoints. Per-route guards (no class-level guard)
    so public/internal routes sit alongside authenticated ones."""

    path = ""

    # ----- public (bridge-facing) --------------------------------------------

    @post("/api/devices/register")
    async def register(self, request: Request, db_session: AsyncSession) -> Response:
        """A bridge self-registers as *pending*. Verifies the Ed25519 signature over the
        registration challenge (proof of key possession) before storing anything."""
        body = await request.json()
        try:
            device_id = UUID(str(body["device_id"]))
            name = str(body["name"]).strip() or "device"
            id_pub = str(body["identity_public_key_b64"])
            tp_pub = str(body["transport_public_key_b64"])
            code_hash = str(body["pairing_code_hash"])
            signature = str(body["signature_b64"])
        except (KeyError, ValueError):
            raise ClientException("malformed registration")

        challenge = pairing_crypto.registration_challenge(
            str(device_id), id_pub, tp_pub, code_hash
        )
        if not pairing_crypto.verify_registration(id_pub, challenge, signature):
            raise NotAuthorizedException("registration signature invalid")

        # Opportunistically purge stale pending registrations.
        await db_session.execute(
            delete(Device).where(
                Device.approved.is_(False),
                Device.created_at < _now() - PENDING_TTL,
            )
        )

        existing = await db_session.get(Device, device_id)
        if existing is not None and existing.approved:
            raise ClientException("device already paired", status_code=409)
        if existing is None:
            db_session.add(
                Device(
                    id=device_id,
                    name=name,
                    identity_public_key_b64=id_pub,
                    transport_public_key_b64=tp_pub,
                    pairing_code_hash=code_hash,
                    approved=False,
                    status="pending",
                )
            )
        else:
            # Re-registration before approval: refresh keys/name/code.
            existing.name = name
            existing.identity_public_key_b64 = id_pub
            existing.transport_public_key_b64 = tp_pub
            existing.pairing_code_hash = code_hash
        await db_session.commit()
        return Response({"device_id": str(device_id), "status": "pending"}, status_code=201)

    @get("/api/devices/{device_id:uuid}/status")
    async def status(self, device_id: UUID, db_session: AsyncSession) -> Response:
        """Bridge polls this until approved. The device_id is an unguessable UUID."""
        device = await db_session.get(Device, device_id)
        if device is None:
            return Response({"approved": False, "owner_user_id": None})
        return Response(
            {
                "approved": device.approved,
                "owner_user_id": str(device.owner_user_id) if device.owner_user_id else None,
            }
        )

    # ----- authenticated (browser/session) -----------------------------------

    @post("/api/devices/lookup", guards=[auth_guard])
    async def lookup(self, request: Request, db_session: AsyncSession) -> Response:
        """Resolve a pairing code to a pending device so the human can compare its
        fingerprint before approving."""
        _require_user(request)
        body = await request.json()
        code = str(body.get("code", "")).strip()
        if not code:
            raise ClientException("code required")
        device = await self._pending_by_code(db_session, code)
        if device is None:
            raise NotFoundException("no pending device for that code")
        return Response(
            {
                "device_id": str(device.id),
                "name": device.name,
                "fingerprint": pairing_crypto.fingerprint(device.identity_public_key_b64),
                "identity_public_key_b64": device.identity_public_key_b64,
            }
        )

    @post("/api/devices/approve", guards=[auth_guard])
    async def approve(self, request: Request, db_session: AsyncSession) -> Response:
        """Bind a pending device (located by its pairing code) to the current user."""
        user_id = _require_user(request)
        body = await request.json()
        code = str(body.get("code", "")).strip()
        if not code:
            raise ClientException("code required")
        device = await self._pending_by_code(db_session, code)
        if device is None:
            raise NotFoundException("no pending device for that code")
        device.owner_user_id = user_id
        device.approved = True
        device.pairing_code_hash = None
        await db_session.commit()
        return Response({"device_id": str(device.id), "approved": True})

    @get("/api/devices", guards=[auth_guard])
    async def list_devices(self, request: Request, db_session: AsyncSession) -> Response:
        """List the current user's approved devices."""
        user_id = _require_user(request)
        rows = (
            await db_session.execute(
                select(Device)
                .where(Device.owner_user_id == user_id, Device.approved.is_(True))
                .order_by(Device.created_at.desc())
            )
        ).scalars().all()
        return Response(
            {
                "devices": [
                    {
                        "id": str(d.id),
                        "name": d.name,
                        "fingerprint": pairing_crypto.fingerprint(d.identity_public_key_b64),
                        "status": d.status,
                        "last_seen_at": d.last_seen_at.isoformat() if d.last_seen_at else None,
                    }
                    for d in rows
                ]
            }
        )

    @post("/api/devices/{device_id:uuid}/revoke", guards=[auth_guard])
    async def revoke(
        self, device_id: UUID, request: Request, db_session: AsyncSession
    ) -> Response:
        """Revoke an owned device — it can no longer authenticate to the relay."""
        user_id = _require_user(request)
        device = await db_session.get(Device, device_id)
        if device is None or device.owner_user_id != user_id:
            raise NotFoundException("device not found")
        device.approved = False
        device.status = "offline"
        await db_session.commit()
        return Response({"device_id": str(device_id), "approved": False})

    @post("/api/gateway-token", guards=[auth_guard])
    async def gateway_token(self, request: Request, db_session: AsyncSession) -> Response:
        """Mint a short-TTL token the browser presents to the relay so it can be scoped
        to this user's devices."""
        user_id = _require_user(request)
        raw = "gw_" + secrets.token_urlsafe(24)
        db_session.add(
            EphemeralToken(
                token_hash=_token_hash(raw),
                purpose="gateway",
                user_id=user_id,
                expires_at=_now() + GATEWAY_TOKEN_TTL,
            )
        )
        await db_session.commit()
        return Response({"token": raw, "expires_in_s": int(GATEWAY_TOKEN_TTL.total_seconds())})

    # ----- internal (relay-facing, localhost-only) ---------------------------

    @get("/internal/devices/{device_id:uuid}")
    async def internal_device(
        self, device_id: UUID, request: Request, db_session: AsyncSession
    ) -> Response:
        """The relay reads a device's pinned key + approval to authenticate /ws/device."""
        _require_localhost(request)
        device = await db_session.get(Device, device_id)
        if device is None:
            raise NotFoundException()
        return Response(
            {
                "device_id": str(device.id),
                "name": device.name,
                "identity_public_key_b64": device.identity_public_key_b64,
                "transport_public_key_b64": device.transport_public_key_b64,
                "approved": device.approved,
                "owner_user_id": str(device.owner_user_id) if device.owner_user_id else None,
            }
        )

    @get("/internal/gateway-token/{token:str}")
    async def internal_gateway_token(
        self, token: str, request: Request, db_session: AsyncSession
    ) -> Response:
        """The relay validates a browser's gateway token → the owning user id."""
        _require_localhost(request)
        row = (
            await db_session.execute(
                select(EphemeralToken).where(
                    EphemeralToken.token_hash == _token_hash(token),
                    EphemeralToken.purpose == "gateway",
                    EphemeralToken.expires_at > _now(),
                )
            )
        ).scalar_one_or_none()
        if row is None:
            raise NotFoundException()
        return Response({"user_id": str(row.user_id)})

    @post("/internal/devices/{device_id:uuid}/status")
    async def internal_set_status(
        self, device_id: UUID, request: Request, db_session: AsyncSession
    ) -> Response:
        """The relay reports a device online/offline so the SPA can show a status dot."""
        _require_localhost(request)
        body = await request.json()
        online = bool(body.get("online", False))
        device = await db_session.get(Device, device_id)
        if device is None:
            raise NotFoundException()
        device.status = "online" if online else "offline"
        if online:
            device.last_seen_at = _now()
        await db_session.commit()
        return Response({"ok": True})

    # ----- helpers -----------------------------------------------------------

    async def _pending_by_code(self, db_session: AsyncSession, code: str) -> Device | None:
        code_hash = pairing_crypto.hash_code(code)
        return (
            await db_session.execute(
                select(Device).where(
                    Device.pairing_code_hash == code_hash,
                    Device.approved.is_(False),
                    Device.created_at >= _now() - PENDING_TTL,
                )
            )
        ).scalar_one_or_none()
