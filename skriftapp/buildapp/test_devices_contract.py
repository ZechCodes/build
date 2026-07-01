"""Contract tests for the browser-facing device listing: each device must expose
``device_id``, ``approved``, ``status``, and ``transport_public_key_b64`` (per the
browser<->relay contract) alongside the existing SPA fields."""

from __future__ import annotations

from datetime import datetime, timezone
from uuid import uuid4

from buildapp.devices_controller import device_summary
from buildapp.models import Device
from buildapp.pairing_crypto import fingerprint


def _device(**overrides) -> Device:
    defaults = {
        "id": uuid4(),
        "name": "workstation",
        "owner_user_id": uuid4(),
        "identity_public_key_b64": "aWRlbnRpdHk",
        "transport_public_key_b64": "dHJhbnNwb3J0",
        "approved": True,
        "status": "online",
        "last_seen_at": datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc),
    }
    defaults.update(overrides)
    return Device(**defaults)


def test_summary_contains_contract_fields():
    device = _device()
    summary = device_summary(device)
    assert summary["device_id"] == str(device.id)
    assert summary["approved"] is True
    assert summary["status"] == "online"
    assert summary["transport_public_key_b64"] == "dHJhbnNwb3J0"


def test_summary_keeps_existing_spa_fields():
    device = _device()
    summary = device_summary(device)
    assert summary["id"] == str(device.id)  # pre-contract key the SPA reads
    assert summary["name"] == "workstation"
    assert summary["fingerprint"] == fingerprint("aWRlbnRpdHk")
    assert summary["last_seen_at"] == "2026-07-01T12:00:00+00:00"


def test_summary_handles_never_seen_device():
    summary = device_summary(_device(last_seen_at=None, status="offline"))
    assert summary["last_seen_at"] is None
    assert summary["status"] == "offline"
