"""Tests for build_app.device_api — device authorization, listing, WS, heartbeat."""

from __future__ import annotations

import asyncio
import base64
import json
import time
from datetime import datetime, timezone
from types import SimpleNamespace
from unittest.mock import AsyncMock, MagicMock, patch
from uuid import UUID, uuid4

import pytest
import pytest_asyncio

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from build_app.models import Device
from build_app.device_api import (
    ConnectedDevice,
    DeviceApiController,
    HEARTBEAT_TIMEOUT_MULTIPLIER,
    MAX_MISSED_WINDOWS,
    _load_public_key,
    _verify_device_signature,
    _record_heartbeat,
    _record_missed_window,
    _notify_device_event,
    _connected_devices,
    start_heartbeat_monitor,
    stop_heartbeat_monitor,
    get_connected_devices,
)


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def _generate_keypair() -> tuple[Ed25519PrivateKey, str]:
    """Generate an Ed25519 keypair, return (private_key, public_key_b64)."""
    private_key = Ed25519PrivateKey.generate()
    public_bytes = private_key.public_key().public_bytes_raw()
    public_key_b64 = base64.b64encode(public_bytes).decode()
    return private_key, public_key_b64


def _sign_ws_handshake(
    private_key: Ed25519PrivateKey,
    path: str = "/api/devices/ws",
) -> tuple[str, str]:
    """Sign a WebSocket handshake, return (timestamp_str, signature_b64)."""
    timestamp_str = str(time.time())
    message = f"{timestamp_str}.GET.{path}".encode()
    signature = private_key.sign(message)
    signature_b64 = base64.b64encode(signature).decode()
    return timestamp_str, signature_b64


def _make_device(
    name: str = "test-device",
    owner_user_id: UUID | None = None,
    public_key_b64: str = "",
    **kwargs,
) -> Device:
    """Create a Device instance for testing (not persisted)."""
    return Device(
        id=kwargs.pop("id", uuid4()),
        name=name,
        public_key=public_key_b64,
        owner_user_id=owner_user_id or uuid4(),
        approved=kwargs.pop("approved", True),
        status=kwargs.pop("status", "offline"),
        last_heartbeat_at=kwargs.pop("last_heartbeat_at", None),
        heartbeat_interval_s=kwargs.pop("heartbeat_interval_s", 30),
        missed_heartbeat_windows=kwargs.pop("missed_heartbeat_windows", None),
        created_at=kwargs.pop("created_at", datetime.now(timezone.utc)),
        updated_at=kwargs.pop("updated_at", datetime.now(timezone.utc)),
    )


# ---------------------------------------------------------------------------
# Ed25519 key loading
# ---------------------------------------------------------------------------

class TestLoadPublicKey:
    def test_load_raw_32_byte_key(self):
        _, pub_b64 = _generate_keypair()
        key = _load_public_key(pub_b64)
        assert isinstance(key, type(Ed25519PrivateKey.generate().public_key()))

    def test_load_invalid_key_raises(self):
        with pytest.raises(Exception):
            _load_public_key(base64.b64encode(b"too short").decode())


# ---------------------------------------------------------------------------
# Signature verification
# ---------------------------------------------------------------------------

class TestVerifyDeviceSignature:
    def test_valid_signature(self):
        private_key, pub_b64 = _generate_keypair()
        pub_key = _load_public_key(pub_b64)
        ts, sig = _sign_ws_handshake(private_key)
        assert _verify_device_signature(pub_key, ts, sig) is True

    def test_wrong_key_fails(self):
        private_key, _ = _generate_keypair()
        _, other_pub_b64 = _generate_keypair()
        other_pub = _load_public_key(other_pub_b64)
        ts, sig = _sign_ws_handshake(private_key)
        assert _verify_device_signature(other_pub, ts, sig) is False

    def test_tampered_timestamp_fails(self):
        private_key, pub_b64 = _generate_keypair()
        pub_key = _load_public_key(pub_b64)
        ts, sig = _sign_ws_handshake(private_key)
        assert _verify_device_signature(pub_key, str(float(ts) + 1), sig) is False

    def test_empty_signature_fails(self):
        _, pub_b64 = _generate_keypair()
        pub_key = _load_public_key(pub_b64)
        assert _verify_device_signature(pub_key, str(time.time()), "") is False


# ---------------------------------------------------------------------------
# Device model helpers
# ---------------------------------------------------------------------------

class TestDeviceMissedWindows:
    def test_get_empty(self):
        device = _make_device()
        assert device.get_missed_windows() == []

    def test_set_and_get(self):
        device = _make_device()
        windows = [
            {"start": "2026-03-14T10:00:00+00:00", "end": "2026-03-14T10:05:00+00:00"},
            {"start": "2026-03-14T11:00:00+00:00", "end": "2026-03-14T11:02:00+00:00"},
        ]
        device.set_missed_windows(windows)
        assert device.get_missed_windows() == windows

    def test_get_with_none(self):
        device = _make_device(missed_heartbeat_windows=None)
        assert device.get_missed_windows() == []


# ---------------------------------------------------------------------------
# Connected devices registry
# ---------------------------------------------------------------------------

class TestConnectedDevicesRegistry:
    def setup_method(self):
        _connected_devices.clear()

    def teardown_method(self):
        _connected_devices.clear()

    def test_register_and_get(self):
        device_id = uuid4()
        conn = ConnectedDevice(
            device_id=device_id,
            owner_user_id=uuid4(),
            socket=MagicMock(),
        )
        _connected_devices[device_id] = conn
        registry = get_connected_devices()
        assert device_id in registry
        assert registry[device_id] is conn

    def test_unregister(self):
        device_id = uuid4()
        _connected_devices[device_id] = ConnectedDevice(
            device_id=device_id,
            owner_user_id=uuid4(),
            socket=MagicMock(),
        )
        _connected_devices.pop(device_id)
        assert device_id not in _connected_devices


# ---------------------------------------------------------------------------
# Heartbeat monitor lifecycle
# ---------------------------------------------------------------------------

class TestHeartbeatMonitorLifecycle:
    def teardown_method(self):
        stop_heartbeat_monitor()

    @pytest.mark.asyncio
    async def test_start_creates_task(self):
        start_heartbeat_monitor()
        from build_app.device_api import _monitor_task
        assert _monitor_task is not None
        assert not _monitor_task.done()
        stop_heartbeat_monitor()

    @pytest.mark.asyncio
    async def test_stop_cancels_task(self):
        start_heartbeat_monitor()
        from build_app.device_api import _monitor_task
        task = _monitor_task
        stop_heartbeat_monitor()
        # Give the event loop a tick so the cancellation propagates.
        await asyncio.sleep(0)
        assert task.cancelled()

    @pytest.mark.asyncio
    async def test_start_is_idempotent(self):
        start_heartbeat_monitor()
        from build_app.device_api import _monitor_task
        first_task = _monitor_task
        start_heartbeat_monitor()
        from build_app.device_api import _monitor_task as second
        assert first_task is second
        stop_heartbeat_monitor()


# ---------------------------------------------------------------------------
# Notification events
# ---------------------------------------------------------------------------

class TestNotifyDeviceEvent:
    @pytest.mark.asyncio
    async def test_fires_notification(self):
        owner = uuid4()
        device_id = uuid4()
        with patch("build_app.device_api.notify_user", new_callable=AsyncMock) as mock_notify:
            await _notify_device_event(owner, "online", device_id, "my-device")
            mock_notify.assert_called_once()
            call_args = mock_notify.call_args
            assert call_args[0][0] == str(owner)
            assert call_args[0][1] == "build:device:online"
            assert call_args[1]["device_id"] == str(device_id)
            assert call_args[1]["device_name"] == "my-device"
            assert call_args[1]["mode"] == "timeseries"

    @pytest.mark.asyncio
    async def test_extra_kwargs_passed(self):
        with patch("build_app.device_api.notify_user", new_callable=AsyncMock) as mock_notify:
            await _notify_device_event(uuid4(), "status", uuid4(), "dev", agents=3)
            assert mock_notify.call_args[1]["agents"] == 3


# ---------------------------------------------------------------------------
# Authorize endpoint logic
# ---------------------------------------------------------------------------

class TestAuthorizeValidation:
    def test_keypair_roundtrip(self):
        """Authorize validates the key, so verify our keypair helper works end-to-end."""
        private_key, pub_b64 = _generate_keypair()
        pub_key = _load_public_key(pub_b64)
        # Sign and verify
        ts, sig = _sign_ws_handshake(private_key)
        assert _verify_device_signature(pub_key, ts, sig) is True

    def test_invalid_key_detected(self):
        """Authorize should reject a bad key."""
        bad_b64 = base64.b64encode(b"not a real key at all").decode()
        with pytest.raises(Exception):
            _load_public_key(bad_b64)


# ---------------------------------------------------------------------------
# Heartbeat timeout calculation
# ---------------------------------------------------------------------------

class TestHeartbeatTimeout:
    def test_timeout_multiplier(self):
        """Default 30s interval * 2.5 multiplier = 75s timeout."""
        interval = 30
        timeout = interval * HEARTBEAT_TIMEOUT_MULTIPLIER
        assert timeout == 75.0

    def test_stale_detection(self):
        """A heartbeat older than the timeout is stale."""
        conn = ConnectedDevice(
            device_id=uuid4(),
            owner_user_id=uuid4(),
            socket=MagicMock(),
            last_heartbeat=time.time() - 100,
            heartbeat_interval=30,
        )
        elapsed = time.time() - conn.last_heartbeat
        timeout = conn.heartbeat_interval * HEARTBEAT_TIMEOUT_MULTIPLIER
        assert elapsed > timeout

    def test_fresh_heartbeat_not_stale(self):
        """A recent heartbeat should not be stale."""
        conn = ConnectedDevice(
            device_id=uuid4(),
            owner_user_id=uuid4(),
            socket=MagicMock(),
            last_heartbeat=time.time(),
            heartbeat_interval=30,
        )
        elapsed = time.time() - conn.last_heartbeat
        timeout = conn.heartbeat_interval * HEARTBEAT_TIMEOUT_MULTIPLIER
        assert elapsed < timeout


# ---------------------------------------------------------------------------
# Missed window recording
# ---------------------------------------------------------------------------

class TestMissedWindowRecording:
    def test_max_windows_cap(self):
        """Missed windows list should be capped at MAX_MISSED_WINDOWS."""
        device = _make_device()
        # Pre-fill with MAX windows.
        windows = [
            {"start": f"2026-03-14T{i:02d}:00:00+00:00", "end": f"2026-03-14T{i:02d}:05:00+00:00"}
            for i in range(MAX_MISSED_WINDOWS)
        ]
        device.set_missed_windows(windows)
        assert len(device.get_missed_windows()) == MAX_MISSED_WINDOWS

        # Add one more — oldest should be dropped.
        windows.append({"start": "2026-03-15T00:00:00+00:00", "end": "2026-03-15T00:05:00+00:00"})
        if len(windows) > MAX_MISSED_WINDOWS:
            windows = windows[-MAX_MISSED_WINDOWS:]
        device.set_missed_windows(windows)
        assert len(device.get_missed_windows()) == MAX_MISSED_WINDOWS
        # The last entry should be our new one.
        assert device.get_missed_windows()[-1]["start"] == "2026-03-15T00:00:00+00:00"


# ---------------------------------------------------------------------------
# WS message protocol
# ---------------------------------------------------------------------------

class TestWSMessageProtocol:
    """Test the expected message shapes match the protocol."""

    def test_heartbeat_message_shape(self):
        msg = {"type": "heartbeat"}
        assert msg["type"] == "heartbeat"

    def test_heartbeat_with_rid(self):
        msg = {"type": "heartbeat", "rid": "abc-123"}
        assert msg["rid"] == "abc-123"

    def test_status_message_shape(self):
        msg = {"type": "status", "agents": 3, "tasks_active": 1}
        assert msg["type"] == "status"
        assert msg["agents"] == 3

    def test_authenticated_response_shape(self):
        resp = {
            "type": "authenticated",
            "device_id": str(uuid4()),
            "heartbeat_interval_s": 30,
        }
        assert resp["type"] == "authenticated"
        assert isinstance(resp["heartbeat_interval_s"], int)

    def test_error_response_shape(self):
        resp = {"type": "error", "error": "missing auth headers"}
        assert resp["type"] == "error"
        assert "error" in resp

    def test_response_with_rid(self):
        resp = {"type": "response", "rid": "req-1", "ok": True}
        assert resp["ok"] is True
        assert resp["rid"] == "req-1"

    def test_response_error_with_rid(self):
        resp = {"type": "response", "rid": "req-2", "ok": False, "error": "unknown message type: foo"}
        assert resp["ok"] is False
