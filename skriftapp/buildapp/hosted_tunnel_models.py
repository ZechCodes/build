"""Durable desired and observed state for the hosted tunnel provisioner."""

from datetime import datetime
from uuid import UUID

from advanced_alchemy.types import DateTimeUTC, GUID
from skrift.db.base import Base
from sqlalchemy import BigInteger, Boolean, CheckConstraint, Integer, LargeBinary, String
from sqlalchemy.orm import Mapped, mapped_column


@Base.registry.mapped
class HostedTunnelAssignment:
    __tablename__ = "hosted_tunnel_assignments"
    __table_args__ = (
        CheckConstraint("generation > 0", name="positive_generation"),
        CheckConstraint("state IN ('pending', 'ready', 'removing')", name="known_state"),
        CheckConstraint(
            "state != 'ready' OR (tunnel_id IS NOT NULL AND dns_record_id IS NOT NULL)",
            name="ready_has_provider_ids",
        ),
    )

    # Keep the row through account/device removal until DNS and tunnel cleanup
    # finishes. These identity fields intentionally have no cascading foreign keys.
    device_id: Mapped[UUID] = mapped_column(GUID(length=16), primary_key=True)
    owner_user_id: Mapped[UUID] = mapped_column(GUID(length=16))
    pairing_created_at: Mapped[datetime] = mapped_column(DateTimeUTC(timezone=True))
    authorization_epoch: Mapped[int] = mapped_column(Integer)
    generation: Mapped[int] = mapped_column(BigInteger)
    label: Mapped[str] = mapped_column(String(32), unique=True)
    hostname: Mapped[str] = mapped_column(String(253), unique=True)
    tunnel_id: Mapped[UUID | None] = mapped_column(GUID(length=16), unique=True)
    dns_record_id: Mapped[str | None] = mapped_column(String(64), unique=True)
    tunnel_create_started: Mapped[bool] = mapped_column(Boolean, default=False)
    state: Mapped[str] = mapped_column(String(8))
    setup_reason: Mapped[str | None] = mapped_column(String(32))
    encrypted_secret: Mapped[bytes] = mapped_column(LargeBinary)
