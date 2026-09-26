"""The k8s probes need a route that exists in every app state. ``GET /`` is
Skrift's CMS home page — it 404s on a fresh site, which fails readiness (traefik
drops every route) and liveness kills the container in a loop. ``/healthz`` must
therefore be unauthenticated, database-free, and always 200."""

from __future__ import annotations

import asyncio
from collections.abc import AsyncIterator

from litestar import Litestar
from litestar.di import Provide
from litestar.handlers import HTTPRouteHandler
from litestar.testing import TestClient
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.db_test_support import asgi_app, in_memory_session_maker
from buildapp import health_controller
from buildapp.health_controller import HealthController, ReadinessController


def _healthz_handler() -> HTTPRouteHandler:
    return HealthController.healthz


def test_healthz_route_is_registered_at_the_expected_path():
    handler = _healthz_handler()
    assert isinstance(handler, HTTPRouteHandler)
    assert HealthController.path == "/healthz"
    assert "/" in handler.paths


def test_healthz_has_no_guards_and_no_db_dependency():
    handler = _healthz_handler()
    assert not handler.guards, "healthz must stay unauthenticated for kubelet probes"
    signature_params = handler.fn.__code__.co_varnames[: handler.fn.__code__.co_argcount]
    assert "db_session" not in signature_params, "healthz must not touch the database"


def test_healthz_reports_ok():
    payload = asyncio.run(HealthController.healthz.fn(None))
    assert payload == {"status": "ok"}


# ``/readyz`` gates traffic during a rolling deploy: it answers 503 until this
# process has reached the database once, then 200 for the life of the process,
# so a later database outage never takes the only pod (and with it the static
# site) out of the route. It needs no signed-in user and no guard.


def _ready_client(session_maker):
    return TestClient(asgi_app([ReadinessController], session_maker=session_maker))


def _app_over(provide) -> Litestar:
    return Litestar(
        route_handlers=[ReadinessController],
        dependencies={"db_session": Provide(provide)},
    )


def test_readyz_answers_200_when_the_database_answers():
    with _ready_client(in_memory_session_maker()) as client:
        response = client.get("/readyz")
    assert response.status_code == 200
    assert response.json() == {"status": "ok"}


def test_readyz_answers_503_until_the_database_has_answered_once():
    with TestClient(_app_over(_unreachable_session)) as client:
        assert client.get("/readyz").status_code == 503
        assert client.get("/readyz").status_code == 503


def test_readyz_stays_200_once_the_database_has_answered():
    session_maker = in_memory_session_maker()
    database_up = [True]

    async def sometimes_reachable() -> AsyncIterator[AsyncSession]:
        if database_up[0]:
            async with session_maker() as session:
                yield session
        else:
            async with AsyncSession() as session:
                yield session

    with TestClient(_app_over(sometimes_reachable)) as client:
        assert client.get("/readyz").status_code == 200
        database_up[0] = False
        assert client.get("/readyz").status_code == 200


def test_readyz_gives_up_on_a_hung_database_within_its_deadline(monkeypatch):
    monkeypatch.setattr(health_controller, "READINESS_QUERY_TIMEOUT_S", 0.05)
    with TestClient(_app_over(_hung_session)) as client:
        response = client.get("/readyz")
    assert response.status_code == 503


def test_readyz_has_no_guards():
    assert not ReadinessController.readyz.guards


async def _unreachable_session() -> AsyncIterator[AsyncSession]:
    """A real session bound to no database: every statement fails, as it does
    for a pod whose database is gone."""
    async with AsyncSession() as session:
        yield session


class _HungSession(AsyncSession):
    """A session whose statements never return, like a connect to a database
    that accepts the socket and never answers."""

    async def execute(self, *_args, **_kwargs):
        await asyncio.sleep(3600)


async def _hung_session() -> AsyncIterator[AsyncSession]:
    async with _HungSession() as session:
        yield session
