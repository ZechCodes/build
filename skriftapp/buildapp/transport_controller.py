"""``POST /api/transport/report`` — a bridge reports one transport event for
one of its client sessions (``planning/v2/Transport Telemetry Spec.md``).

Authenticated like ``/api/push/notify``: by the device's Ed25519 signature over
a timestamped challenge that binds every field, verified against the identity
key pinned at pairing; a freshness window and a replay guard bound a captured
request. No browser session is involved, so no session guard. The row it
writes is one per ``(device, session)``, absorbing events in any order through
``transport_report.apply_event``.
"""

from __future__ import annotations

from datetime import datetime, timezone
from uuid import UUID

from litestar import Controller, Request, post
from litestar.exceptions import ClientException, NotAuthorizedException
from litestar.response import Response
from litestar.status_codes import HTTP_200_OK
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import pairing_crypto, transport_report
from buildapp.models import Device, TransportSession
from buildapp.request_body import require_json_object

REPORT_ROUTE_PATH = "/api/transport/report"

_replay_guard = transport_report.replay_guard()


def reset_replay_guard_for_tests() -> None:
    """A fresh guard per test app: the guard is process state, as in production."""
    global _replay_guard
    _replay_guard = transport_report.replay_guard()


def _now() -> datetime:
    return datetime.now(timezone.utc)


class TransportController(Controller):
    """The one api surface a bridge reports transport events to."""

    path = ""

    # 200, not Litestar's 201: nothing is created for the caller to find, and the
    # bridge treats any 2xx as delivered.
    @post(REPORT_ROUTE_PATH, status_code=HTTP_200_OK)
    async def report(self, request: Request, db_session: AsyncSession) -> Response:
        body = require_json_object(await request.json())
        try:
            device_id = UUID(str(body["device_id"]))
            session_id = str(body["session_id"])
            event = str(body["event"])
            path = str(body["path"])
            timestamp = int(body["timestamp"])
            signature = str(body["signature_b64"])
        except (KeyError, ValueError, TypeError):
            raise ClientException("malformed transport report")
        if event not in transport_report.ALLOWED_EVENTS:
            raise ClientException("unknown transport event")
        if path != transport_report.NO_PATH and path not in transport_report.ALLOWED_PATHS:
            raise ClientException("unknown transport path")
        if not session_id or len(session_id) > 64:
            raise ClientException("malformed session id")

        device = await db_session.get(Device, device_id)
        if device is None or not device.approved or device.owner_user_id is None:
            raise NotAuthorizedException("transport report not authorized")
        challenge = transport_report.report_challenge(
            str(device_id), session_id, event, path, timestamp
        )
        if not pairing_crypto.verify_registration(
            device.identity_public_key_b64, challenge, signature
        ):
            raise NotAuthorizedException("transport report signature invalid")
        now = _now()
        if not transport_report.report_timestamp_fresh(timestamp, now):
            raise NotAuthorizedException("transport report timestamp out of window")
        if not _replay_guard.check_and_record(str(device_id), timestamp, signature, now):
            raise NotAuthorizedException("transport report replayed")

        row = (
            await db_session.execute(
                select(TransportSession).where(
                    TransportSession.device_id == device_id,
                    TransportSession.session_id == session_id,
                )
            )
        ).scalar_one_or_none()
        if row is None:
            # Column defaults apply at flush; the rules run before it, so a
            # fresh row starts the way the table would start it.
            row = TransportSession(
                session_id=session_id,
                device_id=device_id,
                owner_user_id=device.owner_user_id,
                current_path=transport_report.RELAY,
                carrying_count=0,
                turn_count=0,
                fell_back_count=0,
            )
            db_session.add(row)
        at = datetime.fromtimestamp(timestamp, tz=timezone.utc)
        try:
            transport_report.apply_event(row, event, path, at)
        except ValueError as error:
            raise ClientException(str(error)) from error
        await db_session.commit()
        return Response({"ok": True})
