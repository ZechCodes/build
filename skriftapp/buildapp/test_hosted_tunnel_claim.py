"""End-to-end claim contract and its effects on existing endpoint authority."""

import asyncio
import base64
from dataclasses import dataclass, field, replace
from datetime import datetime, timedelta
from uuid import UUID, uuid4

import jwt
import pytest
from litestar.di import Provide
from litestar.exceptions import ClientException, NotAuthorizedException
from litestar.testing import TestClient
from skrift.db.models.user import User

from buildapp import bridge_http_auth as auth
from buildapp import hosted_tunnel_claim as claim
from buildapp import hosted_tunnel_source as source
from buildapp.bridge_http_controller import BridgeHttpController
from buildapp.clock import utc_now
from buildapp.db_test_support import (
    asgi_app,
    create_skrift_tables,
    engine_for,
    file_session_maker,
    in_memory_session_maker,
)
from buildapp.models import AccountPolicy, Device
from buildapp.test_bridge_http_auth import ISSUER, ORIGIN, paired, registered, signed

ZONE = "hosted.example"
PERSONAL = {"mode": "personal", "hosted_capable": True}
HOSTED = {"mode": "hosted", "hosted_capable": True}
SYNTHETIC_SECRET = base64.b64encode(b"synthetic-test-marker-for-a-tunnel").decode()
CHALLENGE = "hosted-tunnel-challenge"
CLAIM = "hosted-tunnel-claim"
BINDING = "browser:client"


@dataclass
class FakeAssignment:
    device_id: UUID
    owner_user_id: UUID
    pairing_created_at: datetime
    authorization_epoch: int
    generation: int = 1
    label: str = "a" * 32
    hostname: str = f"b-{'a' * 32}.{ZONE}"
    tunnel_id: UUID | None = field(default_factory=uuid4)
    dns_record_id: str | None = "synthetic-dns-record"
    state: str = "ready"
    setup_reason: str | None = None
    encrypted_secret: bytes = field(default=b"fake-encrypted-material", repr=False)


class FakeAssignments:
    row = None
    after_materialize = None

    async def get_current_assignment(self, db_session, pairing, reports):
        if self.row and pairing in await reports.hosted_pairings(db_session):
            return self.row
        return None

    def materialize_connector_credentials(self, assignment):
        credentials = {
            "AccountTag": "synthetic-account",
            "TunnelID": str(assignment.tunnel_id),
            "TunnelSecret": SYNTHETIC_SECRET,
        }
        if self.after_materialize:
            self.after_materialize()
        return credentials

    def assign(self, device, **changes):
        self.row = FakeAssignment(
            device.id, device.owner_user_id, device.created_at,
            device.authorization_epoch, **changes,
        )
        return self.row


@pytest.fixture()
def assignments(monkeypatch):
    fake = FakeAssignments()
    monkeypatch.setenv("HOSTED_TUNNEL_ZONE_NAME", ZONE)
    monkeypatch.setattr(source, "get_current_assignment", fake.get_current_assignment)
    monkeypatch.setattr(source, "materialize_credentials", fake.materialize_connector_credentials)
    return fake


async def entitled(db, factory=paired):
    user, device, key = await factory(db)
    db.add(AccountPolicy(id=user, tunnel_access=True, promotion_revision=1))
    await db.commit()
    return user, device, key


def proof_for(key, device, challenge, report=HOSTED):
    return signed(
        key, device, CLAIM, **report, generation=challenge["generation"],
        challenge_id=challenge["challenge_id"], nonce=challenge["nonce"],
    )


async def reset(db, device):
    await db.rollback()
    await db.refresh(device)


async def begin(db, device, key, report=HOSTED):
    result = await claim.issue_challenge(
        db, device.id, signed(key, device, CHALLENGE, **report), ISSUER, utc_now(),
    )
    await db.commit()
    return result


async def submit(db, device, key, report=HOSTED):
    pending = await begin(db, device, key, report)
    result = await claim.claim_assignment(
        db, device.id, proof_for(key, device, pending, report), ISSUER, utc_now(),
    )
    await db.commit()
    return result


@pytest.mark.asyncio
async def test_mode_and_policy_are_derived_only_from_signed_reports(db, assignments):
    user, device, key = await paired(db)
    assignments.assign(device)
    assert await submit(db, device, key) == {"status": "off"}
    assert await source.hosted_mode_source.hosted_pairings(db) == {
        source.PairingIdentity.from_device(device)
    }
    db.add(AccountPolicy(id=user, tunnel_access=True, promotion_revision=1))
    await db.commit()
    assert await submit(db, device, key, {"mode": "hosted", "hosted_capable": False}) == {
        "status": "setup", "reason": "update_required",
    }
    assert not await source.hosted_mode_source.hosted_pairings(db)
    assert await submit(db, device, key, PERSONAL) == {"status": "personal"}
    assert not await source.hosted_mode_source.hosted_pairings(db)
    assert await submit(db, device, key) == {"status": "setup", "reason": "assignment_changed"}
    assert await source.hosted_mode_source.hosted_pairings(db) == {
        source.PairingIdentity.from_device(device)
    }
    result = await submit(db, device, key)
    assert result == {
        "status": "assigned", "hostname": assignments.row.hostname, "generation": 1,
        "credentials": assignments.materialize_connector_credentials(assignments.row),
    }


@pytest.mark.asyncio
async def test_replay_expiry_and_wrong_audience_or_purpose_are_refused(db, assignments):
    _, device, key = await entitled(db)
    raw = signed(key, device, CHALLENGE, **HOSTED)
    pending = await claim.issue_challenge(db, device.id, raw, ISSUER, utc_now())
    await db.commit()
    with pytest.raises(NotAuthorizedException):
        await claim.issue_challenge(db, device.id, raw, ISSUER, utc_now())
    await reset(db, device)
    raw = proof_for(key, device, pending)
    for invalid, issuer, now in (
        (signed(key, device, "registration"), ISSUER, utc_now()),
        (raw, "https://foreign.example", utc_now()),
        (raw, ISSUER, utc_now() + timedelta(seconds=36)),
    ):
        with pytest.raises(NotAuthorizedException):
            await claim.claim_assignment(db, device.id, invalid, issuer, now)
        await reset(db, device)
    expected = await claim.claim_assignment(db, device.id, raw, ISSUER, utc_now())
    assert expected == {"status": "setup", "reason": "pending"}
    await db.commit()
    for repeated in (raw, proof_for(key, device, pending)):
        with pytest.raises(NotAuthorizedException):
            await claim.claim_assignment(db, device.id, repeated, ISSUER, utc_now())
        await reset(db, device)


@pytest.mark.asyncio
async def test_stale_generation_and_rotation_during_materialization_never_release(db, assignments):
    _, device, key = await entitled(db)
    await submit(db, device, key)
    initial = assignments.assign(device)
    pending = await begin(db, device, key)
    assert pending["generation"] == 1
    assignments.row = replace(initial, generation=2)
    result = await claim.claim_assignment(
        db, device.id, proof_for(key, device, pending), ISSUER, utc_now(),
    )
    assert result == {"status": "setup", "reason": "assignment_changed"}
    await db.commit()

    def rotate():
        assignments.row.generation = 3

    assignments.after_materialize = rotate
    assert await submit(db, device, key) == {"status": "setup", "reason": "assignment_changed"}


@pytest.mark.asyncio
async def test_cross_account_assignment_and_stale_owner_are_refused(db, assignments):
    _, device, key = await entitled(db)
    other, other_device, other_key = await paired(db, "other@example.com")
    await submit(db, device, key)
    # The issuer validates assignment identity even if a source returns a foreign row.
    assignments.assign(other_device)
    pending = await begin(db, device, key)
    assert pending["generation"] is None
    assert await submit(db, device, key) == {"status": "setup", "reason": "pending"}
    with pytest.raises(NotAuthorizedException):
        await claim.claim_assignment(
            db, device.id, proof_for(other_key, device, pending), ISSUER, utc_now(),
        )
    await reset(db, device)
    device.owner_user_id = other
    await db.commit()
    with pytest.raises(NotAuthorizedException):
        await claim.claim_assignment(
            db, device.id, proof_for(key, device, pending), ISSUER, utc_now(),
        )


@pytest.mark.asyncio
async def test_suspension_and_newer_personal_mode_fence_outstanding_challenges(db, assignments):
    user, device, key = await entitled(db)
    await submit(db, device, key)
    assignments.assign(device)
    old = await begin(db, device, key)
    assert await submit(db, device, key, PERSONAL) == {"status": "personal"}
    assert not await source.hosted_mode_source.hosted_pairings(db)
    with pytest.raises(NotAuthorizedException):
        await claim.claim_assignment(db, device.id, proof_for(key, device, old), ISSUER, utc_now())
    await reset(db, device)
    pending = await begin(db, device, key)
    account = await db.get(User, user)
    account.is_active = False
    await db.commit()
    with pytest.raises(NotAuthorizedException):
        await claim.claim_assignment(db, device.id, proof_for(key, device, pending), ISSUER, utc_now())
    await reset(db, device)
    with pytest.raises(NotAuthorizedException):
        await begin(db, device, key)


@pytest.mark.asyncio
async def test_separate_workers_cannot_spend_one_challenge_with_distinct_proofs(tmp_path, assignments):
    maker = file_session_maker(tmp_path / "claim.sqlite")
    engine = engine_for(maker)
    await create_skrift_tables(engine)
    try:
        async with maker() as db:
            _, device, key = await entitled(db)
            await submit(db, device, key)
            assignments.assign(device)
            pending = await begin(db, device, key)

        async def compete(proof):
            async with maker() as db:
                try:
                    result = await claim.claim_assignment(db, device.id, proof, ISSUER, utc_now())
                    await db.commit()
                    return result["status"]
                except NotAuthorizedException:
                    return "refused"

        results = await asyncio.gather(*(compete(proof_for(key, device, pending)) for _ in range(2)))
        assert sorted(results) == ["assigned", "refused"]
    finally:
        await engine.dispose()


async def register(db, device, key, origin, epoch, previous, *, generation=None, enabled=True):
    pending = await auth.registration_challenge(
        db, device.id, signed(key, device, "registration-challenge"), ISSUER, utc_now(),
    )
    result = await auth.register_endpoint(
        db, device.id,
        signed(
            key, device, "registration", **pending, origin=origin, endpoint_epoch=epoch,
            previous_endpoint_epoch=previous, enabled=enabled,
            **({"assignment_generation": generation} if generation is not None else {}),
        ),
        ISSUER, utc_now(),
    )
    await db.commit()
    return result


async def discovery(db, user, device, key, origin, epoch):
    pending = await auth.create_discovery(db, user, BINDING, origin, ISSUER, utc_now())
    proof = signed(
        key, device, "discovery", challenge_id=pending["challenge_id"], nonce=pending["nonce"],
        issuer=ISSUER, origin=origin, endpoint_epoch=epoch,
    )
    return await auth.complete_discovery(
        db, user, BINDING, pending["challenge_id"], proof, ISSUER, utc_now(),
    )


async def introspect(db, device, key, minted):
    token = minted["access_token"]
    assertion = signed(key, device, "introspection", token_hash=auth.hash_token(token))
    return await auth.introspect(db, token, assertion, ISSUER, utc_now())


async def setup_hosted_endpoint(db, assignments):
    user, device, key = await entitled(db, registered)
    await submit(db, device, key)
    row = assignments.assign(device)
    origin = f"https://{row.hostname}"
    personal_grant = await discovery(db, user, device, key, ORIGIN, "epoch-one")
    await db.commit()
    with pytest.raises(ClientException) as conflict:
        await register(db, device, key, origin, "hosted", None, generation=1)
    assert conflict.value.status_code == 409
    await reset(db, device)
    await register(db, device, key, origin, "starting", "epoch-one", generation=1, enabled=False)
    assert not device.hosted_ready_once
    await register(db, device, key, origin, "hosted", "starting", generation=1)
    assert device.hosted_ready_once
    assert await introspect(db, device, key, personal_grant) == {"active": False}
    minted = await discovery(db, user, device, key, origin, "hosted")
    await db.commit()
    return user, device, key, origin, minted


@pytest.mark.asyncio
async def test_obsolete_hosted_authority_refuses_all_paths(db, assignments):
    user, device, key, origin, minted = await setup_hosted_endpoint(db, assignments)
    assert (await introspect(db, device, key, minted))["active"]
    assignments.row = replace(assignments.row, generation=2)
    assert await introspect(db, device, key, minted) == {"active": False}
    await db.commit()
    with pytest.raises(NotAuthorizedException):
        await auth.renew(db, user, BINDING, device.id, minted["family_id"], "hosted", ISSUER, utc_now())
    await reset(db, device)
    with pytest.raises(NotAuthorizedException):
        await discovery(db, user, device, key, origin, "hosted")
    await reset(db, device)
    for generation in (None, 1):
        with pytest.raises(NotAuthorizedException):
            await register(db, device, key, origin, "late", "hosted", generation=generation)
        await reset(db, device)
    assignments.row = None
    with pytest.raises(NotAuthorizedException):
        await register(db, device, key, origin, "late", "hosted", generation=1)


@pytest.mark.asyncio
async def test_personal_switch_withdraws_hosted_authority_and_survives_stale_cleanup(db, assignments):
    user, device, key, origin, minted = await setup_hosted_endpoint(db, assignments)
    assert await submit(db, device, key, PERSONAL) == {"status": "personal"}
    assert not await source.hosted_mode_source.hosted_pairings(db)
    assert not (await auth._endpoint(db, device.id)).enabled
    assert await introspect(db, device, key, minted) == {"active": False}
    await db.commit()
    with pytest.raises(ClientException) as conflict:
        await register(db, device, key, ORIGIN, "personal", "epoch-one")
    assert conflict.value.status_code == 409
    await reset(db, device)
    await register(db, device, key, ORIGIN, "personal", "hosted")
    fresh = await discovery(db, user, device, key, ORIGIN, "personal")
    await db.commit()
    await auth.withdraw_hosted_registration(db, device.id, origin, 1, utc_now())
    await db.commit()
    assert (await auth._endpoint(db, device.id)).origin == ORIGIN
    assert (await introspect(db, device, key, fresh))["active"]
    assert (await db.get(Device, device.id)).hosted_ready_once


def test_claim_http_cache_bounds_and_secret_redaction(monkeypatch, assignments, caplog):
    monkeypatch.setattr(
        BridgeHttpController, "dependencies", {"public_base_url": Provide(lambda: ISSUER, sync_to_thread=False)}
    )
    maker = in_memory_session_maker()
    app = asgi_app([BridgeHttpController], session_maker=maker)

    async def seed():
        async with maker() as db:
            _, device, key = await entitled(db)
            await submit(db, device, key)
            assignments.assign(device)
            return device, key

    with TestClient(app=app, raise_server_exceptions=False) as client:
        with client.portal() as portal:
            device, key = portal.call(seed)
        path = f"/api/devices/{device.id}/hosted-tunnel"
        oversized = client.post(path, json={"proof": "a" * 18000}, follow_redirects=False)
        assert oversized.status_code == 413
        assert oversized.headers["cache-control"] == "no-store"
        challenge = client.post(
            path + "/challenge", json={"proof": signed(key, device, CHALLENGE, **HOSTED)},
            follow_redirects=False,
        )
        assert challenge.status_code == 201
        assert challenge.headers["cache-control"] == "no-store"
        assert SYNTHETIC_SECRET not in challenge.text
        proof = proof_for(key, device, challenge.json())
        delivered = client.post(path, json={"proof": proof}, follow_redirects=False)
        assert delivered.status_code == 200
        assert delivered.headers["cache-control"] == "no-store"
        assert delivered.json()["credentials"]["TunnelSecret"] == SYNTHETIC_SECRET
        replay = client.post(path, json={"proof": proof}, follow_redirects=False)
        assert replay.status_code == 401
        assert SYNTHETIC_SECRET not in replay.text

        def fail():
            raise RuntimeError(SYNTHETIC_SECRET)

        assignments.after_materialize = fail
        fresh = client.post(path + "/challenge", json={"proof": signed(key, device, CHALLENGE, **HOSTED)})
        failed = client.post(path, json={"proof": proof_for(key, device, fresh.json())})
        assert failed.status_code == 503
        assert failed.headers["cache-control"] == "no-store"
        assert SYNTHETIC_SECRET not in failed.text
    assert SYNTHETIC_SECRET not in caplog.text


def test_signed_fixture_has_expected_proof_fields():
    # The committed cross-language fixture is checked independently of a clock.
    from pathlib import Path
    import json
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey

    fixture = json.loads((Path(__file__).parents[2] / "planning/v2/fixtures/hosted-tunnel-claim.json").read_text())
    key = Ed25519PublicKey.from_public_bytes(base64.b64decode(fixture["public_key_b64"]))
    for example in fixture["cases"]:
        proof = example["request"]["body"]["proof"]
        assert jwt.get_unverified_header(proof) == {"alg": "EdDSA", "typ": auth.TYPES[CLAIM]}
        payload = jwt.decode(
            proof, key, algorithms=["EdDSA"], audience=fixture["issuer"] + example["request"]["path"],
            options={"verify_exp": False, "verify_iat": False},
        )
        assert payload == example["proof_claims"]
