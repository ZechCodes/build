"""Probe endpoint for orchestrators. ``GET /`` belongs to Skrift's CMS and 404s
until a home page exists, so kubelet probes must target a route that is always
present, unauthenticated, and database-free."""

from __future__ import annotations

from litestar import Controller, get


class HealthController(Controller):
    path = "/healthz"

    @get("/", include_in_schema=False)
    async def healthz(self) -> dict:
        return {"status": "ok"}
