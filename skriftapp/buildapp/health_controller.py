"""Probe endpoints for orchestrators. ``GET /`` belongs to Skrift's CMS and 404s
until a home page exists, so kubelet probes must target routes that are always
present and unauthenticated.

``/healthz`` is liveness: database-free, so a database outage never has the
kubelet restart a process that would come straight back to the same outage.
``/readyz`` is readiness: it answers 200 only when this process can reach the
database, because every page but the static assets needs it. During a rolling
deploy the new pod takes traffic only once ``/readyz`` passes, and the old one is
drained only after that."""

from __future__ import annotations

from litestar import Controller, get
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
    async def readyz(self, db_session: AsyncSession) -> dict:
        try:
            await db_session.execute(text("SELECT 1"))
        except Exception as error:  # any failure means this pod cannot serve
            raise HTTPException(
                status_code=HTTP_503_SERVICE_UNAVAILABLE, detail="database unreachable"
            ) from error
        return {"status": "ok"}
