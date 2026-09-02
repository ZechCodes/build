"""The public, unauthenticated waitlist endpoints: joining the list and leaving it again.
A duplicate email is an idempotent success, so the response never reveals whether an
address is already on the list. Removal is authorised by the signed token in the
unsubscribe link and by nothing else — no session, no guard, no CSRF field — because
anyone holding the token already holds the email it names."""

from __future__ import annotations

import os

from litestar import Controller, Request, get, post
from litestar.di import Provide
from litestar.exceptions import ClientException, SerializationException
from litestar.response import Response
from skrift.config import get_settings
from sqlalchemy import delete
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.models import WaitlistSignup
from buildapp.request_body import require_json_object
from buildapp.unsubscribe_pages import (
    render_confirm_page,
    render_invalid_page,
    render_removed_page,
)
from buildapp.waitlist_email import normalize_waitlist_email
from buildapp.waitlist_mail import WaitlistEmailContext, resolve_waitlist_email_context
from buildapp.waitlist_unsubscribe_token import (
    UNSUBSCRIBE_PATH_PREFIX,
    read_unsubscribe_token,
)

UNSUBSCRIBE_ROUTE_PATH = f"{UNSUBSCRIBE_PATH_PREFIX}{{unsubscribe_token:str}}"
HTML_MEDIA_TYPE = "text/html"
NOT_FOUND_STATUS = 404


def provide_waitlist_email_context() -> WaitlistEmailContext:
    return resolve_waitlist_email_context(get_settings(), os.environ)


class WaitlistController(Controller):
    path = ""
    dependencies = {
        "waitlist_email_context": Provide(
            provide_waitlist_email_context, sync_to_thread=False
        )
    }

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

    @get(UNSUBSCRIBE_ROUTE_PATH)
    async def unsubscribe_confirm(
        self, unsubscribe_token: str, waitlist_email_context: WaitlistEmailContext
    ) -> Response:
        email = read_unsubscribe_token(
            unsubscribe_token, waitlist_email_context.secret_key
        )
        if email is None:
            return Response(
                render_invalid_page(),
                media_type=HTML_MEDIA_TYPE,
                status_code=NOT_FOUND_STATUS,
            )
        return Response(
            render_confirm_page(email, unsubscribe_token), media_type=HTML_MEDIA_TYPE
        )

    @post(UNSUBSCRIBE_ROUTE_PATH, status_code=200)
    async def unsubscribe_remove(
        self,
        unsubscribe_token: str,
        waitlist_email_context: WaitlistEmailContext,
        db_session: AsyncSession,
    ) -> Response:
        email = read_unsubscribe_token(
            unsubscribe_token, waitlist_email_context.secret_key
        )
        if email is None:
            return Response(
                render_invalid_page(),
                media_type=HTML_MEDIA_TYPE,
                status_code=NOT_FOUND_STATUS,
            )
        await db_session.execute(
            delete(WaitlistSignup).where(WaitlistSignup.email == email)
        )
        await db_session.commit()
        return Response(render_removed_page(), media_type=HTML_MEDIA_TYPE)
