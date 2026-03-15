"""Database models for the Build application."""

from __future__ import annotations

import json
from datetime import datetime
from typing import Optional
from uuid import UUID

from sqlalchemy import Boolean, DateTime, ForeignKey, Integer, String, Text, UniqueConstraint, CheckConstraint
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


class Device(Base):
    """A registered device that can run Build agents."""

    __tablename__ = "devices"
    __table_args__ = (
        UniqueConstraint("name", "owner_user_id", name="uq_device_name_owner"),
    )

    name: Mapped[str] = mapped_column(
        String(100),
        nullable=False,
    )
    public_key: Mapped[str] = mapped_column(
        String(100),
        nullable=False,
        comment="Base64-encoded Ed25519 public key",
    )
    owner_user_id: Mapped[UUID] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"),
        nullable=False,
        index=True,
    )
    approved: Mapped[bool] = mapped_column(
        Boolean,
        nullable=False,
        default=True,
        server_default="true",
    )
    status: Mapped[str] = mapped_column(
        String(20),
        nullable=False,
        default="offline",
        server_default="'offline'",
        comment="online or offline",
    )

    # Heartbeat tracking
    last_heartbeat_at: Mapped[Optional[datetime]] = mapped_column(
        DateTime(timezone=True),
        nullable=True,
    )
    heartbeat_interval_s: Mapped[int] = mapped_column(
        Integer,
        nullable=False,
        default=30,
        server_default="30",
        comment="Expected heartbeat interval in seconds",
    )
    missed_heartbeat_windows: Mapped[Optional[str]] = mapped_column(
        Text,
        nullable=True,
        comment="JSON array of {start, end} windows where heartbeats were missed",
    )

    def get_missed_windows(self) -> list[dict]:
        """Parse the missed heartbeat windows JSON."""
        if not self.missed_heartbeat_windows:
            return []
        return json.loads(self.missed_heartbeat_windows)

    def set_missed_windows(self, windows: list[dict]) -> None:
        """Serialize missed heartbeat windows to JSON."""
        self.missed_heartbeat_windows = json.dumps(windows)
