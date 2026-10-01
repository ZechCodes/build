"""The bridge-facing status answer tells a pending device from a revoked one and
from one the api has never heard of, so a bridge holding a stale approval can
say which and pair again (#317).

The route is public and keyed by the bridge's own random UUID, so the answer
for an id nobody registered is the same whoever asks; and an owner is named
only while the device is approved."""

from __future__ import annotations

from collections.abc import Iterator
from uuid import UUID, uuid4

import pytest
from litestar.testing import TestClient
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import desktop_auth, devices_controller, pairing_crypto
from buildapp.db_test_support import add_member, asgi_app, in_memory_session_maker
from buildapp.devices_controller import DevicesController
from buildapp.models import Device

USER = uuid4()


async def _seed_membership(session: AsyncSession) -> None:
    await add_member(session, USER)


@pytest.fixture()
def client(monkeypatch) -> Iterator[TestClient]:
    monkeypatch.setattr(desktop_auth, "session_user_id", lambda connection: USER)
    monkeypatch.setattr(devices_controller, "require_user", lambda request: USER)
    app = asgi_app(
        [DevicesController],
        session_maker=in_memory_session_maker(),
        seed=_seed_membership,
    )
    with TestClient(app=app) as test_client:
        yield test_client


def _device(*, approved: bool, owner: UUID | None, code: str | None) -> Device:
    return Device(
        id=uuid4(),
        name="laptop",
        owner_user_id=owner,
        identity_public_key_b64="identity",
        transport_public_key_b64="transport",
        pairing_code_hash=pairing_crypto.hash_code(code) if code else None,
        approved=approved,
    )


def store(client: TestClient, *devices: Device) -> None:
    async def add() -> None:
        async with client.app.state.make_session() as session:
            session.add_all(devices)
            await session.commit()

    with client.portal() as portal:
        portal.call(add)


def status_of(client: TestClient, device_id: UUID) -> dict:
    response = client.get(f"/api/devices/{device_id}/status")
    assert response.status_code == 200, response.text
    return response.json()


def test_an_approved_device_names_its_owner(client):
    device = _device(approved=True, owner=USER, code=None)
    store(client, device)
    assert status_of(client, device.id) == {
        "approved": True,
        "owner_user_id": str(USER),
        "state": "approved",
    }


def test_a_registered_device_waiting_for_its_code_is_pending(client):
    device = _device(approved=False, owner=None, code="ABCD-EFGH")
    store(client, device)
    assert status_of(client, device.id) == {
        "approved": False,
        "owner_user_id": None,
        "state": "pending",
    }


def test_a_device_revoked_in_settings_is_revoked_and_names_no_owner(client):
    device = _device(approved=True, owner=USER, code=None)
    store(client, device)
    assert client.post(f"/api/devices/{device.id}/revoke").is_success
    assert status_of(client, device.id) == {
        "approved": False,
        "owner_user_id": None,
        "state": "revoked",
    }


def test_an_id_the_api_never_saw_is_unknown(client):
    assert status_of(client, uuid4()) == {
        "approved": False,
        "owner_user_id": None,
        "state": "unknown",
    }
