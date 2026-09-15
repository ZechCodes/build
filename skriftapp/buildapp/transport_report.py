"""Transport reports from bridges — the pure logic (``planning/v2/Transport
Telemetry Spec.md``).

A bridge reports four content-free events per client session: ``minted``,
``carrying`` (with a path, ``direct`` or ``turn``), ``channels_lost`` and
``ended``.
Each is signed by the device identity key over a challenge that binds every
field — ``bridge/src/transport_report.rs`` produces the same bytes — and the
freshness window and replay guard are the push notify's, one scheme for every
device-signed request.

``SessionRow`` and ``apply_event`` are how one session's row absorbs events in
any order; the model in ``models.py`` carries the same fields.
"""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime

from buildapp import web_push

MINTED = "minted"
CARRYING = "carrying"
#: The session's last DataChannel closed while the session lives — an ICE
#: restart is under way, or the session is about to end.
CHANNELS_LOST = "channels_lost"
#: What that event was called while the relay was still a data plane. A bridge
#: one release behind sends this word; it counts in the same column, and the
#: next release may drop it.
FELL_BACK = "fell_back"
ENDED = "ended"
ALLOWED_EVENTS = frozenset({MINTED, CARRYING, CHANNELS_LOST, FELL_BACK, ENDED})

DIRECT = "direct"
TURN = "turn"
RELAY = "relay"
ALLOWED_PATHS = frozenset({DIRECT, TURN})

#: The path field of an event that carries none.
NO_PATH = "-"

# One scheme for every device-signed report.
report_timestamp_fresh = web_push.notify_timestamp_fresh


def replay_guard() -> web_push.NotifyReplayGuard:
    return web_push.NotifyReplayGuard()


def report_challenge(
    device_id: str, session_id: str, event: str, path: str, timestamp: int
) -> str:
    """The message the bridge signs. Byte for byte
    ``bridge/src/transport_report.rs::report_challenge``."""
    return f"transport.{device_id}.{session_id}.{event}.{path}.{timestamp}"


@dataclass
class SessionRow:
    """One session's transport life, as the api keeps it. The ORM model
    mirrors these fields; this is the shape the rules are written against."""

    minted_at: datetime | None = None
    first_carrying_at: datetime | None = None
    first_path: str | None = None
    #: The path the session is carrying on, ``relay`` meaning "no channel" —
    #: the column's word since before the relay stopped being a data plane.
    current_path: str = RELAY
    carrying_count: int = 0
    turn_count: int = 0
    fell_back_count: int = 0
    ended_at: datetime | None = None


def apply_event(  # noqa: C901 — ratchet: at 13, cap 10; a handler table keyed by event word, then drop this
    row: SessionRow, event: str, path: str, at: datetime
) -> None:
    """Absorb one event into ``row``. Out-of-order and repeated events are
    absorbed (spec §Storage): the earliest ``minted`` and ``ended`` stand, a
    ``carrying`` before its ``minted`` still counts, and nothing unsets an end.
    Unknown words, a ``carrying`` without a path, or a path on any other event
    raise ``ValueError`` — the controller answers 400."""
    if event not in ALLOWED_EVENTS:
        raise ValueError(f"unknown transport event {event!r}")
    if event == CARRYING:
        if path not in ALLOWED_PATHS:
            raise ValueError(f"a carrying needs a path, not {path!r}")
    elif path != NO_PATH:
        raise ValueError(f"{event} carries no path, got {path!r}")

    if event == MINTED:
        if row.minted_at is None or at < row.minted_at:
            row.minted_at = at
    elif event == CARRYING:
        if row.first_carrying_at is None or at < row.first_carrying_at:
            row.first_carrying_at = at
            row.first_path = path
        row.current_path = path
        row.carrying_count += 1
        if path == TURN:
            row.turn_count += 1
    elif event in (CHANNELS_LOST, FELL_BACK):
        row.current_path = RELAY
        row.fell_back_count += 1
    elif event == ENDED:
        if row.ended_at is None or at < row.ended_at:
            row.ended_at = at
