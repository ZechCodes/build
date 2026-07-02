"""The k8s probes need a route that exists in every app state. ``GET /`` is
Skrift's CMS home page — it 404s on a fresh site, which fails readiness (traefik
drops every route) and liveness kills the container in a loop. ``/healthz`` must
therefore be unauthenticated, database-free, and always 200."""

from __future__ import annotations

import asyncio

from litestar.handlers import HTTPRouteHandler

from buildapp.health_controller import HealthController


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
