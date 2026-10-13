"""Every plain HTTP API the SPA fetches must admit its desktop host's bearer.

Discover calls from SPA source and handlers from the app config, so adding a
controller or moving a call cannot leave it outside a handwritten route list.
Bridge-signed and public APIs are not SPA calls; administrator routes retain
their separate session and permission guards.
"""

from __future__ import annotations

import importlib
import re
from pathlib import Path

import pytest
import yaml
from litestar.handlers import HTTPRouteHandler
from skrift.auth.guards import Permission, auth_guard

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


def _configured_routes() -> dict[str, list]:
    config = yaml.safe_load((SKRIFTAPP_DIR / "app.yaml").read_text())
    routes = {}
    for controller_name in config["controllers"]:
        module_name, class_name = controller_name.split(":")
        controller = getattr(importlib.import_module(module_name), class_name)(owner=None)
        for handler in controller.get_route_handlers():
            if isinstance(handler, HTTPRouteHandler):
                guards = [*(controller.guards or []), *(handler.guards or [])]
                for path in handler.paths:
                    full_path = f"{controller.path.rstrip('/')}/{path.lstrip('/')}"
                    routes.setdefault(_path_shape(full_path), []).append(guards)
    return routes


@pytest.mark.parametrize("path", _spa_api_paths())
def test_every_spa_api_route_accepts_desktop_authentication(path):
    routes = _configured_routes()
    shape = _path_shape(path)
    assert shape in routes, f"SPA calls an unregistered API: {path}"
    for guards in routes[shape]:
        assert build_auth_guard in guards, f"desktop authentication missing: {path}"
        assert auth_guard not in guards, f"cookie-only guard blocks desktop: {path}"


def test_administrator_routes_keep_session_and_administrator_permission():
    routes = _configured_routes()
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
