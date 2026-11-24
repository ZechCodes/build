"""Shared issuer reads and mode-report contract for hosted tunnel assignments."""

from collections.abc import Collection
from dataclasses import dataclass
from datetime import datetime
from typing import Protocol
from uuid import UUID

from sqlalchemy import select, text, update
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.alpha_membership import current_alpha_membership
from buildapp.bridge_http_models import BridgeHttpEndpoint, BridgeHttpFamily
from buildapp.hosted_tunnel_models import HostedTunnelAssignment
from buildapp.models import AccountPolicy, Device

CHANGE_CHANNEL = "build_hosted_tunnels_changed"


@dataclass(frozen=True)
class PairingIdentity:
    device_id: UUID
    owner_user_id: UUID
    pairing_created_at: datetime
    authorization_epoch: int

    @classmethod
    def from_device(cls, device: Device) -> "PairingIdentity | None":
        if device.approved and device.owner_user_id is not None:
            return cls(device.id, device.owner_user_id, device.created_at, device.authorization_epoch)
        return None

    @classmethod
    def from_assignment(cls, row: HostedTunnelAssignment) -> "PairingIdentity":
        return cls(row.device_id, row.owner_user_id, row.pairing_created_at, row.authorization_epoch)


class HostedModeSource(Protocol):
    async def hosted_pairings(self, db_session: AsyncSession) -> Collection[PairingIdentity]:
        """Read all current signed, capable hosted requests with no personal origin.

        Raise on an incomplete read. A partial set looks like withdrawals.
        """


async def eligible_pairing(db: AsyncSession, identity: PairingIdentity) -> bool:
    # A scalar query rechecks the pairing even when its ORM object is cached or
    # the old device ID was reused after unpairing.
    approved = await db.scalar(select(Device.id).join(AccountPolicy, AccountPolicy.id == Device.owner_user_id).where(
        Device.id == identity.device_id,
        Device.owner_user_id == identity.owner_user_id,
        Device.approved.is_(True),
        Device.created_at == identity.pairing_created_at,
        Device.authorization_epoch == identity.authorization_epoch,
        AccountPolicy.tunnel_access.is_(True),
        current_alpha_membership(identity.owner_user_id),
    ))
    return approved is not None


async def get_current_assignment(
    db_session: AsyncSession, pairing: Device, reports: HostedModeSource,
) -> HostedTunnelAssignment | None:
    """Return only the current eligible assignment, including pending setup."""
    identity = PairingIdentity.from_device(pairing)
    if identity is None or identity not in await reports.hosted_pairings(db_session):
        return None
    if not await eligible_pairing(db_session, identity):
        return None
    row = await db_session.get(HostedTunnelAssignment, identity.device_id, populate_existing=True)
    if row is None or row.state == "removing" or PairingIdentity.from_assignment(row) != identity:
        return None
    return row


async def notify_hosted_tunnels_changed(db_session: AsyncSession) -> None:
    """Wake the separate worker when the caller commits a changed mode report."""
    if db_session.get_bind().dialect.name == "postgresql":
        await db_session.execute(text("SELECT pg_notify(:channel, '')"), {"channel": CHANGE_CHANNEL})


async def withdraw_hosted_endpoint(
    db_session: AsyncSession, device_id: UUID, hostname: str, generation: int, now: datetime,
) -> None:
    """Fence endpoint retirement to the exact obsolete assignment and origin."""
    await db_session.scalar(select(Device.id).where(Device.id == device_id).with_for_update())
    row = await db_session.get(HostedTunnelAssignment, device_id, populate_existing=True, with_for_update=True)
    if row is None or (row.hostname, row.generation) != (hostname, generation):
        return
    origin = f"https://{hostname}"
    await db_session.execute(
        update(BridgeHttpEndpoint)
        .where(BridgeHttpEndpoint.device_id == device_id, BridgeHttpEndpoint.origin == origin)
        .values(enabled=False)
    )
    await db_session.execute(
        update(BridgeHttpFamily)
        .where(BridgeHttpFamily.device_id == device_id, BridgeHttpFamily.origin == origin,
               BridgeHttpFamily.revoked_at.is_(None))
        .values(revoked_at=now)
    )
