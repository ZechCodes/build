"""Separate, revocable HTTP authority; none of these credentials authorizes relay."""

from datetime import datetime
from uuid import UUID

from advanced_alchemy.types import DateTimeUTC
from sqlalchemy import Boolean, ForeignKey, Integer, String, UniqueConstraint
from sqlalchemy.orm import Mapped, mapped_column
from skrift.db.base import Base


class BridgeHttpEndpoint(Base):
    __tablename__ = "bridge_http_endpoints"
    device_id: Mapped[UUID] = mapped_column(
        ForeignKey("devices.id", ondelete="CASCADE"), unique=True
    )
    user_id: Mapped[UUID] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"), index=True
    )
    origin: Mapped[str] = mapped_column(String(2048))
    identity_fingerprint: Mapped[str] = mapped_column(String(64))
    endpoint_epoch: Mapped[str] = mapped_column(String(128))
    authorization_epoch: Mapped[int] = mapped_column(Integer)
    enabled: Mapped[bool] = mapped_column(Boolean)


class BridgeHttpChallenge(Base):
    __tablename__ = "bridge_http_challenges"
    purpose: Mapped[str] = mapped_column(String(32))
    device_id: Mapped[UUID | None] = mapped_column(
        ForeignKey("devices.id", ondelete="CASCADE"), nullable=True, index=True
    )
    user_id: Mapped[UUID | None] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"), nullable=True, index=True
    )
    client_binding: Mapped[str | None] = mapped_column(String(128), nullable=True)
    origin: Mapped[str | None] = mapped_column(String(2048), nullable=True)
    nonce: Mapped[str] = mapped_column(String(128))
    nonce_hash: Mapped[str] = mapped_column(String(64))
    expires_at: Mapped[datetime] = mapped_column(DateTimeUTC(timezone=True), index=True)
    consumed_at: Mapped[datetime | None] = mapped_column(
        DateTimeUTC(timezone=True), nullable=True
    )


class BridgeHttpFamily(Base):
    __tablename__ = "bridge_http_families"
    issuer: Mapped[str] = mapped_column(String(2048))
    user_id: Mapped[UUID] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"), index=True
    )
    device_id: Mapped[UUID] = mapped_column(
        ForeignKey("devices.id", ondelete="CASCADE"), index=True
    )
    client_binding: Mapped[str] = mapped_column(String(128))
    origin: Mapped[str] = mapped_column(String(2048))
    endpoint_epoch: Mapped[str] = mapped_column(String(128))
    authorization_epoch: Mapped[int] = mapped_column(Integer)
    scope: Mapped[str] = mapped_column(String(128))
    parent_jti: Mapped[str | None] = mapped_column(String(128), nullable=True)
    parent_expires_at: Mapped[datetime | None] = mapped_column(
        DateTimeUTC(timezone=True), nullable=True
    )
    revoked_at: Mapped[datetime | None] = mapped_column(
        DateTimeUTC(timezone=True), nullable=True
    )


class BridgeHttpToken(Base):
    __tablename__ = "bridge_http_tokens"
    family_id: Mapped[UUID] = mapped_column(
        ForeignKey("bridge_http_families.id", ondelete="CASCADE"), index=True
    )
    token_hash: Mapped[str] = mapped_column(String(64), unique=True, index=True)
    expires_at: Mapped[datetime] = mapped_column(DateTimeUTC(timezone=True), index=True)


class BridgeHttpReplay(Base):
    __tablename__ = "bridge_http_replays"
    __table_args__ = (
        UniqueConstraint("device_id", "jti", name="uq_bridge_http_device_jti"),
    )
    device_id: Mapped[UUID] = mapped_column(
        ForeignKey("devices.id", ondelete="CASCADE"), index=True
    )
    jti: Mapped[str] = mapped_column(String(128))
    expires_at: Mapped[datetime] = mapped_column(DateTimeUTC(timezone=True), index=True)


class BridgeHttpClient(Base):
    """Logout tombstones prevent old signed browser cookies reminting authority."""

    __tablename__ = "bridge_http_clients"
    __table_args__ = (
        UniqueConstraint(
            "user_id", "binding_hash", name="uq_bridge_http_client_binding"
        ),
    )
    user_id: Mapped[UUID] = mapped_column(
        ForeignKey("users.id", ondelete="CASCADE"), index=True
    )
    binding_hash: Mapped[str] = mapped_column(String(64))
    revoked_at: Mapped[datetime | None] = mapped_column(
        DateTimeUTC(timezone=True), nullable=True
    )
