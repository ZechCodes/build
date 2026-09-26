"""Presence, the pure part: the challenge a bridge signs and the status the api
derives from ``last_seen_at`` (``planning/v2/Strict P2P Transport Spec.md`` rule 6)."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from pathlib import Path
from uuid import uuid4

from buildapp import presence
from buildapp.models import Device

#: The one fixture both languages read, so the two challenge builders cannot
#: drift apart unnoticed (``bridge/tests/presence.rs`` reads the same file).
CHALLENGE_FIXTURE = (
    Path(__file__).resolve().parents[2] / "bridge" / "tests" / "fixtures" / "presence_challenge.txt"
)


def _fixture() -> dict[str, str]:
    fields = {}
    for line in CHALLENGE_FIXTURE.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        key, _, value = line.partition("=")
        fields[key.strip()] = value.strip()
    return fields


def _device(**overrides) -> Device:
    defaults = {
        "id": uuid4(),
        "name": "workstation",
        "owner_user_id": uuid4(),
        "identity_public_key_b64": "aWRlbnRpdHk",
        "transport_public_key_b64": "dHJhbnNwb3J0",
        "approved": True,
        "last_seen_at": None,
    }
    defaults.update(overrides)
    return Device(**defaults)


def test_the_challenge_is_the_shape_the_bridge_signs():
    assert (
        presence.heartbeat_challenge("dev-1", 1750000000) == "heartbeat.dev-1.1750000000"
    )


def test_the_challenge_matches_the_shared_fixture_byte_for_byte():
    fields = _fixture()
    assert (
        presence.heartbeat_challenge(fields["device_id"], int(fields["timestamp"]))
        == fields["challenge"]
    )


def test_the_challenge_binds_the_device_and_the_timestamp():
    base = presence.heartbeat_challenge("dev-1", 1750000000)
    assert base != presence.heartbeat_challenge("dev-2", 1750000000)
    assert base != presence.heartbeat_challenge("dev-1", 1750000001)


def test_a_device_seen_inside_the_window_is_online():
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    device = _device(last_seen_at=now - timedelta(seconds=30))
    assert presence.derived_status(device, now) == "online"


def test_the_window_edge_is_online_and_one_second_past_it_is_offline():
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    at_edge = _device(last_seen_at=now - presence.ONLINE_WINDOW)
    just_past = _device(last_seen_at=now - presence.ONLINE_WINDOW - timedelta(seconds=1))
    assert presence.derived_status(at_edge, now) == "online"
    assert presence.derived_status(just_past, now) == "offline"


def test_a_device_never_seen_is_offline():
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    assert presence.derived_status(_device(last_seen_at=None), now) == "offline"


def test_an_unclaimed_device_is_pending():
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    pending = _device(approved=False, owner_user_id=None)
    assert presence.derived_status(pending, now) == "pending"


def test_a_revoked_device_is_offline_not_pending():
    """Revoke clears ``approved`` and keeps the owner; it must not read as a
    device waiting to be paired."""
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    revoked = _device(approved=False, last_seen_at=now)
    assert presence.derived_status(revoked, now) == "offline"


def test_a_naive_last_seen_is_read_as_utc():
    """SQLite hands back naive datetimes; presence must not raise on one."""
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    device = _device(last_seen_at=datetime(2026, 7, 1, 11, 59, 30))
    assert presence.derived_status(device, now) == "online"


def test_the_freshness_window_is_the_signed_request_window():
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    assert presence.heartbeat_timestamp_fresh(int(now.timestamp()), now)
    assert not presence.heartbeat_timestamp_fresh(int(now.timestamp()) - 600, now)


def test_each_guard_is_its_own_memory():
    assert presence.replay_guard() is not presence.replay_guard()
