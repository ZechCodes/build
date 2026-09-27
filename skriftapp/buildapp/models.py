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

from advanced_alchemy.types import GUID, DateTimeUTC
from sqlalchemy import Boolean, ForeignKey, Integer, String, UniqueConstraint
from sqlalchemy.orm import Mapped, mapped_column

from skrift.db.base import Base

from buildapp.waitlist_address import MAX_WAITLIST_ADDRESS_LENGTH


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
    # Presence is derived from this stamp at read time (buildapp/presence.py).
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

    Nothing writes it since #191 retired the deploy push (a push fires only
    for what adds to the unread counter); the table stays in the migration
    chain until a migration drops it.
    """

    __tablename__ = "announced_app_versions"

    version: Mapped[str] = mapped_column(String(64), unique=True, nullable=False)


class WaitlistSignup(Base):
    __tablename__ = "waitlist_signups"

    email: Mapped[str] = mapped_column(
        String(MAX_WAITLIST_ADDRESS_LENGTH), unique=True, nullable=False
    )


class Invite(Base):
    """One invitation to the alpha. The token is held as a hash (raw shown once, in the
    email); the address it was sent to binds it, so a redemption proves the account and
    the invite are the same person. ``redeemed_by`` with no ``revoked_at`` IS the alpha
    membership marker — there is no separate members table, and revoking a redeemed
    invite is the operator's "remove member" action (``buildapp.alpha_membership``).
    """

    __tablename__ = "invites"

    token_hash: Mapped[str] = mapped_column(
        String(64), unique=True, index=True, nullable=False
    )
    # Normalized by waitlist_address.normalize_waitlist_address — the same rule the
    # waitlist accepts, so an invite and a signup name one address identically.
    email: Mapped[str] = mapped_column(
        String(MAX_WAITLIST_ADDRESS_LENGTH), index=True, nullable=False
    )
    # The admin who sent it. SET NULL: deleting an operator must not delete the
    # invites they issued, nor the membership those invites carry.
    invited_by: Mapped[UUID | None] = mapped_column(
        ForeignKey("users.id", ondelete="SET NULL"), nullable=True, index=True
    )
    expires_at: Mapped[datetime] = mapped_column(
        DateTimeUTC(timezone=True), nullable=False
    )
    redeemed_by: Mapped[UUID | None] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"), nullable=True, index=True
    )
    redeemed_at: Mapped[datetime | None] = mapped_column(
        DateTimeUTC(timezone=True), nullable=True
    )
    revoked_at: Mapped[datetime | None] = mapped_column(
        DateTimeUTC(timezone=True), nullable=True
    )


class TransportSession(Base):
    """One client session's transport life, as the bridge reported it
    (``planning/v2/Transport Telemetry Spec.md`` §Storage). Content-free: ids,
    timestamps, and three words for a path. The rules that fill these columns
    are ``transport_report.apply_event``; the admin transport page reads them.
    """

    __tablename__ = "transport_sessions"
    __table_args__ = (
        UniqueConstraint("device_id", "session_id", name="uq_transport_sessions_device_session"),
    )

    session_id: Mapped[str] = mapped_column(String(64), nullable=False, index=True)
    # Plain columns, not FKs: a device or user deleted later must not take the
    # count with it — the row is history, and the owner is copied at mint.
    device_id: Mapped[UUID] = mapped_column(GUID(length=16), nullable=False, index=True)
    owner_user_id: Mapped[UUID | None] = mapped_column(GUID(length=16), nullable=True, index=True)

    minted_at: Mapped[datetime | None] = mapped_column(DateTimeUTC(timezone=True), nullable=True, index=True)
    first_carrying_at: Mapped[datetime | None] = mapped_column(DateTimeUTC(timezone=True), nullable=True)
    # "direct" | "turn"; null until the first carrying.
    first_path: Mapped[str | None] = mapped_column(String(16), nullable=True)
    # "relay" | "direct" | "turn"
    current_path: Mapped[str] = mapped_column(String(16), default="relay", nullable=False)
    carrying_count: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    turn_count: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    # How often the session lost its last DataChannel while still alive — the
    # bridge's ``channels_lost``. The column keeps its original name (it was
    # ``fell_back`` while the relay was still a data plane) so no migration is
    # needed; both words count here.
    fell_back_count: Mapped[int] = mapped_column(Integer, default=0, nullable=False)
    ended_at: Mapped[datetime | None] = mapped_column(DateTimeUTC(timezone=True), nullable=True)
