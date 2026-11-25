"""Issuer adapter for the provisioner and its signed mode input."""

import os
from collections.abc import Collection
from pathlib import Path
from urllib.parse import urlsplit

from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp.accounts import account_policy
from buildapp.bridge_http_proofs import canonical_origin
from buildapp.cloudflare_tunnels import HOST_PREFIX, label_from_host_label
from buildapp.hosted_tunnel_assignments import (
    HostedModeSource,
    PairingIdentity,
    get_current_assignment,
)
from buildapp.hosted_tunnel_credentials import (
    ConnectorCredentials,
    load_wrapping_key,
    materialize_connector_credentials,
)
from buildapp.hosted_tunnel_models import HostedTunnelAssignment
from buildapp.models import Device, HostedModeReport

class SqlHostedModeSource(HostedModeSource):
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
# Keep in sync with valid_hostname in bridge/src/http/hosted/claim.rs.
RESERVED_HOST_PREFIXES = ("b", "bd")
SETUP_REASONS = {"provisioning", "capacity", "collision"}


def valid_generation(generation) -> bool:
    return type(generation) is int and 0 < generation < 2**53


def setup_reason(row: HostedTunnelAssignment | None) -> str:
    reason = row.setup_reason if row is not None else None
    return reason if reason in SETUP_REASONS else "provisioning"


async def has_tunnel_access(db, device):
    return (await account_policy(db, device.owner_user_id)).tunnel_access


async def current_assignment(db, device) -> HostedTunnelAssignment | None:
    pairing = PairingIdentity.from_device(device)
    prefix = _host_prefix()
    if pairing is None or prefix is None or not await has_tunnel_access(db, device):
        return None
    row = await get_current_assignment(db, device, hosted_mode_source)
    if (
        row is None
        or pairing != PairingIdentity.from_assignment(row)
        or not _valid_assignment_address(row, prefix)
        or not _issuer_has_credentials()
    ):
        return None
    return row


def is_hosted_origin(origin: str) -> bool:
    label, _, domain = (urlsplit(origin).hostname or "").partition(".")
    # Both shipped host-label forms stay reserved before setup and after removal.
    return bool(domain) and any(
        label_from_host_label(label, prefix) is not None for prefix in RESERVED_HOST_PREFIXES
    )


def _host_prefix() -> str | None:
    prefix = os.environ.get("HOSTED_TUNNEL_HOST_PREFIX", "")
    return prefix if HOST_PREFIX.fullmatch(prefix) else None


def _issuer_has_credentials() -> bool:
    path = os.environ.get("HOSTED_TUNNEL_WRAPPING_KEY_FILE")
    if not path or not os.environ.get("HOSTED_TUNNEL_ACCOUNT_ID"):
        return False
    try:
        return Path(path).is_file()
    except OSError:
        return False


def _valid_assignment_address(row: HostedTunnelAssignment, prefix: str) -> bool:
    host_label, _, domain = row.hostname.partition(".")
    label = label_from_host_label(host_label, prefix)
    if not domain or label is None or label != row.label:
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


def ready_assignment(row: HostedTunnelAssignment | None) -> bool:
    return (
        row is not None and row.state == "ready"
        and row.tunnel_id is not None and bool(row.dns_record_id)
    )


def materialize_credentials(assignment: HostedTunnelAssignment) -> ConnectorCredentials:
    return materialize_connector_credentials(
        assignment, account_id=os.environ["HOSTED_TUNNEL_ACCOUNT_ID"],
        wrapping_key=load_wrapping_key(),
    )
