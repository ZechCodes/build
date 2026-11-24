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
from buildapp.bridge_http_proofs import canonical_origin
from buildapp.models import Device, HostedModeReport

try:
    from buildapp.hosted_tunnel_assignments import HostedModeSource as _HostedModeSource
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
            if device.approved and device.owner_user_id is not None:
                return cls(
                    device.id, device.owner_user_id, device.created_at, device.authorization_epoch,
                )
            return None

    async def get_current_assignment(db_session, pairing, reports):
        return None

    async def notify_hosted_tunnels_changed(db_session):
        pass

    class _HostedModeSource(Protocol):
        async def hosted_pairings(self, db_session: AsyncSession) -> Collection[PairingIdentity]: ...


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


class SqlHostedModeSource(_HostedModeSource):
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


hosted_mode_source = SqlHostedModeSource()
RESERVED_LABEL = re.compile(r"b-[0-9a-f]{32}")
SETUP_REASONS = {"provisioning", "capacity", "collision"}


def valid_generation(generation) -> bool:
    return type(generation) is int and 0 < generation < 2**53


def setup_reason(row: Assignment | None) -> str:
    reason = row.setup_reason if row is not None else None
    return reason if reason in SETUP_REASONS else "provisioning"


async def has_tunnel_access(db, device):
    return (await account_policy(db, device.owner_user_id)).tunnel_access


async def current_assignment(db, device) -> Assignment | None:
    pairing = PairingIdentity.from_device(device)
    if pairing is None or not await has_tunnel_access(db, device):
        return None
    row = await get_current_assignment(db, device, hosted_mode_source)
    if (
        row is None
        or pairing != PairingIdentity(
            row.device_id, row.owner_user_id, row.pairing_created_at, row.authorization_epoch,
        )
        or not _valid_assignment_address(row)
    ):
        return None
    return row


def is_hosted_origin(origin: str) -> bool:
    label, _, domain = (urlsplit(origin).hostname or "").partition(".")
    # Reserve the generated label even after the provisioner deletes its row.
    return bool(domain and RESERVED_LABEL.fullmatch(label))


def _valid_assignment_address(row: Assignment) -> bool:
    label, _, domain = row.hostname.partition(".")
    if not domain or label != f"b-{row.label}" or not RESERVED_LABEL.fullmatch(label):
        return False
    origin = f"https://{row.hostname}"
    try:
        return (
            canonical_origin(origin) == origin
            and urlsplit(origin).hostname == row.hostname
            and valid_generation(row.generation)
        )
    except ValueError:
        return False


def ready_assignment(row: Assignment | None) -> bool:
    return (
        row is not None and row.state == "ready"
        and row.tunnel_id is not None and bool(row.dns_record_id)
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
