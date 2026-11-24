"""Short, committed database transactions around provisioner operations."""

import secrets
from uuid import UUID

from sqlalchemy import or_, select
from sqlalchemy.ext.asyncio import AsyncSession, async_sessionmaker

from buildapp.clock import utc_now
from buildapp.cloudflare_tunnels import BridgeZone
from buildapp.hosted_tunnel_assignments import (
    HostedModeSource,
    PairingIdentity,
    eligible_pairing,
    notify_hosted_tunnels_changed,
    withdraw_hosted_endpoint,
)
from buildapp.hosted_tunnel_credentials import new_encrypted_secret
from buildapp.hosted_tunnel_models import HostedTunnelAssignment
from buildapp.models import Device


class StaleAssignment(Exception):
    """The operation lost its database fence."""


class AssignmentStore:
    def __init__(
        self, sessions: async_sessionmaker[AsyncSession], reports: HostedModeSource,
        zone: BridgeZone, wrapping_key: bytes,
    ):
        self.sessions = sessions
        self.reports = reports
        self.zone = zone
        self._wrapping_key = wrapping_key

    def _new_generation(self, row, identity):
        row.owner_user_id = identity.owner_user_id
        row.pairing_created_at = identity.pairing_created_at
        row.authorization_epoch = identity.authorization_epoch
        # Start in the exactly representable range for JS. A fresh row should
        # not reuse the revision of a removed row for the same device.
        row.generation = row.generation + 1 if row.generation else secrets.randbits(52) + 1
        row.label = secrets.token_hex(16)
        row.hostname = self.zone.hostname(row.label)
        row.tunnel_id = None
        row.dns_record_id = None
        row.tunnel_create_started = False
        row.state = "pending"
        row.setup_reason = "provisioning"
        row.encrypted_secret = new_encrypted_secret(row, self._wrapping_key)

    async def prepare(self) -> list[HostedTunnelAssignment]:
        async with self.sessions() as db:
            reports = await self.reports.hosted_pairings(db)
            registered = set(await db.scalars(select(HostedTunnelAssignment.device_id)))
        requested = {report.device_id: report for report in reports}
        rows = []
        for device_id in sorted(registered | requested.keys()):
            row = await self._prepare_device(device_id, requested.get(device_id))
            if row is not None:
                rows.append(row)
        return rows

    async def _prepare_device(self, device_id, requested):
        async with self.sessions.begin() as db:
            row = await self._lock_row(db, device_id)
            if row is not None and not self._in_zone(row):
                # A DB shared during an environment migration must not send
                # this row to a provider configured for a different zone.
                return row
            desired = requested if requested and await eligible_pairing(db, requested) else None
            if row is None and desired is not None:
                row = HostedTunnelAssignment(device_id=device_id)
                self._new_generation(row, desired)
                db.add(row)
            elif row is not None and desired != PairingIdentity.from_assignment(row):
                await self._retire(db, row)
            return row

    async def recheck(self, expected: HostedTunnelAssignment, **updates) -> HostedTunnelAssignment:
        async with self.sessions.begin() as db:
            row = await self._checked_row(db, expected)
            if row is not None:
                if row.state == "ready" and updates.get("state", "ready") != "ready":
                    await self._withdraw(db, row)
                for column, value in updates.items():
                    setattr(row, column, value)
        if row is None:
            raise StaleAssignment
        return row

    async def finish_removal(self, expected: HostedTunnelAssignment) -> None:
        async with self.sessions.begin() as db:
            row = await self._checked_row(db, expected)
            if row is None:
                return
            reports = await self.reports.hosted_pairings(db)
            for report in reports:
                if report.device_id == row.device_id and await eligible_pairing(db, report):
                    self._new_generation(row, report)
                    await notify_hosted_tunnels_changed(db)
                    return
            await db.delete(row)

    async def request_rotation(self, device_id: UUID, generation: int) -> bool:
        async with self.sessions.begin() as db:
            row = await self._lock_row(db, device_id)
            if row is None or row.generation != generation or not self._in_zone(row):
                return False
            await self._retire(db, row)
            await notify_hosted_tunnels_changed(db)
            return True

    async def check_orphan(
        self, label: str, tunnel_id: UUID | None, record_id: str | None,
    ) -> None:
        filters = [HostedTunnelAssignment.label == label]
        if tunnel_id is not None:
            filters.append(HostedTunnelAssignment.tunnel_id == tunnel_id)
        if record_id is not None:
            filters.append(HostedTunnelAssignment.dns_record_id == record_id)
        async with self.sessions() as db:
            # A failed or partial mode-source read must never authorize a sweep.
            await self.reports.hosted_pairings(db)
            if await db.scalar(select(HostedTunnelAssignment.device_id).where(or_(*filters)).limit(1)):
                raise StaleAssignment

    async def _checked_row(self, db, expected):
        row = await self._lock_row(db, expected.device_id)
        if row is None or self._version(row) != self._version(expected):
            return None
        # Re-read the mode source for removal too. An incomplete read aborts
        # this sweep even if the assignment already had removal requested.
        reports = await self.reports.hosted_pairings(db)
        identity = PairingIdentity.from_assignment(row)
        if row.state != "removing" and (identity not in reports or not await eligible_pairing(db, identity)):
            await self._retire(db, row)
            return None
        return row

    @staticmethod
    async def _lock_row(db, device_id):
        # Registration locks the device before touching its single endpoint.
        await db.scalar(select(Device.id).where(Device.id == device_id).with_for_update())
        return await db.get(HostedTunnelAssignment, device_id, populate_existing=True, with_for_update=True)

    @staticmethod
    def _version(row):
        return (
            PairingIdentity.from_assignment(row), row.generation, row.label, row.state,
            row.tunnel_id, row.dns_record_id, row.tunnel_create_started,
        )

    def _in_zone(self, row):
        return self.zone.label_from_hostname(row.hostname) == row.label

    async def _retire(self, db, row):
        if row.state == "removing":
            return
        await self._withdraw(db, row)
        row.state = "removing"
        row.setup_reason = None

    @staticmethod
    async def _withdraw(db, row):
        await withdraw_hosted_endpoint(db, row.device_id, row.hostname, row.generation, utc_now())
