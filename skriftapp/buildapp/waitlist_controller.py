"""The public, unauthenticated waitlist endpoint the prelaunch landing page posts to.
A duplicate email is an idempotent success, so the response never reveals whether an
address is already on the list."""

from __future__ import annotations

from litestar import Controller, Request, post
from litestar.exceptions import ClientException
from litestar.response import Response
from sqlalchemy import select
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.models import WaitlistSignup
from buildapp.request_body import require_json_object
from buildapp.waitlist_email import normalize_waitlist_email


class WaitlistController(Controller):
    path = ""

    @post("/api/waitlist", status_code=200)
    async def join(self, request: Request, db_session: AsyncSession) -> Response:
        body = require_json_object(await request.json())
        email = normalize_waitlist_email(str(body.get("email", "")))
        if email is None:
            raise ClientException("invalid email address")
        existing = await db_session.execute(
            select(WaitlistSignup).where(WaitlistSignup.email == email)
        )
        if existing.scalar_one_or_none() is None:
            db_session.add(WaitlistSignup(email=email))
            try:
                await db_session.commit()
            except IntegrityError:
                await db_session.rollback()
        return Response({"ok": True})
