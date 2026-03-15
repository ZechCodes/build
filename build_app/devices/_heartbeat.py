"""Background heartbeat monitor task with periodic session cleanup."""

from __future__ import annotations

import asyncio
import logging
import time

from build_app.devices._state import (
    _connected_devices,
    _e2e_sessions,
    HEARTBEAT_TIMEOUT_MULTIPLIER,
    cleanup_expired_sessions,
)
from build_app.devices._helpers import notify_device_event

logger = logging.getLogger(__name__)

_monitor_task: asyncio.Task | None = None


async def _heartbeat_monitor_loop() -> None:
    """Periodically check connected devices for missed heartbeats and clean up expired sessions."""
    cycle = 0
    while True:
        await asyncio.sleep(10)  # Check every 10 seconds
        now = time.time()
        cycle += 1

        # Check heartbeats.
        for device_id, conn in list(_connected_devices.items()):
            timeout = conn.heartbeat_interval * HEARTBEAT_TIMEOUT_MULTIPLIER
            elapsed = now - conn.last_heartbeat
            if elapsed > timeout:
                logger.warning(
                    "Device %s missed heartbeat (%.1fs since last)",
                    device_id, elapsed,
                )
                await notify_device_event(
                    conn.owner_user_id,
                    "heartbeat-missed",
                    device_id,
                    "",  # name not cached here; event consumers can look it up
                    elapsed_s=round(elapsed, 1),
                )

        # Clean up expired E2EE sessions every ~60s (6 cycles).
        if cycle % 6 == 0:
            removed = cleanup_expired_sessions()
            if removed:
                logger.info("Cleaned up %d expired E2EE sessions", len(removed))


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
