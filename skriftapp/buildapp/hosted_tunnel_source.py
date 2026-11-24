"""Issuer adapter for the provisioner and its signed mode input."""

import os
import re
from dataclasses import dataclass
from datetime import datetime
from typing import Collection, Protocol, TypedDict
from urllib.parse import urlsplit
from uuid import UUID

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.accounts import account_policy
from buildapp.models import Device, HostedModeReport

try:
    from buildapp.hosted_tunnel_assignments import (
        PairingIdentity,
        get_current_assignment,
        notify_hosted_tunnels_changed,
    )
except ModuleNotFoundError as error:
    if error.name != "buildapp.hosted_tunnel_assignments":
        raise

    @dataclass(frozen=True)
    class PairingIdentity:
        device_id: UUID
        owner_user_id: UUID
        pairing_created_at: datetime
        authorization_epoch: int

        @classmethod
        def from_device(cls, device):
            return cls(
                device.id, device.owner_user_id, device.created_at, device.authorization_epoch,
            )

    async def get_current_assignment(db_session, pairing, reports):
        return None

    async def notify_hosted_tunnels_changed(db_session):
        pass


class Assignment(Protocol):
    device_id: UUID
    owner_user_id: UUID
    pairing_created_at: datetime
    authorization_epoch: int
    generation: int
    label: str
    hostname: str
    tunnel_id: UUID | None
    dns_record_id: str | None
    state: str
    setup_reason: str | None
    encrypted_secret: bytes


class ConnectorCredentials(TypedDict):
    AccountTag: str
    TunnelID: str
    TunnelSecret: str


class MaterializeCredentials(Protocol):
    def __call__(self, assignment: Assignment, /) -> ConnectorCredentials: ...


class HostedModeSource:
    async def hosted_pairings(self, db_session: AsyncSession) -> Collection[PairingIdentity]:
        """Return the complete current pairing set or propagate the read failure."""
        rows = await db_session.execute(
            select(
                HostedModeReport.device_id,
                HostedModeReport.owner_user_id,
                HostedModeReport.pairing_created_at,
                HostedModeReport.authorization_epoch,
            )
            .join(Device, Device.id == HostedModeReport.device_id)
            .where(
                Device.approved.is_(True),
                Device.owner_user_id == HostedModeReport.owner_user_id,
                Device.created_at == HostedModeReport.pairing_created_at,
                Device.authorization_epoch == HostedModeReport.authorization_epoch,
                HostedModeReport.mode == "hosted",
                HostedModeReport.hosted_capable.is_(True),
            )
        )
        return {PairingIdentity(*row) for row in rows}


hosted_mode_source = HostedModeSource()
RESERVED_LABEL = re.compile(r"b-[0-9a-f]{32}")


async def has_tunnel_access(db, device):
    return (await account_policy(db, device.owner_user_id)).tunnel_access


async def current_assignment(db, device) -> Assignment | None:
    if not await has_tunnel_access(db, device):
        return None
    pairing = PairingIdentity.from_device(device)
    row = await get_current_assignment(db, pairing, hosted_mode_source)
    if row is None or pairing != PairingIdentity(
        row.device_id, row.owner_user_id, row.pairing_created_at, row.authorization_epoch,
    ):
        return None
    return row


def bridge_zone() -> str:
    return os.environ.get("HOSTED_TUNNEL_ZONE_NAME", "").strip().lower().removesuffix(".")


def is_hosted_origin(origin: str) -> bool:
    label, _, domain = (urlsplit(origin).hostname or "").partition(".")
    zone = bridge_zone()
    # Without the zone, reserve the generated label shape so loss of issuer
    # configuration cannot turn a hosted address into a personal registration.
    return bool(domain and RESERVED_LABEL.fullmatch(label) and (not zone or domain == zone))


def ready_assignment(row: Assignment | None) -> bool:
    if row is None or row.state != "ready" or not row.tunnel_id or not row.dns_record_id:
        return False
    zone = bridge_zone()
    return (
        bool(zone)
        and RESERVED_LABEL.fullmatch(f"b-{row.label}") is not None
        and row.hostname == f"b-{row.label}.{zone}"
        and type(row.generation) is int
        and 0 < row.generation <= 2**31 - 1
    )


def _materialize_credentials(assignment: Assignment) -> ConnectorCredentials:
    from buildapp.hosted_tunnel_credentials import (
        load_wrapping_key,
        materialize_connector_credentials,
    )

    return materialize_connector_credentials(
        assignment, account_id=os.environ["HOSTED_TUNNEL_ACCOUNT_ID"],
        wrapping_key=load_wrapping_key(),
    )


materialize_credentials: MaterializeCredentials = _materialize_credentials
