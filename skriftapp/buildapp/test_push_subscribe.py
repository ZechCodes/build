"""Subscribing stores only an endpoint on a known push service: the server POSTs
to every stored endpoint later, so the route is where an arbitrary host (an
internal service, a metadata address) is turned away (#191)."""

from __future__ import annotations

from collections.abc import Iterator
from uuid import uuid4

import pytest
from litestar.status_codes import HTTP_201_CREATED, HTTP_400_BAD_REQUEST
from litestar.testing import TestClient
from skrift.db.models.push_subscription import PushSubscription
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import desktop_auth, push_controller
from buildapp.db_test_support import add_member, asgi_app, in_memory_session_maker
from buildapp.push_controller import PushController
from buildapp.test_web_push import ACCEPTED_ENDPOINTS, REJECTED_ENDPOINTS

USER = uuid4()


async def _seed_membership(session: AsyncSession) -> None:
    await add_member(session, USER)


@pytest.fixture()
def client(monkeypatch) -> Iterator[TestClient]:
    monkeypatch.setattr(desktop_auth, "session_user_id", lambda connection: USER)
    monkeypatch.setattr(push_controller, "require_user", lambda request: USER)
    app = asgi_app(
        [PushController],
        session_maker=in_memory_session_maker(),
        seed=_seed_membership,
    )
    with TestClient(app=app) as test_client:
        yield test_client


def _subscribe(client: TestClient, endpoint: str):
    return client.post(
        "/api/push/subscribe",
        json={"endpoint": endpoint, "keys": {"p256dh": "pk", "auth": "ak"}},
    )


def _stored(client: TestClient) -> list[str]:
    async def read() -> list[str]:
        async with client.app.state.make_session() as session:
            rows = (await session.execute(select(PushSubscription))).scalars().all()
            return [row.endpoint for row in rows]

    with client.portal() as portal:
        return portal.call(read)


def test_a_push_service_endpoint_is_stored(client):
    for endpoint in ACCEPTED_ENDPOINTS:
        assert _subscribe(client, endpoint).status_code == HTTP_201_CREATED, endpoint
    assert sorted(_stored(client)) == sorted(ACCEPTED_ENDPOINTS)


def test_an_endpoint_off_the_push_services_is_refused_and_not_stored(client):
    for endpoint in REJECTED_ENDPOINTS:
        refused = _subscribe(client, endpoint)
        assert refused.status_code == HTTP_400_BAD_REQUEST, endpoint
    assert _stored(client) == []
