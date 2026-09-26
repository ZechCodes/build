"""Contract tests for the browser-facing device listing: each device must expose
``device_id``, ``approved``, ``status``, and ``transport_public_key_b64`` (per the
browser<->relay contract) alongside the existing SPA fields.

``status`` is derived from ``last_seen_at`` at read time, so every summary here
is taken at an explicit ``now``."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from uuid import uuid4

from buildapp.devices_controller import device_summary
from buildapp.models import Device
from buildapp.pairing_crypto import fingerprint

NOW = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)


def _device(**overrides) -> Device:
    defaults = {
        "id": uuid4(),
        "name": "workstation",
        "owner_user_id": uuid4(),
        "identity_public_key_b64": "aWRlbnRpdHk",
        "transport_public_key_b64": "dHJhbnNwb3J0",
        "approved": True,
        "last_seen_at": NOW - timedelta(seconds=30),
    }
    defaults.update(overrides)
    return Device(**defaults)


def test_summary_contains_contract_fields():
    device = _device()
    summary = device_summary(device, NOW)
    assert summary["device_id"] == str(device.id)
    assert summary["approved"] is True
    assert summary["status"] == "online"
    assert summary["transport_public_key_b64"] == "dHJhbnNwb3J0"


def test_summary_keeps_existing_spa_fields():
    device = _device()
    summary = device_summary(device, NOW)
    assert summary["id"] == str(device.id)  # pre-contract key the SPA reads
    assert summary["name"] == "workstation"
    assert summary["fingerprint"] == fingerprint("aWRlbnRpdHk")
    assert summary["last_seen_at"] == "2026-07-01T11:59:30+00:00"


def test_summary_handles_never_seen_device():
    summary = device_summary(_device(last_seen_at=None), NOW)
    assert summary["last_seen_at"] is None
    assert summary["status"] == "offline"


def test_summary_status_follows_the_heartbeat_window():
    """A device last seen two minutes ago is past the window: the browser is told
    it is offline."""
    stale = _device(last_seen_at=NOW - timedelta(minutes=2))
    assert device_summary(stale, NOW)["status"] == "offline"
