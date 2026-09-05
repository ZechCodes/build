"""The admin invites page: who has been invited, what became of each invite, and the
two forms that send one and take one back.

Both forms call the same ``invites.issue_invite`` / ``invites.revoke_invite`` the JSON
route calls — the page is another audience for one service, not a second implementation.
Revoking a redeemed invite is how a member is removed, so a redeemed row keeps its
Revoke button.
"""

from __future__ import annotations

from datetime import datetime
from typing import Any, Iterable, Mapping
from uuid import UUID

from litestar import Controller, Request, get, post
from litestar.di import Provide
from litestar.response import Redirect
from litestar.response import Template as TemplateResponse
from skrift.admin.helpers import get_admin_context
from skrift.admin.navigation import ADMIN_NAV_TAG
from skrift.auth.guards import Permission, auth_guard
from skrift.flash import flash_error, flash_success, get_flash_messages
from skrift.forms.core import verify_csrf
from skrift.lib.email_backends import EmailBackend
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import invites
from buildapp.accounts import addresses_by_id
from buildapp.clock import utc_now
from buildapp.email_message import provide_email_backend, provide_public_base_url
from buildapp.invite_mail import send_invite_email
from buildapp.invites import InviteState, invite_state
from buildapp.models import Invite
from buildapp.session_auth import session_user_id

INVITES_ADMIN_PATH = "/admin/invites"
REVOKE_PATH = "/admin/invites/{invite_id}/revoke"
INVITES_PAGE_ROUTE_PATH = "/invites"
REVOKE_ROUTE_PATH = "/invites/{invite_id:uuid}/revoke"
TEMPLATE_NAME = "admin/invites.html"

SEND_INVITE_LABEL = "Send invite"
REVOKE_LABEL = "Revoke"
NO_ONE = "—"
EMAIL_FIELD = "email"

#: The states whose seat can still be taken back. Revoking a redeemed invite is the
#: "remove member" action; a revoked or expired one has nothing left to revoke.
REVOCABLE_STATES = frozenset({InviteState.OPEN, InviteState.REDEEMED})

INVITE_SENT_MESSAGE = "Invite sent to {email}."
INVITE_REFUSED_MESSAGE = "That is not an address we can send to."
INVITE_REVOKED_MESSAGE = "Invite revoked."
INVITE_MISSING_MESSAGE = "That invite no longer exists."
CSRF_REFUSED_MESSAGE = "That form expired. Try again."


def build_invites_dashboard(
    rows: Iterable[Invite], addresses: Mapping[UUID, str], now: datetime
) -> list[dict[str, Any]]:
    """Everything the template shows, pure over its inputs — so the template test
    needs no database and no clock."""
    dashboard = []
    for invite in rows:
        state = invite_state(invite, now)
        dashboard.append(
            {
                "invite_id": str(invite.id),
                "email": invite.email,
                "state": state.value,
                "invited_by": addresses.get(invite.invited_by) or NO_ONE,
                "redeemed_by": addresses.get(invite.redeemed_by) or NO_ONE,
                "redeemed_at": invite.redeemed_at,
                "created_at": invite.created_at,
                "expires_at": invite.expires_at,
                "revocable": state in REVOCABLE_STATES,
                "revoke_path": REVOKE_PATH.format(invite_id=invite.id),
            }
        )
    return dashboard


class InvitesAdminController(Controller):
    """Send an invite, watch it land, take it back."""

    path = "/admin"
    guards = [auth_guard]
    dependencies = {
        "email_backend": Provide(provide_email_backend, sync_to_thread=False),
        "public_base_url": Provide(provide_public_base_url, sync_to_thread=False),
    }

    @get(
        INVITES_PAGE_ROUTE_PATH,
        tags=[ADMIN_NAV_TAG],
        guards=[auth_guard, Permission("administrator")],
        opt={"label": "Invites", "icon": "mail", "order": 94},
    )
    async def invites_page(
        self, request: Request, db_session: AsyncSession
    ) -> TemplateResponse:
        ctx = await get_admin_context(request, db_session)
        rows = await invites.all_invites(db_session)
        return TemplateResponse(
            TEMPLATE_NAME,
            context={
                "flash_messages": get_flash_messages(request),
                "invites": build_invites_dashboard(
                    rows, await addresses_by_id(db_session), utc_now()
                ),
                **ctx,
            },
        )

    @post(INVITES_PAGE_ROUTE_PATH, guards=[auth_guard, Permission("administrator")])
    async def send_invite(
        self,
        request: Request,
        db_session: AsyncSession,
        email_backend: EmailBackend,
        public_base_url: str,
    ) -> Redirect:
        """Sends inline rather than as a background task: the operator is watching the
        page, so the flash should follow a send that actually happened. Still fail-soft
        — a delivery failure is logged and the row stays."""
        if not await verify_csrf(request):
            return _flashed(request, CSRF_REFUSED_MESSAGE, ok=False)
        form = await request.form()
        try:
            invite, raw = await invites.issue_invite(
                db_session,
                str(form.get(EMAIL_FIELD, "")),
                session_user_id(request),
                utc_now(),
            )
        except ValueError:
            return _flashed(request, INVITE_REFUSED_MESSAGE, ok=False)
        await send_invite_email(
            email_backend, invite.email, invites.invite_url(public_base_url, raw)
        )
        return _flashed(request, INVITE_SENT_MESSAGE.format(email=invite.email), ok=True)

    @post(REVOKE_ROUTE_PATH, guards=[auth_guard, Permission("administrator")])
    async def revoke(
        self, request: Request, invite_id: UUID, db_session: AsyncSession
    ) -> Redirect:
        if not await verify_csrf(request):
            return _flashed(request, CSRF_REFUSED_MESSAGE, ok=False)
        revoked = await invites.revoke_invite(db_session, invite_id, utc_now())
        message = INVITE_REVOKED_MESSAGE if revoked else INVITE_MISSING_MESSAGE
        return _flashed(request, message, ok=revoked is not None)


def _flashed(request: Request, message: str, *, ok: bool) -> Redirect:
    (flash_success if ok else flash_error)(request, message)
    return Redirect(INVITES_ADMIN_PATH)
