"""``POST /api/transport/report`` end to end: a report signed by an approved
device's identity key lands as one session row; anything else is refused."""

from __future__ import annotations

import base64
from collections.abc import AsyncIterator, Iterator
from datetime import datetime, timezone
from uuid import uuid4

import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
from litestar import Litestar
from litestar.di import Provide
from litestar.status_codes import (
    HTTP_200_OK,
    HTTP_400_BAD_REQUEST,
    HTTP_401_UNAUTHORIZED,
)
from litestar.testing import TestClient
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from sqlalchemy.pool import StaticPool

from buildapp import transport_controller
from buildapp.db_test_support import IN_MEMORY_DATABASE_URL
from buildapp.models import Device, TransportSession
from buildapp.transport_controller import REPORT_ROUTE_PATH, TransportController
from buildapp.transport_report import NO_PATH, report_challenge



def _ed25519_pair() -> tuple[Ed25519PrivateKey, str]:
    private_key = Ed25519PrivateKey.generate()
    public_b64 = base64.b64encode(
        private_key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
    ).decode("ascii")
    return private_key, public_b64


def _sign(private_key: Ed25519PrivateKey, message: str) -> str:
    return base64.b64encode(private_key.sign(message.encode("utf-8"))).decode("ascii")


class Bridge:
    """A paired device and the key it signs reports with."""

    def __init__(self, device: Device, private_key: Ed25519PrivateKey):
        self.device = device
        self.private_key = private_key

    def report(self, session_id: str, event: str, path: str = NO_PATH, timestamp: int | None = None) -> dict:
        timestamp = timestamp if timestamp is not None else int(datetime.now(timezone.utc).timestamp())
        challenge = report_challenge(str(self.device.id), session_id, event, path, timestamp)
        return {
            "device_id": str(self.device.id),
            "session_id": session_id,
            "event": event,
            "path": path,
            "timestamp": timestamp,
            "signature_b64": _sign(self.private_key, challenge),
        }


@pytest.fixture()
def client() -> Iterator[TestClient]:
    engine = create_async_engine(IN_MEMORY_DATABASE_URL, poolclass=StaticPool)
    make_session = async_sessionmaker(engine, expire_on_commit=False)

    async def provide_db_session() -> AsyncIterator[AsyncSession]:
        async with make_session() as session:
            yield session

    async def create_tables(app: Litestar) -> None:
        async with engine.begin() as connection:
            await connection.run_sync(Device.__table__.create)
            await connection.run_sync(TransportSession.__table__.create)

    async def dispose_engine(app: Litestar) -> None:
        await engine.dispose()

    transport_controller.reset_replay_guard_for_tests()
    app = Litestar(
        route_handlers=[TransportController],
        dependencies={"db_session": Provide(provide_db_session)},
        on_startup=[create_tables],
        on_shutdown=[dispose_engine],
    )
    app.state.make_session = make_session
    with TestClient(app=app) as test_client:
        yield test_client


def paired_bridge(client: TestClient, *, approved: bool = True) -> Bridge:
    private_key, public_b64 = _ed25519_pair()
    device = Device(
        id=uuid4(),
        name="laptop",
        owner_user_id=uuid4() if approved else None,
        identity_public_key_b64=public_b64,
        transport_public_key_b64="x25519-public",
        approved=approved,
        status="online",
    )

    async def store() -> None:
        async with client.app.state.make_session() as session:
            session.add(device)
            await session.commit()

    with client.portal() as portal:
        portal.call(store)
    return Bridge(device, private_key)


def rows(client: TestClient) -> list[TransportSession]:
    async def read() -> list[TransportSession]:
        async with client.app.state.make_session() as session:
            return list((await session.execute(select(TransportSession))).scalars())

    with client.portal() as portal:
        return portal.call(read)


def test_a_signed_trail_becomes_one_session_row(client):
    bridge = paired_bridge(client)
    for event, path in [("minted", NO_PATH), ("carrying", "turn"), ("fell_back", NO_PATH), ("ended", NO_PATH)]:
        response = client.post(REPORT_ROUTE_PATH, json=bridge.report("sess-1", event, path))
        assert response.status_code == HTTP_200_OK, response.text
    (row,) = rows(client)
    assert (row.session_id, row.device_id, row.owner_user_id) == (
        "sess-1",
        bridge.device.id,
        bridge.device.owner_user_id,
    )
    assert (row.first_path, row.current_path, row.turn_count, row.fell_back_count) == (
        "turn",
        "relay",
        1,
        1,
    )
    assert row.minted_at is not None and row.ended_at is not None


def test_a_report_from_an_unapproved_device_is_refused(client):
    bridge = paired_bridge(client, approved=False)
    response = client.post(REPORT_ROUTE_PATH, json=bridge.report("sess-1", "minted"))
    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert rows(client) == []


def test_a_report_signed_by_another_key_is_refused(client):
    bridge = paired_bridge(client)
    forged = bridge.report("sess-1", "minted")
    other, _ = _ed25519_pair()
    forged["signature_b64"] = _sign(other, report_challenge(forged["device_id"], "sess-1", "minted", NO_PATH, forged["timestamp"]))
    response = client.post(REPORT_ROUTE_PATH, json=forged)
    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert rows(client) == []


def test_a_tampered_field_breaks_the_signature(client):
    bridge = paired_bridge(client)
    report = bridge.report("sess-1", "carrying", "direct")
    report["path"] = "turn"  # a direct session cannot be re-labelled as billed
    response = client.post(REPORT_ROUTE_PATH, json=report)
    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert rows(client) == []


def test_a_stale_report_is_refused(client):
    bridge = paired_bridge(client)
    stale = bridge.report("sess-1", "minted", timestamp=1_600_000_000)
    response = client.post(REPORT_ROUTE_PATH, json=stale)
    assert response.status_code == HTTP_401_UNAUTHORIZED


def test_a_replayed_report_is_refused_and_counted_once(client):
    bridge = paired_bridge(client)
    report = bridge.report("sess-1", "carrying", "turn")
    assert client.post(REPORT_ROUTE_PATH, json=report).status_code == HTTP_200_OK
    assert client.post(REPORT_ROUTE_PATH, json=report).status_code == HTTP_401_UNAUTHORIZED
    (row,) = rows(client)
    assert row.turn_count == 1


def test_unknown_words_and_malformed_bodies_are_bad_requests(client):
    bridge = paired_bridge(client)
    response = client.post(REPORT_ROUTE_PATH, json=bridge.report("sess-1", "teleported"))
    assert response.status_code == HTTP_400_BAD_REQUEST
    response = client.post(REPORT_ROUTE_PATH, json=[1, 2, 3])
    assert response.status_code == HTTP_400_BAD_REQUEST
    assert rows(client) == []


def test_the_route_is_device_signed_not_session_guarded():
    handlers = [h for h in vars(TransportController).values() if hasattr(h, "guards")]
    (handler,) = handlers
    assert not handler.guards, "a bridge has no browser session; the signature is the guard"
