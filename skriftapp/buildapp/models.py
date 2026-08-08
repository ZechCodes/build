"""Build app database models — devices paired to user accounts, and the short-lived
tokens browsers present to the relay.

Web-push subscriptions reuse the framework table (``skrift.db.models.push_subscription``);
Build adds no push tables of its own.

Registered for Alembic via the ``models:`` key in ``app.dev.yaml`` (Skrift's
``load_model_modules`` imports it so ``Base.metadata`` sees these tables).
"""

from __future__ import annotations

from datetime import datetime
from uuid import UUID

from advanced_alchemy.types import DateTimeUTC
from sqlalchemy import Boolean, ForeignKey, String
from sqlalchemy.orm import Mapped, mapped_column

from skrift.db.base import Base


class Device(Base):
    """A bridge (device daemon) registered to a user account.

    Created *pending* (no owner) when a bridge self-registers; bound to a user and
    ``approved`` once the human compares the fingerprint and approves. The relay reads
    ``identity_public_key_b64`` + ``approved`` + ``owner_user_id`` to authenticate the
    device's WS connection and to scope which browsers may reach it. The primary key is
    the bridge-generated ``device_id``, so it's the single identifier across api,
    relay, and bridge.
    """

    __tablename__ = "devices"

    name: Mapped[str] = mapped_column(String(255), nullable=False)

    # Null until approved; FK so deleting a user removes their devices.
    owner_user_id: Mapped[UUID | None] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"), nullable=True, index=True
    )

    # Pinned device keys (base64). Identity = Ed25519 (auth challenge), transport =
    # X25519 (clients wrap session keys to it).
    identity_public_key_b64: Mapped[str] = mapped_column(String(128), nullable=False)
    transport_public_key_b64: Mapped[str] = mapped_column(String(128), nullable=False)

    # SHA-256 hex of the pairing code while pending; cleared on approval.
    pairing_code_hash: Mapped[str | None] = mapped_column(
        String(64), index=True, nullable=True
    )

    approved: Mapped[bool] = mapped_column(Boolean, default=False, nullable=False)
    # "pending" | "online" | "offline"
    status: Mapped[str] = mapped_column(String(20), default="pending", nullable=False)
    last_seen_at: Mapped[datetime | None] = mapped_column(
        DateTimeUTC(timezone=True), nullable=True
    )


class EphemeralToken(Base):
    """A single-purpose, short-TTL token. Used for the browser→relay gateway handshake:
    the api mints one for a logged-in user; the relay validates it to learn the user and
    scope routing. Stored as a SHA-256 hash; the raw token is returned to the client once.
    """

    __tablename__ = "ephemeral_tokens"

    token_hash: Mapped[str] = mapped_column(
        String(64), unique=True, index=True, nullable=False
    )
    purpose: Mapped[str] = mapped_column(String(50), nullable=False)
    user_id: Mapped[UUID] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"), nullable=False, index=True
    )
    expires_at: Mapped[datetime] = mapped_column(
        DateTimeUTC(timezone=True), index=True, nullable=False
    )


class AnnouncedAppVersion(Base):
    """A frontend version the startup announcement has already pushed about.

    One row per announced version, newest row authoritative. Exists so a pod
    restart or scale-up serving the SAME build stays silent — the app-update
    push must fire once per deploy, not once per container start.
    """

    __tablename__ = "announced_app_versions"

    version: Mapped[str] = mapped_column(String(64), unique=True, nullable=False)
