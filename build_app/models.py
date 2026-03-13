"""Database models for the Build application."""

from __future__ import annotations

from typing import Optional

from sqlalchemy import Boolean, String, UniqueConstraint, CheckConstraint
from sqlalchemy.orm import Mapped, mapped_column

from skrift.db.base import Base


class CampaignSignup(Base):
    """Email signups for marketing campaigns (e.g. build-launch waitlist)."""

    __tablename__ = "campaign_signups"
    __table_args__ = (
        UniqueConstraint(
            "campaign_slug", "email",
            name="uq_campaign_signups_slug_email",
        ),
        CheckConstraint(
            "email IS NOT NULL",
            name="email_required",
        ),
    )

    campaign_slug: Mapped[str] = mapped_column(
        String(100),
        nullable=False,
        index=True,
    )
    email: Mapped[str] = mapped_column(
        String(320),
        nullable=False,
    )
    email_confirmed: Mapped[bool] = mapped_column(
        Boolean,
        nullable=False,
        default=False,
        server_default="false",
    )
    confirmation_token: Mapped[Optional[str]] = mapped_column(
        String(36),
        nullable=True,
        unique=True,
        index=True,
    )
    email_updates: Mapped[bool] = mapped_column(
        Boolean,
        nullable=False,
        default=True,
        server_default="true",
    )
