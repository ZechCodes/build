"""The admin transport page — how many sessions ride WebRTC directly, how many
had to relay through Cloudflare TURN, and how many never left the WebSocket
relay (``planning/v2/Transport Telemetry Spec.md`` §Classification).

One read-only page at ``/admin/transport``, in the admin nav behind the
``administrator`` permission, over the rows ``/api/transport/report`` keeps.
"""

from __future__ import annotations

from collections import Counter, defaultdict
from datetime import datetime, timedelta, timezone
from typing import Any, Iterable
from uuid import UUID

from litestar import Controller, Request, get
from litestar.response import Template as TemplateResponse
from skrift.admin.helpers import get_admin_context
from skrift.admin.navigation import ADMIN_NAV_TAG
from skrift.auth.guards import Permission, auth_guard
from skrift.flash import get_flash_messages
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import transport_report
from buildapp.models import Device, TransportSession

DIRECT = "direct"
TURN = "turn"
RELAY_ONLY = "relay_only"
UNSTABLE = "unstable"
BUCKETS = (DIRECT, TURN, RELAY_ONLY, UNSTABLE)

WINDOWS: dict[str, timedelta] = {
    "24h": timedelta(hours=24),
    "7d": timedelta(days=7),
    "30d": timedelta(days=30),
}
DEFAULT_WINDOW = "7d"
RECENT_LIMIT = 50
UNKNOWN_DEVICE = "unknown device"


def classify(row: TransportSession) -> str:
    """The bucket one session falls in (spec §Classification). TURN anywhere in
    its life is TURN — that is the billed one — else a fallback makes an
    upgraded session UNSTABLE, else it is DIRECT, and one that never carried
    is RELAY_ONLY."""
    if row.turn_count > 0:
        return TURN
    if row.first_path is None:
        return RELAY_ONLY
    if row.fell_back_count > 0:
        return UNSTABLE
    return DIRECT


def _empty_counts() -> dict[str, int]:
    return {"sessions": 0, **{bucket: 0 for bucket in BUCKETS}}


def _share(part: int, whole: int) -> str:
    return f"{round(100 * part / whole)}%" if whole else "—"


def build_transport_dashboard(
    rows: Iterable[TransportSession],
    devices: Iterable[Device],
    *,
    window: str,
    now: datetime | None = None,
) -> dict[str, Any]:
    """Everything the template shows: totals over the window (by ``minted_at``),
    the same per device, and the newest sessions. Pure over its inputs."""
    now = now or datetime.now(timezone.utc)
    if window not in WINDOWS:
        window = DEFAULT_WINDOW
    since = now - WINDOWS[window]
    names: dict[UUID, str] = {device.id: device.name for device in devices}

    totals = _empty_counts()
    per_device: dict[UUID, dict[str, int]] = defaultdict(_empty_counts)
    in_window: list[TransportSession] = []
    for row in rows:
        if row.minted_at is None or row.minted_at < since:
            continue
        in_window.append(row)
        bucket = classify(row)
        for counts in (totals, per_device[row.device_id]):
            counts["sessions"] += 1
            counts[bucket] += 1

    device_rows = [
        {"device": names.get(device_id, UNKNOWN_DEVICE), "device_id": str(device_id), **counts}
        for device_id, counts in per_device.items()
    ]
    device_rows.sort(key=lambda r: (-r["sessions"], r["device"]))

    in_window.sort(key=lambda r: r.minted_at, reverse=True)
    recent = [
        {
            "session_id": row.session_id,
            "device": names.get(row.device_id, UNKNOWN_DEVICE),
            "bucket": classify(row),
            "first_path": row.first_path or transport_report.RELAY,
            "current_path": row.current_path,
            "carrying_count": row.carrying_count,
            "turn_count": row.turn_count,
            "fell_back_count": row.fell_back_count,
            "minted_at": row.minted_at,
            "ended": row.ended_at is not None,
        }
        for row in in_window[:RECENT_LIMIT]
    ]

    return {
        "window": window,
        "windows": list(WINDOWS),
        "since": since,
        "totals": totals,
        "direct_share": _share(totals[DIRECT], totals["sessions"]),
        "turn_share": _share(totals[TURN], totals["sessions"]),
        "relay_share": _share(totals[RELAY_ONLY], totals["sessions"]),
        "devices": device_rows,
        "recent": recent,
        "labels": {
            DIRECT: "Direct WebRTC",
            TURN: "TURN",
            RELAY_ONLY: "Relay only",
            UNSTABLE: "Unstable",
        },
    }


class TransportAdminController(Controller):
    """Read-only dashboard over the sessions bridges reported."""

    path = "/admin"
    guards = [auth_guard]

    @get(
        "/transport",
        tags=[ADMIN_NAV_TAG],
        guards=[auth_guard, Permission("administrator")],
        opt={"label": "Transport", "icon": "radio", "order": 95},
    )
    async def transport(
        self, request: Request, db_session: AsyncSession, window: str = DEFAULT_WINDOW
    ) -> TemplateResponse:
        ctx = await get_admin_context(request, db_session)
        since = datetime.now(timezone.utc) - WINDOWS.get(window, WINDOWS[DEFAULT_WINDOW])
        rows = (
            (
                await db_session.execute(
                    select(TransportSession).where(TransportSession.minted_at >= since)
                )
            )
            .scalars()
            .all()
        )
        devices = (await db_session.execute(select(Device))).scalars().all()
        dashboard = build_transport_dashboard(rows, devices, window=window)
        return TemplateResponse(
            "admin/transport.html",
            context={
                "flash_messages": get_flash_messages(request),
                "dashboard": dashboard,
                **ctx,
            },
        )
