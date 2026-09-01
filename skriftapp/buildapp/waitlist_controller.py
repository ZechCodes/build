"""The public, unauthenticated waitlist endpoint the prelaunch landing page posts to.
A duplicate email is an idempotent success, so the response never reveals whether an
address is already on the list."""

from __future__ import annotations

from litestar import Controller, Request, post
from litestar.exceptions import ClientException, SerializationException
from litestar.response import Response
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.models import WaitlistSignup
from buildapp.request_body import require_json_object
from buildapp.waitlist_email import normalize_waitlist_email


class WaitlistController(Controller):
    path = ""

    @post("/api/waitlist", status_code=200)
    async def join(self, request: Request, db_session: AsyncSession) -> Response:
        try:
            parsed_body = await request.json()
        except SerializationException as malformed:
            raise ClientException("request body must be valid JSON") from malformed
        body = require_json_object(parsed_body)
        email_field = body.get("email")
        email = (
            normalize_waitlist_email(email_field)
            if isinstance(email_field, str)
            else None
        )
        if email is None:
            raise ClientException("invalid email address")
        db_session.add(WaitlistSignup(email=email))
        try:
            await db_session.commit()
        except IntegrityError:
            await db_session.rollback()
        return Response({"ok": True})
