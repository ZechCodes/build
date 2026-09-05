"""An account holds at most MAX_DEVICES_PER_USER approved devices: approving a
pairing code past the cap is refused with a message that says so, and revoking
one frees its slot."""

from __future__ import annotations

from collections.abc import AsyncIterator, Iterator
from uuid import UUID, uuid4

import pytest
from litestar import Litestar
from litestar.di import Provide
from litestar.status_codes import HTTP_409_CONFLICT
from litestar.testing import TestClient
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker, create_async_engine
from sqlalchemy.pool import StaticPool

from buildapp import desktop_auth, devices_controller, pairing_crypto
from buildapp.devices_controller import MAX_DEVICES_PER_USER, DevicesController
from buildapp.models import Device

IN_MEMORY_DATABASE_URL = "sqlite+aiosqlite:///:memory:"
USER = uuid4()


@pytest.fixture()
def client(monkeypatch) -> Iterator[TestClient]:
    # The browser is this one user, without a real Skrift session in the loop:
    # the guard and the handler both resolve the user through these two.
    monkeypatch.setattr(desktop_auth, "session_user_id", lambda connection: USER)
    monkeypatch.setattr(devices_controller, "require_user", lambda request: USER)
    engine = create_async_engine(IN_MEMORY_DATABASE_URL, poolclass=StaticPool)
    make_session = async_sessionmaker(engine, expire_on_commit=False)

    async def provide_db_session() -> AsyncIterator[AsyncSession]:
        async with make_session() as session:
            yield session

    async def create_tables(app: Litestar) -> None:
        async with engine.begin() as connection:
            await connection.run_sync(Device.__table__.create)

    async def dispose_engine(app: Litestar) -> None:
        await engine.dispose()

    app = Litestar(
        route_handlers=[DevicesController],
        dependencies={"db_session": Provide(provide_db_session)},
        on_startup=[create_tables],
        on_shutdown=[dispose_engine],
    )
    app.state.make_session = make_session
    with TestClient(app=app) as test_client:
        yield test_client


def _pending(code: str) -> Device:
    return Device(
        id=uuid4(),
        name=f"device-{code}",
        owner_user_id=None,
        identity_public_key_b64="identity",
        transport_public_key_b64="transport",
        pairing_code_hash=pairing_crypto.hash_code(code),
        approved=False,
        status="pending",
    )


def _approved(owner: UUID) -> Device:
    return Device(
        id=uuid4(),
        name="owned",
        owner_user_id=owner,
        identity_public_key_b64="identity",
        transport_public_key_b64="transport",
        approved=True,
        status="online",
    )


def store(client: TestClient, *devices: Device) -> None:
    async def add() -> None:
        async with client.app.state.make_session() as session:
            session.add_all(devices)
            await session.commit()

    with client.portal() as portal:
        portal.call(add)


def test_the_cap_is_three():
    assert MAX_DEVICES_PER_USER == 3


def test_approving_past_the_cap_is_refused_and_the_device_stays_pending(client):
    store(client, *(_approved(USER) for _ in range(MAX_DEVICES_PER_USER)), _pending("ABCD-EFGH"))
    response = client.post("/api/devices/approve", json={"code": "ABCD-EFGH"})
    assert response.status_code == HTTP_409_CONFLICT
    assert str(MAX_DEVICES_PER_USER) in response.json()["detail"]
    listed = client.get("/api/devices").json()["devices"]
    assert len(listed) == MAX_DEVICES_PER_USER
    # Still pending: the same code approves once a slot is free.
    assert client.post("/api/devices/lookup", json={"code": "ABCD-EFGH"}).is_success


def test_revoking_a_device_frees_its_slot(client):
    owned = [_approved(USER) for _ in range(MAX_DEVICES_PER_USER)]
    store(client, *owned, _pending("ABCD-EFGH"))
    assert client.post(f"/api/devices/{owned[0].id}/revoke").is_success
    response = client.post("/api/devices/approve", json={"code": "ABCD-EFGH"})
    assert response.is_success, response.text
    assert len(client.get("/api/devices").json()["devices"]) == MAX_DEVICES_PER_USER


def test_another_users_devices_do_not_count(client):
    store(client, *(_approved(uuid4()) for _ in range(MAX_DEVICES_PER_USER)), _pending("ABCD-EFGH"))
    assert client.post("/api/devices/approve", json={"code": "ABCD-EFGH"}).is_success
