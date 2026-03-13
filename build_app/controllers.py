"""Custom controllers for Build."""

import logging

from litestar import Controller, get
from litestar.response import Template
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from build_app.models import CampaignSignup

logger = logging.getLogger(__name__)


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
