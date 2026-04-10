"""Shared in-memory registries and dataclasses for device management."""

from __future__ import annotations

import asyncio
import time
from dataclasses import dataclass, field
from uuid import UUID

from litestar.connection import WebSocket


# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

# How many seconds without a heartbeat before we consider the device offline.
HEARTBEAT_TIMEOUT_MULTIPLIER = 2.5
# Max missed windows to keep per device (rolling).
MAX_MISSED_WINDOWS = 100
# How long a pending registration is valid (10 minutes).
PENDING_EXPIRY_S = 600
# Max E2EE session age (1 hour).
SESSION_TTL_S = 3600
# Max envelope size in bytes (2 MB — needed for chunked image transfer).
MAX_ENVELOPE_SIZE = 2 * 1024 * 1024


# ---------------------------------------------------------------------------
# E2EE session registry
# ---------------------------------------------------------------------------

@dataclass
class E2ESession:
    """Tracks an active E2EE session between a browser and a device."""
    session_id: str
    device_id: UUID
    owner_user_id: UUID
    created_at: float = field(default_factory=time.time)
    last_activity: float = field(default_factory=time.time)

    @property
    def expired(self) -> bool:
        return time.time() - self.last_activity > SESSION_TTL_S

    def touch(self) -> None:
        """Refresh session TTL on activity."""
        self.last_activity = time.time()


_e2e_sessions: dict[str, E2ESession] = {}


def get_e2e_sessions() -> dict[str, E2ESession]:
    """Expose for testing."""
    return _e2e_sessions


def cleanup_expired_sessions() -> list[str]:
    """Remove expired E2EE sessions, return list of removed session IDs."""
    expired = [sid for sid, s in _e2e_sessions.items() if s.expired]
    for sid in expired:
        _e2e_sessions.pop(sid, None)
    return expired


# ---------------------------------------------------------------------------
# Pending registrations (short-lived, for auth flow)
# ---------------------------------------------------------------------------

@dataclass
class PendingRegistration:
    code: str
    name: str
    public_key_b64: str
    created_at: float = field(default_factory=time.time)
    result_queue: asyncio.Queue = field(default_factory=asyncio.Queue)


_pending_registrations: dict[str, PendingRegistration] = {}


def cleanup_expired_pending() -> None:
    """Remove expired pending registrations."""
    now = time.time()
    expired = [
        code for code, reg in _pending_registrations.items()
        if now - reg.created_at > PENDING_EXPIRY_S
    ]
    for code in expired:
        _pending_registrations.pop(code, None)


# ---------------------------------------------------------------------------
# Connected devices registry (for heartbeat monitor + WS relay)
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
