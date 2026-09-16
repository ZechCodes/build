"""Device registration, approval, and the browser→relay gateway-token handshake.

Three audiences, three guard styles:
- **public** (bridge-facing): ``/api/devices/register`` + ``/status`` — no session, but
  registration requires an Ed25519 signature proving key possession.
- **authenticated** (browser session/desktop OAuth): ``/api/devices/...`` and
  ``/api/gateway-token`` — guarded by ``build_auth_guard``.
- **internal** (relay-facing): ``/internal/...`` — the relay reads device keys/approval
  and validates gateway tokens. Reads only: presence is this api's own, derived from
  the heartbeat each bridge posts. Guarded by ``internal_auth_guard`` (``X-Internal-Secret``
  shared secret; dev config may allow localhost callers instead).
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta
from uuid import UUID

from litestar import Controller, Request, get, post
from litestar.exceptions import (
    ClientException,
    HTTPException,
    NotAuthorizedException,
    NotFoundException,
)
from litestar.response import Response
from litestar.status_codes import HTTP_200_OK, HTTP_409_CONFLICT
from sqlalchemy import delete, func, select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import ephemeral_tokens, pairing_crypto, presence
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
#: Where a bridge posts its signed liveness beat (``planning/v2/Strict P2P
#: Transport Spec.md`` rule 6). Public: the signature is the authentication.
HEARTBEAT_ROUTE_PATH = "/api/devices/heartbeat"

_replay_guard = presence.replay_guard()


def reset_replay_guard_for_tests() -> None:
    """A fresh guard per test app: the guard is process state, as in production."""
    global _replay_guard
    _replay_guard = presence.replay_guard()


@dataclass(frozen=True)
class Heartbeat:
    """One well-formed heartbeat, read out of the body."""

    device_id: UUID
    timestamp: int
    signature: str


def read_heartbeat(body: dict) -> Heartbeat:
    """Everything a malformed body can be refused for, in one place — so the
    handler below only authorizes and stamps."""
    try:
        return Heartbeat(
            device_id=UUID(str(body["device_id"])),
            timestamp=int(body["timestamp"]),
            signature=str(body["signature_b64"]),
        )
    except (KeyError, ValueError, TypeError) as malformed:
        raise ClientException("malformed heartbeat") from malformed


def device_summary(device: Device, now: datetime) -> dict:
    """Serialize a device for the browser-facing listing.

    Carries the browser<->relay contract fields (``device_id``, ``approved``,
    ``status``, ``transport_public_key_b64`` — the SPA seals session keys to the
    transport key) plus the pre-contract SPA fields (``id``, ``name``,
    ``fingerprint``, ``last_seen_at``).

    ``status`` is derived from ``last_seen_at`` at ``now``, never read off the
    column: a bridge that stops heartbeating writes nothing, so only a derived
    status can see it go away.
    """
    return {
        "device_id": str(device.id),
        "id": str(device.id),
        "name": device.name,
        "fingerprint": pairing_crypto.fingerprint(device.identity_public_key_b64),
        "approved": device.approved,
        "status": presence.derived_status(device, now),
        "transport_public_key_b64": device.transport_public_key_b64,
        "last_seen_at": device.last_seen_at.isoformat() if device.last_seen_at else None,
    }


#: How many approved devices one account may hold. Enforced at approval — the
#: one moment a device becomes an account's — so revoking frees a slot.
MAX_DEVICES_PER_USER = 3
MAX_DEVICE_NAME_LENGTH = 255


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

    # 200, not Litestar's 201: nothing is created for the caller to find, and a
    # bridge treats any 2xx as delivered.
    @post(HEARTBEAT_ROUTE_PATH, status_code=HTTP_200_OK)
    async def heartbeat(self, request: Request, db_session: AsyncSession) -> Response:
        """A bridge reports that it is alive. Authenticated the way a transport
        report is — an Ed25519 signature over a timestamped challenge, bounded by
        the freshness window and the replay guard — and the only thing it writes
        is ``last_seen_at``; ``GET /api/devices`` derives ``status`` from that."""
        reported = read_heartbeat(await read_json_object(request))
        device = await db_session.get(Device, reported.device_id)
        if device is None or not device.approved or device.owner_user_id is None:
            raise NotAuthorizedException("heartbeat not authorized")
        challenge = presence.heartbeat_challenge(
            str(reported.device_id), reported.timestamp
        )
        if not pairing_crypto.verify_registration(
            device.identity_public_key_b64, challenge, reported.signature
        ):
            raise NotAuthorizedException("heartbeat signature invalid")
        now = utc_now()
        if not presence.heartbeat_timestamp_fresh(reported.timestamp, now):
            raise NotAuthorizedException("heartbeat timestamp out of window")
        if not _replay_guard.check_and_record(
            str(reported.device_id), reported.timestamp, reported.signature, now
        ):
            raise NotAuthorizedException("heartbeat replayed")
        device.last_seen_at = now
        await db_session.commit()
        return Response({"ok": True})

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
        now = utc_now()
        return Response({"devices": [device_summary(d, now) for d in rows]})

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

    @post(
        "/api/devices/{device_id:uuid}/rename",
        guards=[build_auth_guard],
        status_code=HTTP_200_OK,
    )
    async def rename(
        self, device_id: UUID, request: Request, db_session: AsyncSession
    ) -> Response:
        """Rename a device owned by the current account."""
        user_id = require_user(request)
        device = await db_session.get(Device, device_id)
        if device is None or device.owner_user_id != user_id or not device.approved:
            raise NotFoundException("device not found")
        body = await read_json_object(request)
        supplied_name = body.get("name")
        name = supplied_name.strip() if isinstance(supplied_name, str) else ""
        if not name:
            raise ClientException("device name required")
        if len(name) > MAX_DEVICE_NAME_LENGTH:
            raise ClientException(
                f"device name must be {MAX_DEVICE_NAME_LENGTH} characters or fewer"
            )
        device.name = name
        await db_session.commit()
        return Response({"device_id": str(device_id), "name": name})

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
