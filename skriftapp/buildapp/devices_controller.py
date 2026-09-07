"""Device registration, approval, and the browser→relay gateway-token handshake.

Three audiences, three guard styles:
- **public** (bridge-facing): ``/api/devices/register`` + ``/status`` — no session, but
  registration requires an Ed25519 signature proving key possession.
- **authenticated** (browser session/desktop OAuth): ``/api/devices/...`` and
  ``/api/gateway-token`` — guarded by ``build_auth_guard``.
- **internal** (relay-facing): ``/internal/...`` — the relay reads device keys/approval
  and validates gateway tokens. Guarded by ``internal_auth_guard`` (``X-Internal-Secret``
  shared secret; dev config may allow localhost callers instead).
"""

from __future__ import annotations

from datetime import timedelta
from uuid import UUID

from litestar import Controller, Request, get, post
from litestar.exceptions import (
    ClientException,
    HTTPException,
    NotAuthorizedException,
    NotFoundException,
)
from litestar.response import Response
from litestar.status_codes import HTTP_409_CONFLICT
from sqlalchemy import delete, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import ephemeral_tokens, pairing_crypto
from buildapp.clock import utc_now
from buildapp.desktop_auth import build_auth_guard
from buildapp.internal_auth import internal_auth_guard
from buildapp.models import Device
from buildapp.request_body import read_json_object
from buildapp.session_auth import require_user

# Pending registrations that are never approved get cleaned up after this long.
PENDING_TTL = timedelta(minutes=15)
# Gateway tokens are short-lived; the SPA re-mints on (re)connect.
GATEWAY_TOKEN_TTL = timedelta(minutes=5)
GATEWAY_TOKEN_PURPOSE = "gateway"
GATEWAY_TOKEN_PREFIX = "gw_"


def device_summary(device: Device) -> dict:
    """Serialize a device for the browser-facing listing.

    Carries the browser<->relay contract fields (``device_id``, ``approved``,
    ``status``, ``transport_public_key_b64`` — the SPA seals session keys to the
    transport key) plus the pre-contract SPA fields (``id``, ``name``,
    ``fingerprint``, ``last_seen_at``).
    """
    return {
        "device_id": str(device.id),
        "id": str(device.id),
        "name": device.name,
        "fingerprint": pairing_crypto.fingerprint(device.identity_public_key_b64),
        "approved": device.approved,
        "status": device.status,
        "transport_public_key_b64": device.transport_public_key_b64,
        "last_seen_at": device.last_seen_at.isoformat() if device.last_seen_at else None,
    }


#: How many approved devices one account may hold. Enforced at approval — the
#: one moment a device becomes an account's — so revoking frees a slot.
MAX_DEVICES_PER_USER = 3


class DevicesController(Controller):
    """Device pairing + gateway-token endpoints. Per-route guards (no class-level guard)
    so public/internal routes sit alongside authenticated ones."""

    path = ""

    # ----- public (bridge-facing) --------------------------------------------

    @post("/api/devices/register")
    async def register(self, request: Request, db_session: AsyncSession) -> Response:
        """A bridge self-registers as *pending*. Verifies the Ed25519 signature over the
        registration challenge (proof of key possession) before storing anything."""
        body = await read_json_object(request)
        try:
            device_id = UUID(str(body["device_id"]))
            name = str(body["name"]).strip() or "device"
            id_pub = str(body["identity_public_key_b64"])
            tp_pub = str(body["transport_public_key_b64"])
            code_hash = str(body["pairing_code_hash"])
            signature = str(body["signature_b64"])
        except (KeyError, ValueError, TypeError):
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
                Device.created_at < utc_now() - PENDING_TTL,
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

    @post("/api/devices/lookup", guards=[build_auth_guard])
    async def lookup(self, request: Request, db_session: AsyncSession) -> Response:
        """Resolve a pairing code to a pending device so the human can compare its
        fingerprint before approving."""
        require_user(request)
        body = await read_json_object(request)
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

    @post("/api/devices/approve", guards=[build_auth_guard])
    async def approve(self, request: Request, db_session: AsyncSession) -> Response:
        """Bind a pending device (located by its pairing code) to the current user.

        An account holds at most ``MAX_DEVICES_PER_USER`` approved devices: the
        cap is checked here, the one moment a device becomes an account's, so a
        revoked device frees its slot and a pending one waits for it."""
        user_id = require_user(request)
        body = await read_json_object(request)
        code = str(body.get("code", "")).strip()
        if not code:
            raise ClientException("code required")
        device = await self._pending_by_code(db_session, code)
        if device is None:
            raise NotFoundException("no pending device for that code")
        owned = await db_session.scalar(
            select(func.count())
            .select_from(Device)
            .where(Device.owner_user_id == user_id, Device.approved.is_(True))
        )
        if (owned or 0) >= MAX_DEVICES_PER_USER:
            raise HTTPException(
                status_code=HTTP_409_CONFLICT,
                detail=(
                    f"this account already has {MAX_DEVICES_PER_USER} devices — "
                    "revoke one in Settings → Devices to add another"
                ),
            )
        device.owner_user_id = user_id
        device.approved = True
        device.pairing_code_hash = None
        await db_session.commit()
        return Response({"device_id": str(device.id), "approved": True})

    @get("/api/devices", guards=[build_auth_guard])
    async def list_devices(self, request: Request, db_session: AsyncSession) -> Response:
        """List the current user's approved devices."""
        user_id = require_user(request)
        rows = (
            await db_session.execute(
                select(Device)
                .where(Device.owner_user_id == user_id, Device.approved.is_(True))
                .order_by(Device.created_at.desc())
            )
        ).scalars().all()
        return Response({"devices": [device_summary(d) for d in rows]})

    @post("/api/devices/{device_id:uuid}/revoke", guards=[build_auth_guard])
    async def revoke(
        self, device_id: UUID, request: Request, db_session: AsyncSession
    ) -> Response:
        """Revoke an owned device — it can no longer authenticate to the relay."""
        user_id = require_user(request)
        device = await db_session.get(Device, device_id)
        if device is None or device.owner_user_id != user_id:
            raise NotFoundException("device not found")
        device.approved = False
        device.status = "offline"
        await db_session.commit()
        return Response({"device_id": str(device_id), "approved": False})

    @post("/api/gateway-token", guards=[build_auth_guard])
    async def gateway_token(self, request: Request, db_session: AsyncSession) -> Response:
        """Mint a short-TTL token the browser presents to the relay so it can be scoped
        to this user's devices."""
        raw = await ephemeral_tokens.mint(
            db_session,
            purpose=GATEWAY_TOKEN_PURPOSE,
            prefix=GATEWAY_TOKEN_PREFIX,
            user_id=require_user(request),
            ttl=GATEWAY_TOKEN_TTL,
            now=utc_now(),
        )
        return Response({"token": raw, "expires_in_s": int(GATEWAY_TOKEN_TTL.total_seconds())})

    # ----- internal (relay-facing, shared-secret guarded) ---------------------

    @get("/internal/devices/{device_id:uuid}", guards=[internal_auth_guard])
    async def internal_device(
        self, device_id: UUID, db_session: AsyncSession
    ) -> Response:
        """The relay reads a device's pinned key + approval to authenticate /ws/device."""
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

    @get("/internal/gateway-token/{token:str}", guards=[internal_auth_guard])
    async def internal_gateway_token(
        self, token: str, db_session: AsyncSession
    ) -> Response:
        """The relay validates a browser's gateway token → the owning user id."""
        user_id = await ephemeral_tokens.holder_of(
            db_session, purpose=GATEWAY_TOKEN_PURPOSE, raw=token, now=utc_now()
        )
        if user_id is None:
            raise NotFoundException()
        return Response({"user_id": str(user_id)})

    @post("/internal/devices/{device_id:uuid}/status", guards=[internal_auth_guard])
    async def internal_set_status(
        self, device_id: UUID, request: Request, db_session: AsyncSession
    ) -> Response:
        """The relay reports a device online/offline so the SPA can show a status dot."""
        body = await read_json_object(request)
        online = bool(body.get("online", False))
        device = await db_session.get(Device, device_id)
        if device is None:
            raise NotFoundException()
        device.status = "online" if online else "offline"
        if online:
            device.last_seen_at = utc_now()
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
                    Device.created_at >= utc_now() - PENDING_TTL,
                )
            )
        ).scalar_one_or_none()
