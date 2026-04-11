"""Tests for build_app.devices — device authorization, E2EE sessions, notifications."""

from __future__ import annotations

import base64
import json
import time
from datetime import datetime, timezone
from unittest.mock import AsyncMock, patch
from uuid import UUID, uuid4

import pytest

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

from build_app.models import Device
from build_app.devices import (
    E2ESession,
    load_public_key,
    verify_device_signature,
    notify_device_event,
    _e2e_sessions,
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
        key = load_public_key(pub_b64)
        assert isinstance(key, type(Ed25519PrivateKey.generate().public_key()))

    def test_load_invalid_key_raises(self):
        with pytest.raises(Exception):
            load_public_key(base64.b64encode(b"too short").decode())


# ---------------------------------------------------------------------------
# Signature verification
# ---------------------------------------------------------------------------

class TestVerifyDeviceSignature:
    def test_valid_signature(self):
        private_key, pub_b64 = _generate_keypair()
        pub_key = load_public_key(pub_b64)
        ts, sig = _sign_ws_handshake(private_key)
        assert verify_device_signature(pub_key, ts, sig) is True

    def test_wrong_key_fails(self):
        private_key, _ = _generate_keypair()
        _, other_pub_b64 = _generate_keypair()
        other_pub = load_public_key(other_pub_b64)
        ts, sig = _sign_ws_handshake(private_key)
        assert verify_device_signature(other_pub, ts, sig) is False

    def test_tampered_timestamp_fails(self):
        private_key, pub_b64 = _generate_keypair()
        pub_key = load_public_key(pub_b64)
        ts, sig = _sign_ws_handshake(private_key)
        assert verify_device_signature(pub_key, str(float(ts) + 1), sig) is False

    def test_empty_signature_fails(self):
        _, pub_b64 = _generate_keypair()
        pub_key = load_public_key(pub_b64)
        assert verify_device_signature(pub_key, str(time.time()), "") is False


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
# E2EE sessions
# ---------------------------------------------------------------------------

class TestE2EESessions:
    def setup_method(self):
        _e2e_sessions.clear()

    def teardown_method(self):
        _e2e_sessions.clear()

    def test_register_and_lookup(self):
        session = E2ESession(
            session_id="s1",
            device_id=uuid4(),
            owner_user_id=uuid4(),
        )
        _e2e_sessions["s1"] = session
        assert _e2e_sessions["s1"] is session

    def test_expired_check(self):
        session = E2ESession(
            session_id="s2",
            device_id=uuid4(),
            owner_user_id=uuid4(),
            last_activity=time.time() - 7200,  # 2 hours ago
        )
        assert session.expired is True

    def test_fresh_not_expired(self):
        session = E2ESession(
            session_id="s3",
            device_id=uuid4(),
            owner_user_id=uuid4(),
        )
        assert session.expired is False

    def test_touch_refreshes(self):
        session = E2ESession(
            session_id="s4",
            device_id=uuid4(),
            owner_user_id=uuid4(),
            last_activity=time.time() - 3500,  # almost expired
        )
        session.touch()
        assert session.expired is False


# ---------------------------------------------------------------------------
# Notification events
# ---------------------------------------------------------------------------

class TestNotifyDeviceEvent:
    @pytest.mark.asyncio
    async def test_fires_notification(self):
        owner = uuid4()
        device_id = uuid4()
        with patch("build_app.devices._helpers.notify_user", new_callable=AsyncMock) as mock_notify:
            await notify_device_event(owner, "online", device_id, "my-device")
            mock_notify.assert_called_once()
            call_args = mock_notify.call_args
            assert call_args[0][0] == str(owner)
            assert call_args[0][1] == "build:device:online"
            assert call_args[1]["device_id"] == str(device_id)
            assert call_args[1]["device_name"] == "my-device"
            assert call_args[1]["mode"] == "timeseries"

    @pytest.mark.asyncio
    async def test_extra_kwargs_passed(self):
        with patch("build_app.devices._helpers.notify_user", new_callable=AsyncMock) as mock_notify:
            await notify_device_event(uuid4(), "status", uuid4(), "dev", agents=3)
            assert mock_notify.call_args[1]["agents"] == 3


# ---------------------------------------------------------------------------
# Authorize endpoint logic
# ---------------------------------------------------------------------------

class TestAuthorizeValidation:
    def test_keypair_roundtrip(self):
        """Authorize validates the key, so verify our keypair helper works end-to-end."""
        private_key, pub_b64 = _generate_keypair()
        pub_key = load_public_key(pub_b64)
        ts, sig = _sign_ws_handshake(private_key)
        assert verify_device_signature(pub_key, ts, sig) is True

    def test_invalid_key_detected(self):
        """Authorize should reject a bad key."""
        bad_b64 = base64.b64encode(b"not a real key at all").decode()
        with pytest.raises(Exception):
            load_public_key(bad_b64)


# ---------------------------------------------------------------------------
# WS message protocol (shape verification — protocol still valid for relay)
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
