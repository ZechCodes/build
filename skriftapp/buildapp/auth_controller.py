"""Skrift's auth controller with account creation behind an invite (#314).

Build does not fork Skrift's sign-in: this subclass keeps every route and replaces four
handlers by name, each doing Build's part and then calling Skrift's own handler.

- ``/auth/login`` and ``/auth/{provider}/login`` show one view (#315): account creation
  for the address of the invite this session carries, or sign-in. A visitor without an
  open invite only ever gets sign-in; one with an invite gets account creation, or
  sign-in with ``?view=signin``. ``view`` is allow-listed, never echoed, and Skrift's
  ``next`` needs no carrying: Skrift keeps it in the session.
- ``register/options`` refuses any address but the carried invite's, whatever the form
  posts. ``register/complete`` refuses when the address Skrift kept from the options
  step is no longer the carried invite's (another invite opened since, or this one
  revoked or expired), so nothing passes on the strength of an earlier check.
- A completed registration redeems the invite in the same request and sends the new
  member to /app/.

Skrift's own checks come first: Build's applies only to a request Skrift would act on
(a configured passkey method and a valid CSRF token). Anything else goes straight to
Skrift's handler, so an unknown provider is still its 404 and a token-less post its
``invalid_csrf``. Build's refusal answers ``invite_required`` with the CSRF token, like
Skrift's own refusals. Skrift's generic ``invalid_request`` for an address that already
has an account is untouched.

Only passkey registration is behind the invite. Skrift's dummy and OAuth methods create
accounts on their own routes, unguarded: production's ``app.yaml`` configures the passkey
method alone (``test_auth_controller`` pins that), while ``app.dev.yaml`` and
``app.mail.yaml`` configure the dummy method, so anyone reaching a dev or mail-test app
can make an account without an invite.

The handlers are replaced by name against a pinned Skrift; ``test_auth_controller``
pins the route inventory so an upgrade that renames one, or adds a route that creates
accounts, fails loudly.
"""

from __future__ import annotations

import hmac
from typing import Annotated

from litestar import Request, get, post
from litestar.params import Parameter
from litestar.response import Redirect, Response
from litestar.response import Template as TemplateResponse
from litestar.status_codes import HTTP_201_CREATED, HTTP_403_FORBIDDEN
from skrift.auth.second_factors.passkey_service import get_primary_passkey_registration_state
from skrift.config import get_settings
from skrift.controllers.auth import AuthController
from skrift.forms.core import CSRF_FIELD_NAME, CSRF_SESSION_KEY
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
#: The view the sign-in page draws: ``signup`` or ``signin``.
PAGE_VIEW_CONTEXT = "page_view"
SIGNUP_VIEW = "signup"
SIGNIN_VIEW = "signin"


def invite_required(request: Request) -> Response:
    return Response(
        {"error": INVITE_REQUIRED, "csrf_token": request.session.get(CSRF_SESSION_KEY, "")},
        status_code=HTTP_403_FORBIDDEN,
    )


async def skrift_would_proceed(request: Request, provider: str) -> bool:
    """Whether Skrift's handler would get past its own checks: the provider is a
    configured passkey method and the form's CSRF token matches. Compared, not spent:
    Skrift's handler verifies and rotates it when Build lets the request through."""
    settings = get_settings()
    if provider not in settings.auth.get_method_keys():
        return False
    if settings.auth.get_primary_auth_method_type(provider) != "passkey":
        return False
    submitted = str((await request.form()).get(CSRF_FIELD_NAME, ""))
    stored = str(request.session.get(CSRF_SESSION_KEY, ""))
    return bool(stored) and hmac.compare_digest(submitted, stored)


def page_view(invite: Invite | None, requested: str | None) -> str:
    """Account creation for a visitor carrying an open invite, unless they asked for
    sign-in; sign-in for everyone else, whatever they asked for."""
    return SIGNUP_VIEW if invite and requested != SIGNIN_VIEW else SIGNIN_VIEW


async def with_page_view(
    response: Redirect | TemplateResponse,
    request: Request,
    db_session: AsyncSession,
    requested: str | None,
) -> Redirect | TemplateResponse:
    if isinstance(response, TemplateResponse):
        invite = await carried_invite(request, db_session, utc_now())
        response.context[INVITE_EMAIL_CONTEXT] = invite.email if invite else None
        response.context[PAGE_VIEW_CONTEXT] = page_view(invite, requested)
    return response


class BuildAuthController(AuthController):
    @get("/login")
    async def login_page(
        self,
        request: Request,
        db_session: AsyncSession,
        next_url: Annotated[str | None, Parameter(query="next")] = None,
        view: str | None = None,
    ) -> TemplateResponse:
        response = await AuthController.login_page.fn(self, request, next_url)
        return await with_page_view(response, request, db_session, view)

    @get("/{provider:str}/login")
    async def oauth_login(
        self,
        request: Request,
        db_session: AsyncSession,
        provider: str,
        next_url: Annotated[str | None, Parameter(query="next")] = None,
        view: str | None = None,
    ) -> Redirect | TemplateResponse:
        response = await AuthController.oauth_login.fn(self, request, provider, next_url)
        return await with_page_view(response, request, db_session, view)

    @post("/{provider:str}/register/options")
    async def begin_primary_method_registration(
        self, request: Request, db_session: AsyncSession, provider: str
    ) -> Response:
        email = str((await request.form()).get("email", ""))
        if await skrift_would_proceed(request, provider) and not admits(
            await carried_invite(request, db_session, utc_now()), email
        ):
            return invite_required(request)
        return await AuthController.begin_primary_method_registration.fn(
            self, request, db_session, provider
        )

    @post("/{provider:str}/register/complete")
    async def complete_primary_method_registration(
        self, request: Request, db_session: AsyncSession, provider: str
    ) -> Response:
        signup = get_primary_passkey_registration_state(request)
        invite = None
        # No registration under way is Skrift's refusal to give, in its own words.
        if await skrift_would_proceed(request, provider) and signup is not None:
            invite = await carried_invite(request, db_session, utc_now())
            if not admits(invite, signup.email):
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
