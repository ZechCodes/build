"""Probe endpoints for orchestrators. ``GET /`` belongs to Skrift's CMS and 404s
until a home page exists, so kubelet probes must target routes that are always
present and unauthenticated.

``/healthz`` is liveness: database-free, so a database outage never has the
kubelet restart a process that would come straight back to the same outage.

``/readyz`` is readiness, and sticky: it answers 503 until this process has
reached the database once, then 200 for the life of the process. A rolling
deploy moves traffic to a new pod only once it has shown it can reach the
database. A later outage leaves the pod in the route: the one replica also serves
the pages that never touch the database (the landing page, docs, installers),
and those stay up while the app's own pages fail, exactly as they did under
``/healthz`` readiness."""

from __future__ import annotations

from litestar import Controller, Request, get
from litestar.exceptions import HTTPException
from litestar.status_codes import HTTP_503_SERVICE_UNAVAILABLE
from sqlalchemy import text
from sqlalchemy.ext.asyncio import AsyncSession


class HealthController(Controller):
    path = "/healthz"

    @get("/", include_in_schema=False)
    async def healthz(self) -> dict:
        return {"status": "ok"}


class ReadinessController(Controller):
    path = "/readyz"

    @get("/", include_in_schema=False)
    async def readyz(self, request: Request, db_session: AsyncSession) -> dict:
        if not getattr(request.app.state, "reached_database", False):
            try:
                await db_session.execute(text("SELECT 1"))
            except Exception as error:  # any failure means this pod cannot serve yet
                raise HTTPException(
                    status_code=HTTP_503_SERVICE_UNAVAILABLE, detail="database unreachable"
                ) from error
            request.app.state.reached_database = True
        return {"status": "ok"}
