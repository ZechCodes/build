"""Every plain HTTP API the SPA fetches must admit its desktop host's bearer.

Discover calls from SPA source and handlers from the app config, so adding a
controller or moving a call cannot leave it outside a handwritten route list.
Bridge-signed and public APIs are not SPA calls; administrator routes retain
their separate session and permission guards.
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest
import yaml
from litestar import Litestar
from litestar.handlers import HTTPRouteHandler
from skrift.auth.guards import Permission, auth_guard
from skrift.config import get_settings

from buildapp.desktop_auth import build_auth_guard

SKRIFTAPP_DIR = Path(__file__).resolve().parents[1]
SPA_SOURCE = SKRIFTAPP_DIR.parent / "spa" / "src"
API_FETCH = re.compile(r"\bfetch\(\s*[\"'`](/api/[^\"'`]+)[\"'`]")
PATH_PARAMETER = re.compile(r"\$?\{[^}]+\}")


def _path_shape(path: str) -> str:
    return PATH_PARAMETER.sub("{}", path)


def _spa_api_paths() -> list[str]:
    paths = {
        match.group(1)
        for file in SPA_SOURCE.rglob("*.js")
        for match in API_FETCH.finditer(file.read_text())
    }
    assert paths, "SPA API discovery found no calls"
    return sorted(paths)


@pytest.fixture()
def configured_routes(skrift_app) -> dict[str, list]:
    # The full-app fixture uses the dev config; keep its controller inventory
    # identical to production so a newly configured controller cannot be missed.
    config = yaml.safe_load((SKRIFTAPP_DIR / "app.yaml").read_text())
    assert get_settings().controllers == config["controllers"]
    return _configured_routes(skrift_app)


def _configured_routes(app) -> dict[str, list]:
    # Skrift wraps the Litestar app in storage middleware.
    while not isinstance(app, Litestar):
        app = app.app
    routes = {}
    for route in app.routes:
        for handler in getattr(route, "route_handlers", []):
            # Litestar's generated preflight handler was never a configured
            # controller handler and does not carry endpoint guards.
            if (
                isinstance(handler, HTTPRouteHandler)
                and getattr(handler.fn, "__qualname__", "")
                != "HTTPRoute.create_options_handler.<locals>.options_handler"
            ):
                guards = [
                    guard
                    for layer in handler.ownership_layers
                    for guard in (layer.guards or [])
                ]
                routes.setdefault(_path_shape(route.path), []).append(guards)
    return routes


@pytest.mark.parametrize("path", _spa_api_paths())
def test_every_spa_api_route_accepts_desktop_authentication(path, configured_routes):
    routes = configured_routes
    shape = _path_shape(path)
    assert shape in routes, f"SPA calls an unregistered API: {path}"
    for guards in routes[shape]:
        assert build_auth_guard in guards, f"desktop authentication missing: {path}"
        assert auth_guard not in guards, f"cookie-only guard blocks desktop: {path}"


def test_administrator_routes_keep_session_and_administrator_permission(configured_routes):
    routes = configured_routes
    paths = [path for path in routes if path.startswith("/admin/")]
    # Skrift's own admin has different per-resource permissions; pin Build's
    # administrator pages and its invitation API, which the SPA does not call.
    build_admin_paths = {
        "/admin/transport",
        "/admin/invites",
        "/admin/invites/open-link",
        "/admin/invites/{}/revoke",
        "/admin/waitlist",
        "/admin/waitlist/{}/invite",
        "/api/invites",
    }
    assert build_admin_paths - {"/api/invites"} <= set(paths)
    for path in build_admin_paths:
        for guards in routes[path]:
            assert auth_guard in guards, path
            assert any(
                isinstance(guard, Permission) and guard.permission == "administrator"
                for guard in guards
            ), path
            assert build_auth_guard not in guards, path
