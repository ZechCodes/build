"""Custom controllers for Build."""

import logging

from litestar import Controller, Request, get
from litestar.response import Redirect, Response, Template
from sqlalchemy import select, and_
from sqlalchemy.ext.asyncio import AsyncSession
from skrift.lib.hooks import add_filter

from build_app.models import CampaignSignup

logger = logging.getLogger(__name__)

CAMPAIGN_SLUG = "build-launch"


async def _redirect_to_confirm(next_url, login_result, request):
    """After OAuth login, redirect to the email updates confirmation page."""
    return "/signup/oauth-confirm"


add_filter("login_redirect", _redirect_to_confirm, priority=5)


PREVIEW_FRAME_HTML = """<!DOCTYPE html>
<html><head><meta charset="UTF-8"></head><body>
<script>
window.addEventListener('message', function(e) {
  if (e.data && e.data.type === '__build_preview') {
    document.open();
    document.write(e.data.html);
    document.close();
  }
});
window.parent.postMessage({type: '__build_preview_ready'}, '*');
</script>
</body></html>"""


@get("/preview-frame", exclude_from_auth=True)
async def preview_frame() -> Response:
    """Serve a minimal HTML page for rendering HTML previews in an iframe.

    This endpoint has a permissive CSP so that arbitrary user HTML
    (with inline styles, scripts, and external resources) can render.
    """
    return Response(
        content=PREVIEW_FRAME_HTML,
        media_type="text/html",
        headers={
            "Content-Security-Policy": "default-src * 'unsafe-inline' 'unsafe-eval' data: blob:",
            "X-Frame-Options": "SAMEORIGIN",
        },
    )


class SignupConfirmController(Controller):
    """Handles signup confirmation routes."""

    path = "/signup"

    @get("/confirm")
    async def confirm_signup(self, token: str, db_session: AsyncSession) -> Template:
        """Confirm a waitlist email via token and render themed result."""
        result = await db_session.execute(
            select(CampaignSignup).where(
                CampaignSignup.confirmation_token == token
            )
        )
        signup = result.scalar_one_or_none()

        if signup:
            signup.email_confirmed = True
            signup.confirmation_token = None
            await db_session.commit()
            logger.info("Email confirmed for signup %s", signup.id)

        return Template(
            "page-confirm.html",
            context={"success": signup is not None},
        )

    @get("/oauth-confirm")
    async def oauth_confirm(self, request: Request, db_session: AsyncSession) -> Template | Redirect:
        """Show confirmation page after OAuth login asking to join waitlist."""
        email = request.session.get("user_email")
        if not email:
            return Redirect(path="/")

        # Check if already signed up
        result = await db_session.execute(
            select(CampaignSignup).where(
                and_(
                    CampaignSignup.campaign_slug == CAMPAIGN_SLUG,
                    CampaignSignup.email == email,
                )
            )
        )
        existing = result.scalar_one_or_none()

        return Template(
            "page-oauth-confirm.html",
            context={
                "email": email,
                "name": request.session.get("user_name", ""),
                "already_signed_up": existing is not None,
            },
        )

    @get("/oauth-join")
    async def oauth_join(self, request: Request, db_session: AsyncSession) -> Redirect:
        """User confirmed they want to join the waitlist via OAuth."""
        email = request.session.get("user_email")
        if not email:
            return Redirect(path="/")

        # Check for existing signup
        result = await db_session.execute(
            select(CampaignSignup).where(
                and_(
                    CampaignSignup.campaign_slug == CAMPAIGN_SLUG,
                    CampaignSignup.email == email,
                )
            )
        )
        existing = result.scalar_one_or_none()

        if not existing:
            signup = CampaignSignup(
                campaign_slug=CAMPAIGN_SLUG,
                email=email,
                email_confirmed=True,
                confirmation_token=None,
                email_updates=True,
            )
            db_session.add(signup)
            await db_session.commit()
            logger.info("OAuth waitlist signup: %s", email)

        return Redirect(path="/signup/oauth-done")

    @get("/oauth-done")
    async def oauth_done(self, request: Request) -> Template | Redirect:
        """Thank you page after OAuth waitlist signup."""
        email = request.session.get("user_email")
        if not email:
            return Redirect(path="/")

        return Template(
            "page-oauth-done.html",
            context={
                "email": email,
                "name": request.session.get("user_name", ""),
            },
        )
