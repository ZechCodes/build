"""Guard-wiring tests for PushController: browser routes carry the reusable
``auth_guard``; the bridge-facing notify route carries no session guard (it is
authenticated by the device's Ed25519 signature, mirroring device registration)."""

from __future__ import annotations

from litestar.handlers import HTTPRouteHandler

from skrift.auth.guards import auth_guard

from buildapp.push_controller import PushController


def _route_handlers() -> list[HTTPRouteHandler]:
    return [
        handler
        for handler in vars(PushController).values()
        if isinstance(handler, HTTPRouteHandler)
    ]


def _paths(handler: HTTPRouteHandler) -> list[str]:
    return list(handler.paths)


def test_browser_routes_use_auth_guard():
    browser_paths = {
        "/api/push/subscribe",
        "/api/push/unsubscribe",
        "/api/push/vapid-public-key",
    }
    guarded = set()
    for handler in _route_handlers():
        matched = browser_paths & set(_paths(handler))
        if matched:
            assert auth_guard in (handler.guards or []), _paths(handler)
            guarded |= matched
    assert guarded == browser_paths


def test_notify_route_is_device_signature_authenticated_not_session():
    notify_handlers = [
        h for h in _route_handlers() if "/api/push/notify" in _paths(h)
    ]
    assert len(notify_handlers) == 1
    assert auth_guard not in (notify_handlers[0].guards or [])
