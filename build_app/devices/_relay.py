"""Redis stream integration for the relay service.

Publisher: sends commands to the relay (send message to device, disconnect device).
Consumer: reads device events from the relay (envelopes, status, heartbeat, connect/disconnect).
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
from typing import Any
from uuid import UUID

import redis.asyncio as aioredis

from skrift.lib.notifications import notify_user, NotificationMode

from build_app.devices._state import (
    _e2e_sessions,
    cleanup_expired_sessions,
)

logger = logging.getLogger(__name__)

# Stream keys (must match build-relay).
DEVICE_EVENTS_STREAM = "relay:device-events"
DEVICE_COMMANDS_STREAM = "relay:device-commands"

# Consumer group names.
WEB_CONSUMER_GROUP = "web-consumers"
RELAY_CONSUMER_GROUP = "relay-consumers"

_redis: aioredis.Redis | None = None
_consumer_task: asyncio.Task | None = None
_session_cleanup_task: asyncio.Task | None = None


async def init_relay() -> None:
    """Connect to Redis and start the event consumer."""
    global _redis
    if _redis is not None:
        return  # Already initialized.
    redis_url = os.environ.get("REDIS_URL", "redis://localhost:6379/0")
    _redis = aioredis.from_url(redis_url, decode_responses=True)

    # Ensure consumer groups exist.
    for stream, group in [
        (DEVICE_EVENTS_STREAM, WEB_CONSUMER_GROUP),
        (DEVICE_COMMANDS_STREAM, RELAY_CONSUMER_GROUP),
    ]:
        try:
            await _redis.xgroup_create(stream, group, id="0", mkstream=True)
        except aioredis.ResponseError as e:
            if "BUSYGROUP" not in str(e):
                raise

    _start_event_consumer()
    _start_session_cleanup()
    logger.info("Relay stream consumer started (REDIS_URL=%s)", redis_url)


async def close_relay() -> None:
    """Stop consumers and disconnect from Redis."""
    global _redis
    _stop_event_consumer()
    _stop_session_cleanup()
    if _redis:
        await _redis.aclose()
        _redis = None


# ---------------------------------------------------------------------------
# Publishing commands (web app → relay)
# ---------------------------------------------------------------------------

async def send_to_device(device_id: UUID, message: dict[str, Any]) -> None:
    """Publish a command to send a message to a device via the relay."""
    if not _redis:
        raise RuntimeError("Relay not initialized")
    await _redis.xadd(DEVICE_COMMANDS_STREAM, {
        "type": "send",
        "device_id": str(device_id),
        "message": json.dumps(message),
    }, maxlen=10000)


async def disconnect_device(device_id: UUID) -> None:
    """Publish a command to disconnect a device via the relay."""
    if not _redis:
        raise RuntimeError("Relay not initialized")
    await _redis.xadd(DEVICE_COMMANDS_STREAM, {
        "type": "disconnect",
        "device_id": str(device_id),
    }, maxlen=10000)


async def is_device_connected(device_id: UUID) -> bool:
    """Check if a device is connected to the relay (via internal API fallback or stream)."""
    # For now, check by querying the relay's internal API.
    # This is a lightweight HTTP call to the relay pod.
    relay_url = os.environ.get("RELAY_INTERNAL_URL", "http://build-relay:8081")
    import httpx
    try:
        async with httpx.AsyncClient(timeout=5) as client:
            resp = await client.get(f"{relay_url}/internal/device/{device_id}/connected")
            if resp.status_code == 200:
                return resp.json().get("connected", False)
    except Exception:
        logger.warning("Failed to check device connection status via relay")
    return False


# ---------------------------------------------------------------------------
# Consuming device events (relay → web app)
# ---------------------------------------------------------------------------

async def _process_event(fields: dict[str, str]) -> None:
    """Process a single device event from the relay."""
    event_type = fields.get("type", "")
    device_id_str = fields.get("device_id", "")

    try:
        device_id = UUID(device_id_str)
    except (ValueError, AttributeError):
        logger.warning("Invalid device_id in event: %s", device_id_str)
        return

    if event_type in ("e2ee_envelope", "session_accept"):
        session_id = fields.get("session_id", "")
        envelope_raw = fields.get("envelope", "")
        try:
            envelope = json.loads(envelope_raw)
        except (json.JSONDecodeError, TypeError):
            envelope = envelope_raw

        session = _e2e_sessions.get(session_id)
        if not session or session.device_id != device_id:
            logger.debug("Dropping %s for unknown session %s", event_type, session_id)
            return
        if session.expired:
            _e2e_sessions.pop(session_id, None)
            return

        session.touch()
        await notify_user(
            str(session.owner_user_id),
            "build:e2ee:envelope",
            mode=NotificationMode.EPHEMERAL,
            push_notify=False,
            session_id=session_id,
            envelope=envelope,
        )

    elif event_type == "connected":
        owner_user_id = fields.get("owner_user_id", "")
        device_name = fields.get("device_name", "")
        await _notify_device_event(owner_user_id, "online", device_id, device_name)

    elif event_type == "disconnected":
        owner_user_id = fields.get("owner_user_id", "")
        device_name = fields.get("device_name", "")
        # Clean up E2EE sessions for this device.
        stale = [sid for sid, s in _e2e_sessions.items() if s.device_id == device_id]
        for sid in stale:
            _e2e_sessions.pop(sid, None)
        await _notify_device_event(owner_user_id, "offline", device_id, device_name)

    elif event_type == "e2ee-ready":
        owner_user_id = fields.get("owner_user_id", "")
        device_name = fields.get("device_name", "")
        await _notify_device_event(owner_user_id, "e2ee-ready", device_id, device_name)

    elif event_type == "heartbeat-missed":
        owner_user_id = fields.get("owner_user_id", "")
        device_name = fields.get("device_name", "")
        elapsed_s = fields.get("elapsed_s", "0")
        await _notify_device_event(
            owner_user_id, "heartbeat-missed", device_id, device_name,
            elapsed_s=float(elapsed_s),
        )

    elif event_type == "heartbeat-resumed":
        # Device was marked offline after a heartbeat-missed notification;
        # its heartbeats are flowing again. Tell the dashboard it's
        # online so the sidebar / reconnect pill un-sticks live without
        # waiting for the next fetchDevices refresh.
        owner_user_id = fields.get("owner_user_id", "")
        device_name = fields.get("device_name", "")
        await _notify_device_event(owner_user_id, "online", device_id, device_name)

    elif event_type == "status":
        owner_user_id = fields.get("owner_user_id", "")
        device_name = fields.get("device_name", "")
        extra = {k: v for k, v in fields.items()
                 if k not in ("type", "device_id", "owner_user_id", "device_name")}
        await _notify_device_event(
            owner_user_id, "status", device_id, device_name, **extra,
        )

    elif event_type == "heartbeat":
        pass  # DB update handled by relay

    else:
        logger.debug("Unknown relay event type: %s", event_type)


async def _notify_device_event(
    owner_user_id: str,
    event_type: str,
    device_id: UUID,
    device_name: str,
    **extra: Any,
) -> None:
    """Fire a Skrift SSE notification for the dashboard."""
    await notify_user(
        owner_user_id,
        f"build:device:{event_type}",
        mode=NotificationMode.TIMESERIES,
        push_notify=False,
        device_id=str(device_id),
        device_name=device_name,
        **extra,
    )


async def _event_consumer_loop() -> None:
    """Continuously read device events from the relay Redis stream."""
    assert _redis is not None
    consumer_name = "web-1"

    while True:
        try:
            results = await _redis.xreadgroup(
                WEB_CONSUMER_GROUP, consumer_name,
                {DEVICE_EVENTS_STREAM: ">"},
                count=10, block=5000,
            )
            if not results:
                continue
            for _stream, messages in results:
                for msg_id, fields in messages:
                    try:
                        await _process_event(fields)
                    except Exception:
                        logger.exception("Error processing relay event %s", msg_id)
                    await _redis.xack(DEVICE_EVENTS_STREAM, WEB_CONSUMER_GROUP, msg_id)
        except asyncio.CancelledError:
            raise
        except aioredis.ResponseError as e:
            # Same self-heal as the relay's command consumer — the
            # stream/group can disappear (FLUSHDB, XGROUP DESTROY) and
            # init_relay only runs at startup, so without this we'd
            # tight-loop on NOGROUP and never deliver another envelope.
            if "NOGROUP" in str(e):
                logger.warning("Consumer group missing, recreating: %s", e)
                try:
                    await _redis.xgroup_create(
                        DEVICE_EVENTS_STREAM, WEB_CONSUMER_GROUP,
                        id="$", mkstream=True,
                    )
                except aioredis.ResponseError as create_err:
                    if "BUSYGROUP" not in str(create_err):
                        logger.exception("Failed to recreate consumer group")
                        await asyncio.sleep(2)
                continue
            logger.exception("Relay event consumer error, retrying in 2s")
            await asyncio.sleep(2)
        except Exception:
            logger.exception("Relay event consumer error, retrying in 2s")
            await asyncio.sleep(2)


async def _session_cleanup_loop() -> None:
    """Periodically clean up expired E2EE sessions."""
    while True:
        await asyncio.sleep(60)
        removed = cleanup_expired_sessions()
        if removed:
            logger.info("Cleaned up %d expired E2EE sessions", len(removed))


def _start_event_consumer() -> None:
    global _consumer_task
    if _consumer_task is None or _consumer_task.done():
        _consumer_task = asyncio.create_task(_event_consumer_loop())


def _stop_event_consumer() -> None:
    global _consumer_task
    if _consumer_task and not _consumer_task.done():
        _consumer_task.cancel()
        _consumer_task = None


def _start_session_cleanup() -> None:
    global _session_cleanup_task
    if _session_cleanup_task is None or _session_cleanup_task.done():
        _session_cleanup_task = asyncio.create_task(_session_cleanup_loop())


def _stop_session_cleanup() -> None:
    global _session_cleanup_task
    if _session_cleanup_task and not _session_cleanup_task.done():
        _session_cleanup_task.cancel()
        _session_cleanup_task = None
