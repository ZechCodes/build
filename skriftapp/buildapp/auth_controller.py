"""Skrift's auth controller with account creation behind an invite (#314).

Build does not fork Skrift's sign-in: this subclass keeps every route and replaces four
handlers by name, each doing Build's part and then calling Skrift's own handler.

- ``/auth/login`` and ``/auth/{provider}/login`` hand the sign-in page the address of
  the invite this session carries, so the page offers account creation for that address
  only, and none at all without one.
- ``register/options`` refuses any address but the carried invite's, whatever the form
  posts. ``register/complete`` refuses when the address Skrift kept from the options
  step is no longer the carried invite's (another invite opened since, or this one
  revoked or expired), so nothing passes on the strength of an earlier check.
- A completed registration redeems the invite in the same request and sends the new
  member to /app/.

A refusal answers ``invite_required`` with the CSRF token, like Skrift's own refusals.
Skrift's generic ``invalid_request`` for an address that already has an account is
untouched.
"""

from __future__ import annotations

from typing import Annotated

from litestar import Request, get, post
from litestar.params import Parameter
from litestar.response import Redirect, Response
from litestar.response import Template as TemplateResponse
from litestar.status_codes import HTTP_201_CREATED, HTTP_403_FORBIDDEN
from skrift.auth.second_factors.passkey_service import get_primary_passkey_registration_state
from skrift.controllers.auth import AuthController
from skrift.forms.core import CSRF_SESSION_KEY
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import invites
from buildapp.clock import utc_now
from buildapp.invite_pages import APP_PATH
from buildapp.models import Invite
from buildapp.session_auth import session_user_id
from buildapp.signup_invite import admits, carried_invite

INVITE_REQUIRED = "invite_required"
#: The sign-in page's name for the address it offers an account for.
INVITE_EMAIL_CONTEXT = "invite_email"


def invite_required(request: Request) -> Response:
    return Response(
        {"error": INVITE_REQUIRED, "csrf_token": request.session.get(CSRF_SESSION_KEY, "")},
        status_code=HTTP_403_FORBIDDEN,
    )


async def with_invite_email(
    response: Redirect | TemplateResponse, request: Request, db_session: AsyncSession
) -> Redirect | TemplateResponse:
    if isinstance(response, TemplateResponse):
        invite = await carried_invite(request, db_session, utc_now())
        response.context[INVITE_EMAIL_CONTEXT] = invite.email if invite else None
    return response


class BuildAuthController(AuthController):
    @get("/login")
    async def login_page(
        self,
        request: Request,
        db_session: AsyncSession,
        next_url: Annotated[str | None, Parameter(query="next")] = None,
    ) -> TemplateResponse:
        response = await AuthController.login_page.fn(self, request, next_url)
        return await with_invite_email(response, request, db_session)

    @get("/{provider:str}/login")
    async def oauth_login(
        self,
        request: Request,
        db_session: AsyncSession,
        provider: str,
        next_url: Annotated[str | None, Parameter(query="next")] = None,
    ) -> Redirect | TemplateResponse:
        response = await AuthController.oauth_login.fn(self, request, provider, next_url)
        return await with_invite_email(response, request, db_session)

    @post("/{provider:str}/register/options")
    async def begin_primary_method_registration(
        self, request: Request, db_session: AsyncSession, provider: str
    ) -> Response:
        email = str((await request.form()).get("email", ""))
        if not admits(await carried_invite(request, db_session, utc_now()), email):
            return invite_required(request)
        return await AuthController.begin_primary_method_registration.fn(
            self, request, db_session, provider
        )

    @post("/{provider:str}/register/complete")
    async def complete_primary_method_registration(
        self, request: Request, db_session: AsyncSession, provider: str
    ) -> Response:
        invite = await carried_invite(request, db_session, utc_now())
        signup = get_primary_passkey_registration_state(request)
        # No registration under way is Skrift's refusal to give, in its own words.
        if signup is not None and not admits(invite, signup.email):
            return invite_required(request)
        response = await AuthController.complete_primary_method_registration.fn(
            self, request, db_session, provider
        )
        if response.status_code == HTTP_201_CREATED and await _redeem(
            request, db_session, invite, signup.email
        ):
            return Response({"ok": True, "redirect": APP_PATH}, status_code=HTTP_201_CREATED)
        return response


async def _redeem(
    request: Request, db_session: AsyncSession, invite: Invite, email: str
) -> bool:
    """Spend the invite on the account Skrift just created and signed in. With a second
    factor still to pass there is no signed-in user yet; the invite link, where Skrift
    sends the person next, redeems it once they are through."""
    user_id = session_user_id(request)
    if user_id is None:
        return False
    await db_session.refresh(invite)
    if not invites.redeem(invite, user_id, email, utc_now()).ok:
        return False
    await db_session.commit()
    return True
