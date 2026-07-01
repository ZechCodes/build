"""The push service worker must be served at /app/sw.js (root of the SPA scope,
so it can control /app/ pages) and must never be cached immutably — a stale
worker would outlive deploys."""

from __future__ import annotations

from litestar.handlers import HTTPRouteHandler

from buildapp.controllers import BuildController


def test_service_worker_route_exists_at_app_scope_root():
    handlers = [
        handler
        for handler in vars(BuildController).values()
        if isinstance(handler, HTTPRouteHandler) and "/sw.js" in list(handler.paths)
    ]
    assert len(handlers) == 1, "BuildController must serve /app/sw.js"
