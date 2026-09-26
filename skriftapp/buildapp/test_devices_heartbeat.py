"""``POST /api/devices/heartbeat`` end to end: a heartbeat signed by an approved
device's identity key sets ``last_seen_at`` and nothing else, anything else is
refused, and the listing reports a status derived from that stamp — the api
stores no status at all (``planning/v2/Strict P2P Transport Spec.md`` rule 6)."""

from __future__ import annotations

import base64
from collections.abc import Iterator
from datetime import datetime, timedelta, timezone
from uuid import UUID, uuid4

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
from litestar.status_codes import (
    HTTP_200_OK,
    HTTP_400_BAD_REQUEST,
    HTTP_401_UNAUTHORIZED,
)
from litestar.testing import TestClient
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import desktop_auth, devices_controller, presence
from buildapp.db_test_support import add_member, asgi_app, in_memory_session_maker
from buildapp.devices_controller import HEARTBEAT_ROUTE_PATH, DevicesController
from buildapp.models import Device

USER = uuid4()


async def _seed_membership(session: AsyncSession) -> None:
    await add_member(session, USER)


@pytest.fixture()
def client(monkeypatch) -> Iterator[TestClient]:
    devices_controller.reset_replay_guard_for_tests()
    monkeypatch.setattr(desktop_auth, "session_user_id", lambda connection: USER)
    monkeypatch.setattr(devices_controller, "require_user", lambda request: USER)
    app = asgi_app(
        [DevicesController],
        session_maker=in_memory_session_maker(),
        seed=_seed_membership,
    )
    with TestClient(app=app) as test_client:
        yield test_client


def _ed25519_pair() -> tuple[Ed25519PrivateKey, str]:
    private_key = Ed25519PrivateKey.generate()
    public_b64 = base64.b64encode(
        private_key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
    ).decode("ascii")
    return private_key, public_b64


def _sign(private_key: Ed25519PrivateKey, message: str) -> str:
    return base64.b64encode(private_key.sign(message.encode("utf-8"))).decode("ascii")


class Bridge:
    """A paired device and the key it signs heartbeats with."""

    def __init__(self, device: Device, private_key: Ed25519PrivateKey):
        self.device = device
        self.private_key = private_key

    def heartbeat(self, timestamp: int | None = None) -> dict:
        if timestamp is None:
            timestamp = int(datetime.now(timezone.utc).timestamp())
        challenge = presence.heartbeat_challenge(str(self.device.id), timestamp)
        return {
            "device_id": str(self.device.id),
            "timestamp": timestamp,
            "signature_b64": _sign(self.private_key, challenge),
        }


def paired_bridge(
    client: TestClient, *, approved: bool = True, owner: UUID | None = USER
) -> Bridge:
    private_key, public_b64 = _ed25519_pair()
    device = Device(
        id=uuid4(),
        name="laptop",
        owner_user_id=owner if approved else None,
        identity_public_key_b64=public_b64,
        transport_public_key_b64="x25519-public",
        approved=approved,
    )

    async def store() -> None:
        async with client.app.state.make_session() as session:
            session.add(device)
            await session.commit()

    with client.portal() as portal:
        portal.call(store)
    return Bridge(device, private_key)


def stored(client: TestClient, device_id: UUID) -> Device:
    async def read() -> Device:
        async with client.app.state.make_session() as session:
            return (
                await session.execute(select(Device).where(Device.id == device_id))
            ).scalar_one()

    with client.portal() as portal:
        return portal.call(read)


def set_last_seen(client: TestClient, device_id: UUID, moment: datetime | None) -> None:
    async def write() -> None:
        async with client.app.state.make_session() as session:
            device = (
                await session.execute(select(Device).where(Device.id == device_id))
            ).scalar_one()
            device.last_seen_at = moment
            await session.commit()

    with client.portal() as portal:
        portal.call(write)


def test_a_signed_heartbeat_stamps_last_seen_and_writes_nothing_else(client):
    """``last_seen_at`` is the whole write, and the listing is online because it
    derives status from the stamp."""
    bridge = paired_bridge(client)
    set_last_seen(client, bridge.device.id, None)

    response = client.post(HEARTBEAT_ROUTE_PATH, json=bridge.heartbeat())

    assert response.status_code == HTTP_200_OK, response.text
    assert response.json() == {"ok": True}
    device = stored(client, bridge.device.id)
    assert device.last_seen_at is not None
    (listed,) = client.get("/api/devices").json()["devices"]
    assert listed["status"] == "online"


def test_a_heartbeat_makes_the_listing_report_online(client):
    bridge = paired_bridge(client)
    set_last_seen(client, bridge.device.id, None)
    client.post(HEARTBEAT_ROUTE_PATH, json=bridge.heartbeat())
    (listed,) = client.get("/api/devices").json()["devices"]
    assert listed["status"] == "online"


def test_a_device_that_stopped_heartbeating_reads_offline(client):
    """Nothing writes "offline" when a bridge goes away; the derived window is
    what the SPA sees, so a bridge last seen two minutes ago is offline."""
    bridge = paired_bridge(client)
    set_last_seen(
        client,
        bridge.device.id,
        datetime.now(timezone.utc) - timedelta(minutes=2),
    )
    (listed,) = client.get("/api/devices").json()["devices"]
    assert listed["status"] == "offline"


def test_a_device_that_never_heartbeated_reads_offline(client):
    bridge = paired_bridge(client)
    set_last_seen(client, bridge.device.id, None)
    (listed,) = client.get("/api/devices").json()["devices"]
    assert listed["status"] == "offline"


def test_a_stale_heartbeat_is_refused(client):
    bridge = paired_bridge(client)
    stale = int(datetime.now(timezone.utc).timestamp()) - 600
    response = client.post(HEARTBEAT_ROUTE_PATH, json=bridge.heartbeat(stale))
    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert stored(client, bridge.device.id).last_seen_at is None


def test_a_replayed_heartbeat_is_refused(client):
    bridge = paired_bridge(client)
    body = bridge.heartbeat()
    assert client.post(HEARTBEAT_ROUTE_PATH, json=body).status_code == HTTP_200_OK
    assert (
        client.post(HEARTBEAT_ROUTE_PATH, json=body).status_code == HTTP_401_UNAUTHORIZED
    )


def test_a_heartbeat_from_an_unapproved_device_is_refused(client):
    bridge = paired_bridge(client, approved=False)
    response = client.post(HEARTBEAT_ROUTE_PATH, json=bridge.heartbeat())
    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert stored(client, bridge.device.id).last_seen_at is None


def test_a_heartbeat_for_an_unknown_device_is_refused(client):
    bridge = paired_bridge(client)
    forged = bridge.heartbeat()
    forged["device_id"] = str(uuid4())
    response = client.post(HEARTBEAT_ROUTE_PATH, json=forged)
    assert response.status_code == HTTP_401_UNAUTHORIZED


def test_a_heartbeat_signed_by_another_key_is_refused(client):
    bridge = paired_bridge(client)
    forged = bridge.heartbeat()
    other, _ = _ed25519_pair()
    forged["signature_b64"] = _sign(
        other, presence.heartbeat_challenge(forged["device_id"], forged["timestamp"])
    )
    response = client.post(HEARTBEAT_ROUTE_PATH, json=forged)
    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert stored(client, bridge.device.id).last_seen_at is None


def test_a_tampered_timestamp_breaks_the_signature(client):
    bridge = paired_bridge(client)
    body = bridge.heartbeat()
    body["timestamp"] = body["timestamp"] - 1
    assert (
        client.post(HEARTBEAT_ROUTE_PATH, json=body).status_code == HTTP_401_UNAUTHORIZED
    )


def test_a_malformed_heartbeat_is_a_client_error(client):
    assert (
        client.post(HEARTBEAT_ROUTE_PATH, json={"device_id": "not-a-uuid"}).status_code
        == HTTP_400_BAD_REQUEST
    )
    assert (
        client.post(
            HEARTBEAT_ROUTE_PATH,
            json={"device_id": str(uuid4()), "timestamp": "soon", "signature_b64": "x"},
        ).status_code
        == HTTP_400_BAD_REQUEST
    )


def test_the_heartbeat_needs_no_browser_session(client):
    """The route is bridge-facing: no session guard, only the signature."""
    bridge = paired_bridge(client)
    assert (
        client.post(HEARTBEAT_ROUTE_PATH, json=bridge.heartbeat()).status_code
        == HTTP_200_OK
    )
