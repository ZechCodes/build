"""Reconcile one Cloudflare tunnel and proxied CNAME per eligible bridge."""

import asyncio
import logging
import random
from contextlib import asynccontextmanager
from functools import partial

from sqlalchemy import text

from buildapp.cloudflare_tunnels import ProviderError
from buildapp.hosted_tunnel_credentials import decrypt_tunnel_secret
from buildapp.hosted_tunnel_inventory import OwnershipCollision, check_dns, check_tunnel, owned_resources
from buildapp.hosted_tunnel_store import AssignmentStore, StaleAssignment

log = logging.getLogger(__name__)


@asynccontextmanager
async def postgres_reconcile_lease(engine, zone_name):
    """Hold a transaction-scoped advisory lock on a dedicated connection."""
    async with engine.begin() as connection:
        elected = await connection.scalar(
            text("SELECT pg_try_advisory_xact_lock(hashtextextended(:name, 0))"),
            {"name": f"build-hosted-tunnels:{zone_name}"},
        )

        async def check():
            # SQLAlchemy cannot reconnect inside this transaction. If the
            # connection died, the advisory lock may already belong to a peer.
            await connection.execute(text("SELECT 1"))

        yield check if elected else None


class HostedTunnelProvisioner:
    def __init__(self, sessions, reports, provider, wrapping_key, capacity, *, interval=30.0, lease=None):
        if interval <= 0 or capacity < 0 or len(wrapping_key) != 32:
            raise ValueError("invalid hosted tunnel provisioner settings")
        self._provider = provider
        self._zone = provider.zone
        self._store = AssignmentStore(sessions, reports, self._zone, wrapping_key)
        self._wrapping_key = wrapping_key
        self._capacity = capacity
        self._interval = interval
        self._changed = asyncio.Event()
        self._lease = lease or partial(postgres_reconcile_lease, sessions.kw["bind"], self._zone.name)

    def nudge(self) -> None:
        self._changed.set()

    async def request_rotation(self, device_id, generation) -> bool:
        changed = await self._store.request_rotation(device_id, generation)
        if changed:
            self.nudge()
        return changed

    async def run(self) -> None:
        delay = 0.0
        while True:
            self._changed.clear()
            try:
                await self.reconcile_once()
            except ProviderError as exc:
                log.warning("Hosted tunnel reconcile deferred: %s", exc)
                delay = min(60.0, 2 * delay or 1.0) if exc.retryable else 0.0
            except Exception as exc:
                # DB/adapter exceptions can contain SQL, parameters or config.
                log.warning("Hosted tunnel reconcile deferred (%s)", type(exc).__name__)
                delay = 0.0
            else:
                delay = 0.0
            if delay:
                await asyncio.sleep(random.uniform(delay / 2, delay))
                continue
            try:
                await asyncio.wait_for(self._changed.wait(), timeout=self._interval)
            except TimeoutError:
                pass

    async def reconcile_once(self) -> None:
        async with self._lease() as check_lease:
            if check_lease is None:
                return
            rows = await self._store.prepare()
            await check_lease()
            inventory = await self._provider.inventory()
            await check_lease()
            try:
                for row in rows:
                    if self._zone.label_from_hostname(row.hostname) == row.label:
                        await self._reconcile_row(row, inventory, check_lease)
            except StaleAssignment:
                # An in-flight create might have succeeded. Start with a fresh
                # inventory before another assignment can use this capacity.
                self.nudge()
                return
            assigned_labels = {row.label for row in rows}
            for label in sorted(inventory.reserved_labels(self._zone) - assigned_labels):
                await self._sweep_orphan(label, check_lease)

    async def _reconcile_row(self, row, inventory, check_lease):
        guard = partial(self._store.recheck, row)
        if row.state == "removing":
            await self._remove(row, guard, check_lease)
        else:
            await self._ensure(row, inventory, check_lease)

    async def _ensure(self, row, inventory, check_lease):
        try:
            resources = owned_resources(inventory, self._zone, row.label, row)
        except OwnershipCollision:
            await self._store.recheck(row, state="pending", setup_reason="collision")
            return
        tunnel = resources.tunnel
        if tunnel is None and (row.tunnel_id is not None or resources.dns is not None):
            state, reason = ("removing", None) if row.tunnel_create_started else ("pending", "collision")
            await self._store.recheck(row, state=state, setup_reason=reason)
            return
        if tunnel is None:
            row, tunnel = await self._create_tunnel(row, inventory, check_lease)
            if tunnel is None:
                return
        row = await self._store.recheck(row, tunnel_id=tunnel.id)
        record = resources.dns
        if record is None:
            row = await self._store.recheck(
                row, dns_record_id=None, state="pending", setup_reason="provisioning",
            )
            record = await self._call(
                check_lease, partial(self._store.recheck, row),
                partial(self._provider.create_dns, row.label, tunnel.id),
            )
            self._verify_created(check_dns, self._zone, row.label, record, tunnel.id)
            inventory.dns[record.id] = record
        await self._store.recheck(row, dns_record_id=record.id, state="ready", setup_reason=None)

    async def _create_tunnel(self, row, inventory, check_lease):
        if len(inventory.tunnels) >= self._capacity:
            return await self._store.recheck(row, setup_reason="capacity"), None
        row = await self._store.recheck(row, tunnel_create_started=True, setup_reason="provisioning")
        secret = decrypt_tunnel_secret(row, self._wrapping_key)
        tunnel = await self._call(
            check_lease, partial(self._store.recheck, row),
            partial(self._provider.create_tunnel, row.label, secret),
        )
        self._verify_created(check_tunnel, self._zone, row.label, tunnel)
        inventory.tunnels[tunnel.id] = tunnel
        return row, tunnel

    @staticmethod
    def _verify_created(check, *args):
        try:
            check(*args)
        except OwnershipCollision:
            raise ProviderError("unexpected create result") from None

    async def _remove(self, row, guard, check_lease):
        inventory = await self._call(check_lease, guard, self._provider.inventory)
        try:
            resources = owned_resources(inventory, self._zone, row.label, row)
            if await self._delete_resources(row.label, resources, inventory, guard, check_lease, row):
                await self._store.finish_removal(row)
        except OwnershipCollision:
            log.warning("Hosted tunnel cleanup deferred due to an ownership collision")

    async def _sweep_orphan(self, label, check_lease):
        try:
            await self._sweep_unassigned(label, check_lease)
        except StaleAssignment:
            return
        except OwnershipCollision:
            log.warning("Hosted tunnel orphan cleanup deferred due to an ownership collision")

    async def _sweep_unassigned(self, label, check_lease):
        inventory = await self._call(
            check_lease, partial(self._store.check_orphan, label, None, None), self._provider.inventory,
        )
        resources = owned_resources(inventory, self._zone, label, None)
        tunnel_id = resources.tunnel.id if resources.tunnel else (resources.dns.target_tunnel if resources.dns else None)
        record_id = resources.dns.id if resources.dns else None
        guard = partial(self._store.check_orphan, label, tunnel_id, record_id)
        await self._delete_resources(label, resources, inventory, guard, check_lease)

    async def _delete_resources(self, label, resources, inventory, guard, check_lease, row=None):
        tunnel, dns = resources.tunnel, resources.dns
        if dns is not None:
            await self._call(check_lease, guard, partial(self._provider.delete_dns, dns.id))
            inventory = await self._call(check_lease, guard, self._provider.inventory)
            remaining = owned_resources(inventory, self._zone, label, row)
            if remaining.dns is not None or dns.id in inventory.dns or remaining.tunnel != tunnel:
                return False
        if tunnel is None:
            return True
        if any(record.target_tunnel == tunnel.id for record in inventory.dns.values()):
            log.warning("Hosted tunnel deletion deferred while a CNAME targets it")
            return False
        await self._call(check_lease, guard, partial(self._provider.delete_tunnel, tunnel.id))
        confirmed = await self._call(check_lease, guard, self._provider.inventory)
        return (
            tunnel.id not in confirmed.tunnels
            and not any(self._zone.label_from_tunnel(item.name) == label for item in confirmed.tunnels.values())
            and not any(
                record.target_tunnel == tunnel.id or self._zone.label_from_hostname(record.name) == label
                for record in confirmed.dns.values()
            )
        )

    @staticmethod
    async def _call(check_lease, guard, request):
        await guard()
        await check_lease()
        try:
            return await request()
        finally:
            await check_lease()
            await guard()
