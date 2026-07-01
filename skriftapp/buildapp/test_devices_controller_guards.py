"""Guard-wiring tests: every route on DevicesController must carry the right
reusable guard — internal routes the shared-secret guard, browser routes
``auth_guard`` — so no route relies on inline auth checks."""

from __future__ import annotations

from litestar.handlers import HTTPRouteHandler

from skrift.auth.guards import auth_guard

from buildapp.devices_controller import DevicesController
from buildapp.internal_auth import internal_auth_guard


def _route_handlers() -> list[HTTPRouteHandler]:
    return [
        handler
        for handler in vars(DevicesController).values()
        if isinstance(handler, HTTPRouteHandler)
    ]


def _paths(handler: HTTPRouteHandler) -> list[str]:
    return list(handler.paths)


def test_internal_routes_use_internal_auth_guard():
    internal_handlers = [
        h for h in _route_handlers() if any(p.startswith("/internal/") for p in _paths(h))
    ]
    assert len(internal_handlers) == 3  # device, gateway-token, status
    for handler in internal_handlers:
        assert internal_auth_guard in (handler.guards or []), _paths(handler)


def test_browser_routes_use_auth_guard():
    browser_paths = {
        "/api/devices",
        "/api/devices/lookup",
        "/api/devices/approve",
        "/api/devices/{device_id:uuid}/revoke",
        "/api/gateway-token",
    }
    for handler in _route_handlers():
        if browser_paths & set(_paths(handler)):
            assert auth_guard in (handler.guards or []), _paths(handler)


def test_no_localhost_only_inline_check_remains():
    assert not hasattr(
        __import__("buildapp.devices_controller", fromlist=["_require_localhost"]),
        "_require_localhost",
    )
