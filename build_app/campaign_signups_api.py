"""Campaign signups API — Litestar controller.

Handles email interest capture for the Build launch waitlist.
Lives at /v2/api/campaign-signups and uses Skrift's DB session.
"""

from __future__ import annotations

import logging
import re
import time
from collections import defaultdict
from uuid import uuid4

from litestar import Controller, Request, post
from litestar.exceptions import HTTPException
from litestar.status_codes import (
    HTTP_201_CREATED,
    HTTP_409_CONFLICT,
    HTTP_422_UNPROCESSABLE_ENTITY,
    HTTP_429_TOO_MANY_REQUESTS,
)
from msgspec import Struct
from sqlalchemy import select, and_
from sqlalchemy.ext.asyncio import AsyncSession

from build_app.config import get_settings
from build_app.email import send_email
from build_app.models import CampaignSignup

logger = logging.getLogger(__name__)

# Per-IP rate limiting: max 5 requests per 60-second window.
_RATE_LIMIT = 5
_RATE_WINDOW = 60  # seconds
_request_log: dict[str, list[float]] = defaultdict(list)


def _check_rate_limit(ip: str) -> None:
    """Raise 429 if the IP has exceeded the signup rate limit."""
    now = time.monotonic()
    timestamps = _request_log[ip]
    _request_log[ip] = timestamps = [t for t in timestamps if now - t < _RATE_WINDOW]
    if len(timestamps) >= _RATE_LIMIT:
        raise HTTPException(
            status_code=HTTP_429_TOO_MANY_REQUESTS,
            detail="Too many requests. Please try again later.",
        )
    timestamps.append(now)


def _build_confirmation_html(confirm_url: str) -> str:
    """Build the styled HTML confirmation email for Build waitlist."""
    return f"""\
<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Confirm your email</title>
</head>
<body style="margin:0;padding:0;background-color:#07070a;font-family:system-ui,-apple-system,sans-serif">
<table role="presentation" cellpadding="0" cellspacing="0" border="0" width="100%" style="background-color:#07070a">
<tr><td align="center" style="padding:48px 16px">
  <table role="presentation" cellpadding="0" cellspacing="0" border="0" width="480" style="max-width:480px;width:100%">
    <tr><td style="padding:0 0 32px;font-size:20px;font-weight:700;color:#e8e8ed;letter-spacing:-0.02em">
      Build
    </td></tr>
    <tr><td style="background-color:#0e0e14;border:1px solid rgba(255,255,255,0.06);border-radius:12px;padding:36px 32px">
      <p style="margin:0 0 16px;font-size:18px;font-weight:600;color:#e8e8ed;line-height:1.3">Confirm your email</p>
      <p style="margin:0 0 24px;font-size:14px;color:#8b8b9e;line-height:1.6">You signed up for the Build launch waitlist. Click the button below to confirm your email and we'll notify you when we're ready.</p>
      <table role="presentation" cellpadding="0" cellspacing="0" border="0"><tr><td style="background-color:#818cf8;border-radius:8px">
        <a href="{confirm_url}" target="_blank" style="display:inline-block;padding:12px 32px;font-size:14px;font-weight:600;color:#fff;text-decoration:none">Confirm email</a>
      </td></tr></table>
      <p style="margin:24px 0 0;font-size:12px;color:#55556a;line-height:1.5">If you didn't sign up, you can safely ignore this email. This link expires in 48 hours.</p>
    </td></tr>
    <tr><td style="padding:24px 0 0;text-align:center">
      <p style="margin:0;font-size:11px;color:#55556a">&copy; 2026 getbuild.ing</p>
    </td></tr>
  </table>
</td></tr>
</table>
</body>
</html>"""


async def _send_confirmation(email: str, token: str) -> None:
    """Build and send the styled confirmation email."""
    settings = get_settings()
    confirm_url = f"{settings.site_base_url}/signup/confirm?token={token}"
    html = _build_confirmation_html(confirm_url)
    await send_email(email, "Build: confirm your email", html)


class SignupBody(Struct):
    """Request body for campaign signup."""

    campaign_slug: str
    email: str
    email_updates: bool = True


class CampaignSignupsApiController(Controller):
    """Campaign signups REST API using Skrift's primary database."""

    path = "/v2/api/campaign-signups"

    @post("", status_code=HTTP_201_CREATED)
    async def create_signup(
        self, data: SignupBody, request: Request, db_session: AsyncSession
    ) -> dict:
        """Register interest in a campaign."""
        _check_rate_limit(request.client.host if request.client else "unknown")

        if not data.email:
            raise HTTPException(
                status_code=HTTP_422_UNPROCESSABLE_ENTITY,
                detail="Email is required.",
            )

        if not re.match(r"^[^@\s]+@[^@\s]+\.[^@\s]+$", data.email):
            raise HTTPException(
                status_code=HTTP_422_UNPROCESSABLE_ENTITY,
                detail="Invalid email format.",
            )

        # Check for duplicate
        existing = await db_session.execute(
            select(CampaignSignup).where(
                and_(
                    CampaignSignup.campaign_slug == data.campaign_slug,
                    CampaignSignup.email == data.email,
                )
            )
        )
        existing_signup = existing.scalar_one_or_none()

        if existing_signup:
            if not existing_signup.email_confirmed:
                # Resend confirmation for unconfirmed email signups
                token = str(uuid4())
                existing_signup.confirmation_token = token
                await db_session.commit()
                await _send_confirmation(data.email, token)
                return {"status": "ok", "campaign_slug": data.campaign_slug}

            raise HTTPException(
                status_code=HTTP_409_CONFLICT,
                detail="Already signed up for this campaign.",
            )

        token = str(uuid4())
        signup = CampaignSignup(
            campaign_slug=data.campaign_slug,
            email=data.email,
            email_confirmed=False,
            confirmation_token=token,
            email_updates=data.email_updates,
        )
        db_session.add(signup)
        await db_session.commit()

        await _send_confirmation(data.email, token)

        logger.info("Campaign signup created: %s for %s", data.email, data.campaign_slug)

        return {"status": "ok", "campaign_slug": data.campaign_slug}
