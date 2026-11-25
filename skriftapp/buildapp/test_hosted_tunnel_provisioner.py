"""Lifecycle tests against a Cloudflare-shaped loopback HTTP server."""

import asyncio
import base64
import json
import logging
from contextlib import asynccontextmanager
from dataclasses import dataclass, field
from datetime import timedelta
from uuid import uuid4

import pytest
import pytest_asyncio
from sqlalchemy import select

from buildapp.cloudflare_tunnels import BridgeZone, CloudflareTunnelClient, ProviderError
from buildapp.db_test_support import add_account, add_member, create_skrift_tables, engine_for, file_session_maker
from buildapp.hosted_tunnel_assignments import PairingIdentity, get_current_assignment
from buildapp.hosted_tunnel_credentials import decrypt_tunnel_secret, materialize_connector_credentials
from buildapp.hosted_tunnel_models import HostedTunnelAssignment
from buildapp.hosted_tunnel_provisioner import HostedTunnelProvisioner
from buildapp.models import AccountPolicy, Device

ACCOUNT = "fixture-account"
TOKEN = "fixture-provider-token"
ZONE = "bridges.example.test"
WRAPPING_KEY = bytes(range(32))
TUNNELS = f"/client/v4/accounts/{ACCOUNT}/cfd_tunnel"
DNS = "/client/v4/zones/fixture-zone/dns_records"


@dataclass
class Reports:
    pairings: set = field(default_factory=set)
    incomplete: bool = False

    async def hosted_pairings(self, db_session):
        if self.incomplete:
            raise RuntimeError("fixture source failed")
        return self.pairings.copy()


class FakeCloudflare:
    def __init__(self):
        self.tunnels = {}
        self.dns = {}
        self.history = []
        self.lose = None
        self.fail = None
        self.incomplete = None
        self.before = None
        self.secret = None
        self.keep_dns = False
        self.fail_status = 503
        self.redirect = False

    async def respond(self, reader, writer):
        try:
            head = (await reader.readuntil(b"\r\n\r\n")).decode()
            line, *headers = head.strip().split("\r\n")
            method, target, _ = line.split()
            headers = dict(header.split(": ", 1) for header in headers)
            assert headers["Authorization"] == f"Bearer {TOKEN}"
            length = int(headers.get("Content-Length", 0))
            data = json.loads(await reader.readexactly(length)) if length else None
            path, _, query = target.partition("?")
            self.history.append((method, path))
            if self.redirect:
                writer.write(b"HTTP/1.1 302 Found\r\nLocation: /redirected\r\nContent-Length: 0\r\n\r\n")
                await writer.drain()
                return
            if self.before is not None:
                await self.before(method, path)
            if self.fail == (method, path):
                self.fail = None
                await self.send(writer, self.fail_status, {"success": False, "errors": [TOKEN, self.secret]})
                return
            payload = self.dispatch(method, path, query, data)
            if self.lose == (method, path):
                self.lose = None
                return
            await self.send(writer, 200, payload)
        finally:
            writer.close()
            await writer.wait_closed()

    async def send(self, writer, status, payload):
        body = json.dumps(payload).encode()
        writer.write(f"HTTP/1.1 {status} Test\r\nContent-Length: {len(body)}\r\n"
                     "Connection: close\r\n\r\n".encode() + body)
        await writer.drain()

    def dispatch(self, method, path, query, data):
        resources = self.tunnels if path.startswith(TUNNELS) else self.dns
        base = TUNNELS if resources is self.tunnels else DNS
        if method == "GET":
            return self.page(resources, base, path, query)
        if method == "DELETE":
            resource_id = path.removeprefix(base + "/")
            if resources is self.tunnels:
                assert query == "cascade=true"
                assert all(record["content"] != f"{resource_id}.cfargotunnel.com" for record in self.dns.values())
            kept = resources is self.dns and self.keep_dns
            return {"success": True, "result": resources[resource_id] if kept else resources.pop(resource_id)}
        resource_id = str(uuid4())
        if resources is self.tunnels:
            assert data["config_src"] == "local"
            assert len(base64.b64decode(data["tunnel_secret"], validate=True)) >= 32
            self.secret = data.pop("tunnel_secret")
            assert all(tunnel["name"] != data["name"] for tunnel in resources.values())
        else:
            assert data["type"] == "CNAME" and data["proxied"] is True
        resource = resources[resource_id] = {"id": resource_id, **data}
        return {"success": True, "result": resource}

    def page(self, resources, base, path, query):
        assert path == base
        params = dict(item.split("=") for item in query.split("&") if item)
        if resources is self.tunnels:
            assert params["is_deleted"] == "false"
        page = int(params["page"])
        values = list(resources.values())[(page - 1) * 2 : page * 2]
        total = len(resources) + (2 if self.incomplete == base else 0)
        return {
            "success": True, "result": values,
            "result_info": {"page": page, "per_page": 2, "count": len(values), "total_count": total},
        }


@asynccontextmanager
async def sqlite_test_lease():
    async def check():
        pass

    yield check


@dataclass
class Environment:
    db: object
    device: Device
    invite_id: object
    reports: Reports
    api: FakeCloudflare
    provider: CloudflareTunnelClient

    def worker(self, *, cap=10, account_limit=100, interval=0.03, lease=sqlite_test_lease):
        return HostedTunnelProvisioner(
            self.db, self.reports, self.provider, WRAPPING_KEY, cap,
            account_limit=account_limit, interval=interval, lease=lease,
        )

    async def row(self):
        async with self.db() as db:
            return await db.get(HostedTunnelAssignment, self.device.id)

    async def claimable(self):
        async with self.db() as db:
            return await get_current_assignment(db, self.device, self.reports)

    async def entitlement(self, enabled):
        async with self.db.begin() as db:
            policy = await db.get(AccountPolicy, self.device.owner_user_id)
            policy.tunnel_access = enabled


@pytest_asyncio.fixture
async def env(tmp_path):
    sessions = file_session_maker(tmp_path / "provisioner.sqlite")
    engine = engine_for(sessions)
    try:
        await create_skrift_tables(engine)
        async with hosted_environment(sessions) as initialized:
            yield initialized
    finally:
        await engine.dispose()


@asynccontextmanager
async def hosted_environment(sessions, *, api=None, prefix="b"):
    async with sessions() as db:
        user = await add_account(db, "provisioning@example.test")
        invite = await add_member(db, user, "provisioning@example.test")
        device = Device(
            approved=True, owner_user_id=user, identity_public_key_b64="identity",
            transport_public_key_b64="transport", name="fixture", hosted_ready_once=True,
        )
        db.add_all([device, AccountPolicy(id=user, tunnel_access=True)])
        await db.commit()
    api = api if api is not None else FakeCloudflare()
    server = await asyncio.start_server(api.respond, "127.0.0.1", 0)
    port = server.sockets[0].getsockname()[1]
    async with server, CloudflareTunnelClient(
        base_url=f"http://127.0.0.1:{port}/client/v4", account_id=ACCOUNT,
        zone_id="fixture-zone", zone_name=ZONE, host_prefix=prefix, token=TOKEN,
    ) as provider:
        yield Environment(sessions, device, invite.id, Reports({PairingIdentity.from_device(device)}), api, provider)


@pytest.mark.parametrize("prefix", ["", "B", "x-", "-x", "b.dev", "b" * 9])
def test_bridge_zone_requires_a_short_lowercase_host_prefix(prefix):
    with pytest.raises(ValueError, match="host prefix"):
        BridgeZone(ZONE, prefix)


def test_different_zones_have_distinct_resource_names_and_comments():
    here = BridgeZone(ZONE, "b")
    there = BridgeZone("other.example.test", "b")
    label, tunnel_id = uuid4().hex, uuid4()
    assert here.label_from_tunnel(there.tunnel_name(label)) is None
    assert here.dns_comment(label, tunnel_id) != there.dns_comment(label, tunnel_id)


@pytest.mark.asyncio
@pytest.mark.parametrize("prefix, peer_prefix", [("b", "bd"), ("bd", "b")])
async def test_shared_zone_scopes_capacity_and_cleanup(tmp_path, prefix, peer_prefix):
    api = FakeCloudflare()
    sessions = file_session_maker(tmp_path / "own.sqlite")
    peer_sessions = file_session_maker(tmp_path / "peer.sqlite")
    try:
        await create_skrift_tables(engine_for(sessions))
        await create_skrift_tables(engine_for(peer_sessions))
        async with (
            hosted_environment(sessions, api=api, prefix=prefix) as own,
            hosted_environment(peer_sessions, api=api, prefix=peer_prefix) as peer,
        ):
            await prove_shared_zone_isolation(own, peer)
    finally:
        await engine_for(sessions).dispose()
        await engine_for(peer_sessions).dispose()


async def prove_shared_zone_isolation(own, peer):
    await own.worker(cap=0).reconcile_once()
    label = (await own.row()).label
    own_zone, peer_zone = own.provider.zone, peer.provider.zone
    assert own_zone.label_from_tunnel(peer_zone.tunnel_name(label)) is None
    assert own_zone.label_from_hostname(peer_zone.hostname(label)) is None

    peer_tunnel = await peer.provider.create_tunnel(label, WRAPPING_KEY)
    peer_dns = await peer.provider.create_dns(label, peer_tunnel.id)
    peer_dangling = await peer.provider.create_dns(uuid4().hex, uuid4())
    peer_snapshot = (
        own.api.tunnels[str(peer_tunnel.id)].copy(),
        own.api.dns[peer_dns.id].copy(),
        own.api.dns[peer_dangling.id].copy(),
    )
    own_orphan = await own.provider.create_tunnel(uuid4().hex, WRAPPING_KEY)
    own_dangling = await own.provider.create_dns(uuid4().hex, uuid4())
    # A foreign tunnel is never adopted, even with the same random label.
    # Nor may a DNS record with our name/comment authorize deleting its target.
    foreign_target = await own.provider.create_dns(uuid4().hex, peer_tunnel.id)
    worker = own.worker(cap=1)
    await worker.reconcile_once()
    assert (await own.row()).setup_reason == "capacity"
    assert str(own_orphan.id) not in own.api.tunnels
    assert own_dangling.id not in own.api.dns

    await worker.reconcile_once()
    ready = await own.claimable()
    assert ready.state == "ready"
    assert ready.hostname == own_zone.hostname(label)
    assert ready.tunnel_id != peer_tunnel.id
    await own.entitlement(False)
    await worker.reconcile_once()
    assert str(ready.tunnel_id) not in own.api.tunnels
    assert ready.dns_record_id not in own.api.dns
    assert foreign_target.id in own.api.dns
    assert (
        own.api.tunnels[str(peer_tunnel.id)],
        own.api.dns[peer_dns.id],
        own.api.dns[peer_dangling.id],
    ) == peer_snapshot


@pytest.mark.asyncio
@pytest.mark.parametrize("lost_path", [TUNNELS, DNS])
async def test_uncertain_create_survives_restart_and_adopts_exact_resources(env, lost_path):
    async def before(method, path):
        row = await env.row()
        assert row and row.encrypted_secret and row.state == "pending"

    env.api.before = before
    env.api.lose = ("POST", lost_path)
    with pytest.raises(ProviderError, match="transport"):
        await env.worker().reconcile_once()
    original = await env.row()
    await env.worker().reconcile_once()
    ready = await env.claimable()
    assert ready and ready.state == "ready"
    assert (ready.generation, ready.label, ready.encrypted_secret) == (
        original.generation, original.label, original.encrypted_secret,
    )
    assert ready.hostname == f"b-{ready.label}.{ZONE}"
    assert len(ready.label) == 32
    assert len(env.api.tunnels) == len(env.api.dns) == 1
    assert env.api.history.count(("POST", TUNNELS)) == env.api.history.count(("POST", DNS)) == 1


@pytest.mark.asyncio
async def test_missed_nudge_and_signed_report_requirement(env):
    env.reports.pairings.clear()
    worker = env.worker()
    async with env.db() as db:
        assert await db.get(Device, env.device.id) is not None
    await worker.reconcile_once()
    assert await env.row() is None and not env.api.tunnels
    task = asyncio.create_task(worker.run())
    try:
        env.reports.pairings.add(PairingIdentity.from_device(env.device))
        async with asyncio.timeout(3):
            while (row := await env.claimable()) is None or row.state != "ready":
                await asyncio.sleep(0.01)
    finally:
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task


@pytest.mark.asyncio
async def test_quota_counts_existing_environment_tunnels(env):
    for index in range(3):
        key = str(uuid4())
        env.api.tunnels[key] = {
            "id": key, "name": env.provider.zone.tunnel_name(f"{index:032x}"), "config_src": "cloudflare",
        }
    worker = env.worker(cap=3)
    await worker.reconcile_once()
    row = await env.claimable()
    assert row.state == "pending" and row.setup_reason == "capacity"
    assert all(method == "GET" for method, _ in env.api.history)
    del env.api.tunnels[next(iter(env.api.tunnels))]
    await worker.reconcile_once()
    assert (await env.claimable()).state == "ready"


@pytest.mark.asyncio
async def test_account_limit_counts_unrelated_tunnels_and_requires_complete_inventory(env):
    key = str(uuid4())
    env.api.tunnels[key] = {"id": key, "name": "other-consumer", "config_src": "cloudflare"}
    worker = env.worker(cap=2, account_limit=1)
    await worker.reconcile_once()
    row = await env.claimable()
    assert row.state == "pending" and row.setup_reason == "capacity"
    assert all(method == "GET" for method, _ in env.api.history)

    del env.api.tunnels[key]
    env.api.incomplete = TUNNELS
    with pytest.raises(ProviderError, match="incomplete inventory"):
        await worker.reconcile_once()
    assert all(method == "GET" for method, _ in env.api.history)

    env.api.incomplete = None
    await worker.reconcile_once()
    assert (await env.claimable()).state == "ready"


@pytest.mark.asyncio
@pytest.mark.parametrize("path", [TUNNELS, DNS])
async def test_entitlement_loss_during_provider_write_fences_claims(env, path):
    async def before(method, actual_path):
        if (method, actual_path) == ("POST", path):
            await env.entitlement(False)
            assert await env.claimable() is None

    env.api.before = before
    worker = env.worker()
    await worker.reconcile_once()
    assert await env.claimable() is None
    if path == TUNNELS:
        assert ("POST", DNS) not in env.api.history
    await worker.reconcile_once()
    assert await env.row() is None and not env.api.dns and not env.api.tunnels


@pytest.mark.asyncio
async def test_dangling_cname_holds_tunnel_deletion(env):
    worker = env.worker()
    await worker.reconcile_once()
    row = await env.row()
    await env.entitlement(False)
    assert await env.claimable() is None
    env.api.fail = ("DELETE", f"{DNS}/{row.dns_record_id}")
    with pytest.raises(ProviderError, match="HTTP 503"):
        await worker.reconcile_once()
    assert row.dns_record_id in env.api.dns and str(row.tunnel_id) in env.api.tunnels
    assert ("DELETE", f"{TUNNELS}/{row.tunnel_id}") not in env.api.history
    await worker.reconcile_once()
    dns_delete = env.api.history.index(("DELETE", f"{DNS}/{row.dns_record_id}"), env.api.history.index(
        ("DELETE", f"{DNS}/{row.dns_record_id}")) + 1)
    assert env.api.history.index(("GET", DNS), dns_delete + 1) < env.api.history.index(
        ("DELETE", f"{TUNNELS}/{row.tunnel_id}"))
    assert not env.api.tunnels and not env.api.dns and await env.row() is None


@pytest.mark.asyncio
async def test_incomplete_inventory_or_desired_read_preserves_orphans(env):
    worker = env.worker()
    await worker.reconcile_once()
    async with env.db.begin() as db:
        await db.delete(await db.get(HostedTunnelAssignment, env.device.id))
    env.reports.pairings.clear()
    env.reports.incomplete = True
    with pytest.raises(RuntimeError, match="fixture source failed"):
        await worker.reconcile_once()
    env.reports.incomplete = False
    env.api.incomplete = DNS
    with pytest.raises(ProviderError, match="inventory"):
        await worker.reconcile_once()
    assert not any(method == "DELETE" for method, _ in env.api.history)
    env.api.incomplete = None

    async def lose_mode_source(method, path):
        if (method, path) == ("GET", DNS):
            env.reports.incomplete = True

    env.api.before = lose_mode_source
    with pytest.raises(RuntimeError, match="fixture source failed"):
        await worker.reconcile_once()
    assert not any(method == "DELETE" for method, _ in env.api.history)
    assert env.api.tunnels and env.api.dns

    env.api.before = None
    env.reports.incomplete = False
    await worker.reconcile_once()
    assert not env.api.tunnels and not env.api.dns


@pytest.mark.asyncio
async def test_ambiguous_ownership_leaves_colliding_dns_and_tunnel(env):
    worker = env.worker()
    await worker.reconcile_once()
    row = await env.row()
    env.api.dns[row.dns_record_id]["content"] = f"{uuid4()}.cfargotunnel.com"
    await worker.reconcile_once()
    assert (await env.claimable()).setup_reason == "collision"
    await env.entitlement(False)
    await worker.reconcile_once()
    assert env.api.dns and env.api.tunnels
    assert not any(method == "DELETE" for method, _ in env.api.history)


@pytest.mark.asyncio
async def test_exact_tunnel_name_without_prior_create_is_an_ownership_collision(env):
    tunnel_id = str(uuid4())
    existing = {"id": tunnel_id, "config_src": "local"}

    async def seed_before_first_inventory(method, path):
        if (method, path) == ("GET", TUNNELS):
            row = await env.row()
            assert row.state == "pending" and not row.tunnel_create_started
            existing["name"] = env.provider.zone.tunnel_name(row.label)
            env.api.tunnels[tunnel_id] = existing
            env.api.before = None

    env.api.before = seed_before_first_inventory
    worker = env.worker()
    await worker.reconcile_once()

    row = await env.claimable()
    assert (row.state, row.setup_reason) == ("pending", "collision")
    assert row.tunnel_id is None and row.dns_record_id is None
    assert not row.tunnel_create_started

    await env.entitlement(False)
    await worker.reconcile_once()
    assert env.api.tunnels == {tunnel_id: existing}
    assert not env.api.dns
    assert all(method == "GET" for method, _ in env.api.history)


@pytest.mark.asyncio
async def test_repair_and_rotation_replace_credentials_and_preserve_current_first_ready(env):
    worker = env.worker()
    await worker.reconcile_once()
    old = await env.row()
    await worker.request_rotation(env.device.id, old.generation)
    assert await env.claimable() is None
    await worker.reconcile_once()
    await worker.reconcile_once()
    rotated = await env.claimable()
    assert rotated.generation > old.generation
    assert rotated.label != old.label and rotated.tunnel_id != old.tunnel_id
    assert rotated.encrypted_secret != old.encrypted_secret
    async with env.db.begin() as db:
        device = await db.get(Device, env.device.id)
        assert device.hosted_ready_once
        device.authorization_epoch += 1
    env.reports.pairings = {PairingIdentity.from_device(device)}
    env.device = device
    assert await env.claimable() is None
    await worker.reconcile_once()
    await worker.reconcile_once()
    current = await env.claimable()
    assert current.authorization_epoch == device.authorization_epoch
    assert current.generation > rotated.generation and current.tunnel_id != rotated.tunnel_id


@pytest.mark.asyncio
async def test_at_rest_credentials_and_redacted_errors(env, caplog):
    caplog.set_level(logging.DEBUG)
    worker = env.worker()
    env.api.fail = ("POST", DNS)
    with pytest.raises(ProviderError) as failure:
        await worker.reconcile_once()
    await worker.reconcile_once()
    row = await env.row()
    credentials = materialize_connector_credentials(row, account_id=ACCOUNT, wrapping_key=WRAPPING_KEY)
    assert credentials == {
        "AccountTag": ACCOUNT, "TunnelID": str(row.tunnel_id), "TunnelSecret": env.api.secret,
    }
    async with env.db() as db:
        cipher = await db.scalar(select(HostedTunnelAssignment.encrypted_secret))
    assert base64.b64decode(env.api.secret) not in cipher
    for marker in (TOKEN, env.api.secret, base64.b64encode(WRAPPING_KEY).decode()):
        assert marker not in repr((row, env.provider, worker, failure.value, credentials))
        assert marker not in caplog.text and marker not in str(failure.value)


@pytest.mark.asyncio
@pytest.mark.parametrize("changed_field", [
    "device_id", "owner_user_id", "pairing_created_at", "authorization_epoch", "generation", "hostname",
])
async def test_assignment_ciphertext_cannot_be_replayed_with_different_binding(env, changed_field):
    await env.worker().reconcile_once()
    original = await env.row()
    assert original.state == "ready"
    assert decrypt_tunnel_secret(original, WRAPPING_KEY) == base64.b64decode(env.api.secret)
    replacement = {
        "device_id": uuid4(),
        "owner_user_id": uuid4(),
        "pairing_created_at": original.pairing_created_at + timedelta(seconds=1),
        "authorization_epoch": original.authorization_epoch + 1,
        "generation": original.generation + 1,
        "hostname": f"b-{uuid4().hex}.{ZONE}",
    }[changed_field]

    async with env.db.begin() as db:
        row = await db.get(HostedTunnelAssignment, original.device_id)
        setattr(row, changed_field, replacement)
        row.encrypted_secret = original.encrypted_secret
        target_id = row.device_id

    async with env.db() as db:
        replayed = await db.get(HostedTunnelAssignment, target_id)
        assert getattr(replayed, changed_field) == replacement
        assert replayed.encrypted_secret == original.encrypted_secret
        assert replayed.state == "ready" and replayed.tunnel_id == original.tunnel_id

    with pytest.raises(ValueError, match="unavailable"):
        decrypt_tunnel_secret(replayed, WRAPPING_KEY)
    with pytest.raises(ValueError, match="unavailable"):
        materialize_connector_credentials(replayed, account_id=ACCOUNT, wrapping_key=WRAPPING_KEY)


@pytest.mark.asyncio
async def test_lost_reply_while_replacing_missing_dns_keeps_generation(env):
    worker = env.worker()
    await worker.reconcile_once()
    original = await env.row()
    del env.api.dns[original.dns_record_id]
    env.api.lose = ("POST", DNS)
    with pytest.raises(ProviderError, match="transport"):
        await worker.reconcile_once()
    await worker.reconcile_once()
    current = await env.row()
    assert current.state == "ready"
    assert (current.generation, current.tunnel_id) == (original.generation, original.tunnel_id)
    assert current.dns_record_id != original.dns_record_id


@pytest.mark.asyncio
async def test_acknowledged_dns_delete_must_be_confirmed_by_inventory(env):
    worker = env.worker()
    await worker.reconcile_once()
    row = await env.row()
    await env.entitlement(False)
    env.api.keep_dns = True
    await worker.reconcile_once()
    assert row.dns_record_id in env.api.dns and str(row.tunnel_id) in env.api.tunnels
    env.api.keep_dns = False
    await worker.reconcile_once()
    assert not env.api.dns and not env.api.tunnels


@pytest.mark.asyncio
async def test_late_dns_orphan_only_removed_with_exact_ownership(env):
    worker = env.worker()
    await worker.reconcile_once()
    row = await env.row()
    orphan = env.api.dns[row.dns_record_id].copy()
    await env.entitlement(False)
    await worker.reconcile_once()
    assert not env.api.tunnels and not env.api.dns and await env.row() is None
    env.api.dns[orphan["id"]] = {**orphan, "comment": "operator-record"}
    await worker.reconcile_once()
    assert orphan["id"] in env.api.dns
    env.api.dns[orphan["id"]] = orphan
    await worker.reconcile_once()
    assert not env.api.tunnels and not env.api.dns


@pytest.mark.asyncio
async def test_membership_loss_withdraws_exact_origin_and_preserves_personal_endpoint(env):
    from buildapp.bridge_http_models import BridgeHttpEndpoint, BridgeHttpFamily
    from buildapp.clock import utc_now
    from buildapp.models import Invite

    worker = env.worker()
    await worker.reconcile_once()
    row = await env.row()
    origin = f"https://{row.hostname}"
    async with env.db.begin() as db:
        endpoint = BridgeHttpEndpoint(
            device_id=row.device_id, user_id=row.owner_user_id, origin=origin, identity_fingerprint="fixture",
            endpoint_epoch="one", authorization_epoch=row.authorization_epoch, enabled=True,
        )
        db.add(endpoint)
        for candidate in (origin, "https://personal.example.test"):
            db.add(BridgeHttpFamily(
                device_id=row.device_id, user_id=row.owner_user_id, origin=candidate, issuer="https://issuer",
                client_binding="fixture", endpoint_epoch="one", authorization_epoch=row.authorization_epoch,
                scope="bridge:read",
            ))
    async with env.db.begin() as db:
        invite = await db.get(Invite, env.invite_id)
        invite.revoked_at = utc_now()
    assert await env.claimable() is None

    async def select_personal(method, path):
        if method == "DELETE":
            async with env.db.begin() as db:
                endpoint = await db.scalar(select(BridgeHttpEndpoint))
                assert not endpoint.enabled or endpoint.origin == "https://personal.example.test"
                endpoint.origin = "https://personal.example.test"
                endpoint.enabled = True

    env.api.before = select_personal
    await worker.reconcile_once()
    async with env.db() as db:
        endpoint = await db.scalar(select(BridgeHttpEndpoint))
        families = {r.origin: r.revoked_at for r in await db.scalars(select(BridgeHttpFamily))}
    assert endpoint.enabled and endpoint.origin == "https://personal.example.test"
    assert families[origin] is not None and families[endpoint.origin] is None
    assert not env.api.dns and not env.api.tunnels


@pytest.mark.asyncio
async def test_current_reader_refreshes_same_session(env):
    worker = env.worker()
    await worker.reconcile_once()
    async with env.db() as db:
        first = await get_current_assignment(db, env.device, env.reports)
        assert first.state == "ready"
        await worker.request_rotation(env.device.id, first.generation)
        assert await get_current_assignment(db, env.device, env.reports) is None
        assert first.state == "removing"


@pytest.mark.asyncio
async def test_provider_does_not_follow_redirects(env):
    env.api.redirect = True
    with pytest.raises(ProviderError, match="HTTP 302"):
        await env.worker().reconcile_once()
    assert env.api.history == [("GET", TUNNELS)]


@pytest.mark.asyncio
async def test_suspended_account_cannot_claim_or_keep_resources(env):
    from skrift.db.models.user import User

    worker = env.worker()
    await worker.reconcile_once()
    async with env.db.begin() as db:
        user = await db.get(User, env.device.owner_user_id)
        user.is_active = False
    assert await env.claimable() is None
    await worker.reconcile_once()
    assert not env.api.tunnels and not env.api.dns


@pytest.mark.asyncio
async def test_rate_limits_and_server_errors_back_off_with_bounded_jitter(env, monkeypatch):
    from buildapp import hosted_tunnel_provisioner

    worker = env.worker()
    delays = []

    def jitter(minimum, maximum):
        delays.append((minimum, maximum))
        return 0.0

    monkeypatch.setattr(hosted_tunnel_provisioner.random, "uniform", jitter)
    reconcile = worker.reconcile_once

    async def failing_pass():
        if len(delays) == 8:
            raise asyncio.CancelledError
        env.api.fail = ("GET", TUNNELS)
        env.api.fail_status = 429 if len(delays) % 2 == 0 else 503
        await reconcile()

    monkeypatch.setattr(worker, "reconcile_once", failing_pass)
    with pytest.raises(asyncio.CancelledError):
        await worker.run()
    assert [maximum for _, maximum in delays] == [1, 2, 4, 8, 16, 32, 60, 60]
    assert all(minimum == maximum / 2 for minimum, maximum in delays)
    assert not env.api.dns and not env.api.tunnels


@pytest.mark.asyncio
async def test_recreated_device_cannot_claim_previous_pairings_assignment(env):
    worker = env.worker()
    await worker.reconcile_once()
    old = await env.row()
    async with env.db.begin() as db:
        await db.delete(await db.get(Device, env.device.id))
    assert await env.claimable() is None
    async with env.db.begin() as db:
        replacement = Device(
            id=env.device.id, owner_user_id=env.device.owner_user_id, approved=True,
            name="replacement", identity_public_key_b64="new-identity", transport_public_key_b64="new-transport",
        )
        db.add(replacement)
    assert replacement.created_at != old.pairing_created_at
    assert replacement.authorization_epoch == old.authorization_epoch
    env.device = replacement
    env.reports.pairings = {PairingIdentity.from_device(replacement)}
    assert await env.claimable() is None
    await worker.reconcile_once()
    await worker.reconcile_once()
    new = await env.claimable()
    assert new.state == "ready"
    assert new.generation > old.generation
    assert new.tunnel_id != old.tunnel_id and new.hostname != old.hostname
    async with env.db() as db:
        assert not (await db.get(Device, replacement.id)).hosted_ready_once
