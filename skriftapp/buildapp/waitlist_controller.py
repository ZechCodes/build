"""The public, unauthenticated waitlist endpoints: joining the list and leaving it again.
Removal is authorised by the signed token in the unsubscribe link and by nothing else."""

from __future__ import annotations

import asyncio
import os

from litestar import Controller, Request, get, post
from litestar.di import Provide
from litestar.enums import MediaType
from litestar.exceptions import ClientException, SerializationException
from litestar.response import Response
from litestar.status_codes import HTTP_200_OK, HTTP_404_NOT_FOUND
from skrift.config import get_settings
from skrift.lib.email_backends import EmailBackend
from sqlalchemy import delete
from sqlalchemy.exc import IntegrityError
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.email_message import provide_email_backend
from buildapp.models import WaitlistSignup
from buildapp.request_body import require_json_object
from buildapp.unsubscribe_pages import (
    render_confirm_page,
    render_invalid_page,
    render_removed_page,
)
from buildapp.waitlist_address import normalize_waitlist_address
from buildapp.waitlist_mail import (
    WaitlistEmailContext,
    resolve_waitlist_email_context,
    waitlist_signup_email_task,
)
from buildapp.waitlist_unsubscribe_token import (
    UNSUBSCRIBE_PATH_PREFIX,
    read_unsubscribe_token,
)

JOIN_ROUTE_PATH = "/api/waitlist"
UNSUBSCRIBE_ROUTE_PATH = f"{UNSUBSCRIBE_PATH_PREFIX}{{unsubscribe_token:str}}"


def provide_waitlist_email_context() -> WaitlistEmailContext:
    return resolve_waitlist_email_context(get_settings(), os.environ)


async def invalid_token_response() -> Response:
    return Response(
        await asyncio.to_thread(render_invalid_page),
        media_type=MediaType.HTML,
        status_code=HTTP_404_NOT_FOUND,
    )


class WaitlistController(Controller):
    path = ""
    dependencies = {
        "waitlist_email_context": Provide(
            provide_waitlist_email_context, sync_to_thread=False
        ),
        "email_backend": Provide(provide_email_backend, sync_to_thread=False),
    }

    @post(JOIN_ROUTE_PATH, status_code=HTTP_200_OK)
    async def join(
        self,
        request: Request,
        db_session: AsyncSession,
        waitlist_email_context: WaitlistEmailContext,
        email_backend: EmailBackend,
    ) -> Response:
        try:
            parsed_body = await request.json()
        except SerializationException as malformed:
            raise ClientException("request body must be valid JSON") from malformed
        body = require_json_object(parsed_body)
        email_field = body.get("email")
        email = (
            normalize_waitlist_address(email_field)
            if isinstance(email_field, str)
            else None
        )
        if email is None:
            raise ClientException("invalid email address")
        db_session.add(WaitlistSignup(email=email))
        signup_email_task = None
        try:
            await db_session.commit()
        except IntegrityError:
            await db_session.rollback()
        else:
            signup_email_task = waitlist_signup_email_task(
                email_backend=email_backend,
                signup_email=email,
                context=waitlist_email_context,
            )
        return Response({"ok": True}, background=signup_email_task)

    @get(UNSUBSCRIBE_ROUTE_PATH)
    async def unsubscribe_confirm(
        self, unsubscribe_token: str, waitlist_email_context: WaitlistEmailContext
    ) -> Response:
        email = read_unsubscribe_token(
            unsubscribe_token, waitlist_email_context.secret_key
        )
        if email is None:
            return await invalid_token_response()
        return Response(
            await asyncio.to_thread(render_confirm_page, email, unsubscribe_token),
            media_type=MediaType.HTML,
        )

    @post(UNSUBSCRIBE_ROUTE_PATH, status_code=HTTP_200_OK)
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
            return await invalid_token_response()
        await db_session.execute(
            delete(WaitlistSignup).where(WaitlistSignup.email == email)
        )
        await db_session.commit()
        return Response(
            await asyncio.to_thread(render_removed_page), media_type=MediaType.HTML
        )
