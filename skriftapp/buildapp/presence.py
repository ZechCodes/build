"""Device presence — the pure logic (``planning/v2/Strict P2P Transport Spec.md``
rule 6, "Presence is the api's").

A bridge posts a signed heartbeat every 30 s; the api stores nothing but
``last_seen_at`` and derives ``status`` at read time. That makes presence
self-healing: a bridge that dies, loses its network or is killed writes no
"offline" — it simply stops writing, and every reader after the window sees
``offline``. The relay is not involved.

The challenge and the freshness/replay scheme are the push notify's, one scheme
for every device-signed request; ``bridge/src/presence.rs`` produces the same
bytes, and ``bridge/tests/fixtures/presence_challenge.txt`` is the fixture both
test suites read.
"""

from __future__ import annotations

from datetime import datetime, timedelta, timezone

from buildapp import web_push

#: How long after its last heartbeat a device still reads as ``online``. Three
#: missed 30 s heartbeats: one lost post is not a device going away.
ONLINE_WINDOW = timedelta(seconds=90)

PENDING = "pending"
ONLINE = "online"
OFFLINE = "offline"

# One scheme for every device-signed request.
heartbeat_timestamp_fresh = web_push.notify_timestamp_fresh


def replay_guard() -> web_push.NotifyReplayGuard:
    return web_push.NotifyReplayGuard()


def heartbeat_challenge(device_id: str, timestamp: int) -> str:
    """The message the bridge signs. Byte for byte
    ``bridge/src/presence.rs::heartbeat_challenge``."""
    return f"heartbeat.{device_id}.{timestamp}"


def _as_utc(moment: datetime) -> datetime:
    """SQLite hands naive datetimes back out of ``DateTimeUTC``; they are UTC."""
    return moment if moment.tzinfo is not None else moment.replace(tzinfo=timezone.utc)


def derived_status(device, now: datetime) -> str:
    """The status a reader sees: ``pending`` while the device waits for approval,
    ``online`` iff it heartbeated within :data:`ONLINE_WINDOW`, else ``offline``.

    A device that lost its approval (revoked) is ``offline``, never ``pending``:
    it is not waiting for anyone.
    """
    if not device.approved:
        return PENDING if device.status == PENDING else OFFLINE
    last_seen_at = device.last_seen_at
    if last_seen_at is None:
        return OFFLINE
    return ONLINE if now - _as_utc(last_seen_at) <= ONLINE_WINDOW else OFFLINE
