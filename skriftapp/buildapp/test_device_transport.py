"""Device listings expose the existing registration without changing legacy fields."""

import base64
import json
from types import SimpleNamespace
from uuid import uuid4

import pytest
from litestar.testing import TestClient
from skrift.auth.session_keys import SESSION_USER_ID

from buildapp import (
    bridge_http_auth, device_transport, devices_controller, ephemeral_tokens, ice_servers, internal_auth, pairing_crypto,
)
from buildapp.accounts import account_policy
from buildapp.bridge_http_models import BridgeHttpEndpoint
from buildapp.clock import utc_now
from buildapp.db_test_support import (
    add_member, asgi_app, in_memory_session_maker, session_backend_config, sign_in, sign_in_member,
)
from buildapp.devices_controller import DevicesController
from buildapp.device_transport import rtc_allowed
from buildapp.models import AccountPolicy, Device
from buildapp.rtc_controller import RtcController
from buildapp.test_bridge_http_auth import ISSUER, ORIGIN, registered, signed


@pytest.fixture
def transport_client():
    session_config = session_backend_config()
    app = asgi_app(
        [DevicesController, RtcController],
        session_maker=in_memory_session_maker(),
        session_config=session_config,
    )
    with TestClient(app=app, session_config=session_config) as client:
        yield client


async def store_device(db, user, **fields):
    device = Device(
        owner_user_id=user, name="Laptop", approved=True,
        identity_public_key_b64="aWRlbnRpdHk", transport_public_key_b64="dHJhbnNwb3J0",
        **fields,
    )
    db.add(device)
    await db.commit()
    return device


@pytest.mark.asyncio
@pytest.mark.parametrize("bootstrap", ["on", "off"])
@pytest.mark.parametrize(
    "endpoint_enabled, generation, ready_once, can_bootstrap",
    [
        (False, 2**51, False, True),
        (True, 2**51, False, True),
        (False, None, False, False),
        (True, None, False, False),
        (False, 2**51, True, False),
    ],
    ids=["starting-hosted", "enabled-hosted", "disabled-personal", "enabled-personal", "ready-once"],
)
async def test_rtc_bootstrap_distinguishes_hosted_from_personal(
    db, monkeypatch, bootstrap, endpoint_enabled, generation, ready_once, can_bootstrap,
):
    monkeypatch.setenv("BUILD_RTC_BOOTSTRAP", bootstrap)
    user, _, _ = await registered(db)
    device = await store_device(db, user, hosted_ready_once=ready_once)
    db.add_all([
        AccountPolicy(id=user, tunnel_access=True),
        BridgeHttpEndpoint(
            device_id=device.id, user_id=user, origin=ORIGIN,
            identity_fingerprint=pairing_crypto.fingerprint(device.identity_public_key_b64),
            endpoint_epoch="bootstrap", authorization_epoch=device.authorization_epoch,
            enabled=endpoint_enabled, assignment_generation=generation,
        ),
    ])
    await db.commit()
    assert await rtc_allowed(db, device) is (bootstrap == "on" and can_bootstrap)


@pytest.mark.parametrize("endpoint_state", [
    None, "current", "disabled", "wrong_owner", "stale_epoch", "stale_key",
])
def test_device_transport_listing_is_additive_and_uses_current_personal_endpoint(
    transport_client, endpoint_state,
):
    client = transport_client
    foreign = sign_in_member(client, "foreign@example.com")
    user = sign_in_member(client, "owner@example.com")

    async def seed():
        async with client.app.state.make_session() as db:
            await store_device(db, foreign)
            device = await store_device(db, user)
            if endpoint_state:
                db.add(BridgeHttpEndpoint(
                    device_id=device.id,
                    user_id=foreign if endpoint_state == "wrong_owner" else user,
                    origin=ORIGIN,
                    identity_fingerprint=(
                        "stale" if endpoint_state == "stale_key"
                        else pairing_crypto.fingerprint(device.identity_public_key_b64)
                    ),
                    endpoint_epoch="epoch-one",
                    authorization_epoch=int(endpoint_state == "stale_epoch"),
                    enabled=endpoint_state != "disabled",
                ))
                await db.commit()
            return device

    with client.portal() as portal:
        device = portal.call(seed)
    response = client.get("/api/devices")
    assert response.status_code == 200, response.text
    legacy = {
        "device_id": str(device.id), "id": str(device.id), "name": "Laptop",
        "fingerprint": pairing_crypto.fingerprint(device.identity_public_key_b64),
        "approved": True, "status": "offline", "transport_public_key_b64": "dHJhbnNwb3J0",
        "last_seen_at": None,
    }
    personal = endpoint_state == "current"
    legacy_response = {"devices": [{
        **legacy, "transport": {
            "source": "personal" if personal else "webrtc",
            "rtc_allowed": True,
            "reason": None,
            "origin": ORIGIN if personal else None,
            "assignment_revision": None,
        },
    }]}

    policy = {"tunnel_access": False, "promotion_revision": 0}
    policy_bytes = json.dumps(policy, separators=(",", ":")).encode()
    assert response.json() == {**legacy_response, "transport_policy": policy}
    assert response.content == (
        json.dumps(legacy_response, separators=(",", ":")).encode()[:-1]
        + b',"transport_policy":' + policy_bytes + b"}"
    )


def test_device_transport_entitlement_does_not_grant_membership(transport_client):
    client = transport_client
    email = "outside-alpha@example.com"
    user = sign_in(client, email)

    async def seed():
        async with client.app.state.make_session() as db:
            db.add(AccountPolicy(id=user, account_type="admin", tunnel_access=True))
            await db.commit()
            await store_device(db, user)

    with client.portal() as portal:
        portal.call(seed)
    assert client.get("/api/devices").status_code == 403

    async def enroll():
        async with client.app.state.make_session() as db:
            await add_member(db, user, email)

    with client.portal() as portal:
        portal.call(enroll)
    assert client.get("/api/devices").json()["devices"][0]["transport"]["rtc_allowed"] is True


@pytest.mark.parametrize("bootstrap", [None, "off"], ids=["default-on", "off"])
def test_device_transport_rtc_policy_is_shared_by_listing_relay_and_mints(
    transport_client, monkeypatch, bootstrap,
):
    client = transport_client
    if bootstrap is None:
        monkeypatch.delenv("BUILD_RTC_BOOTSTRAP", raising=False)
    else:
        monkeypatch.setenv("BUILD_RTC_BOOTSTRAP", bootstrap)
    monkeypatch.setenv("INTERNAL_API_SECRET", "relay-test-secret")
    monkeypatch.setattr(internal_auth, "get_settings", lambda: SimpleNamespace(
        internal_api=internal_auth.InternalApiConfig(),
    ))
    foreign_user = sign_in_member(client, "foreign@example.com")
    owner = sign_in_member(client, "entitled@example.com")

    async def seed():
        async with client.app.state.make_session() as db:
            db.add(AccountPolicy(id=owner, tunnel_access=True))
            foreign = await store_device(db, foreign_user)
            ready = await store_device(db, owner, hosted_ready_once=True)
            pending = await store_device(db, owner)
            personal = await store_device(db, owner)
            unapproved = await store_device(db, owner)
            unapproved.approved = False
            db.add(BridgeHttpEndpoint(
                device_id=personal.id, user_id=owner, origin=ORIGIN,
                identity_fingerprint=pairing_crypto.fingerprint(personal.identity_public_key_b64),
                endpoint_epoch="epoch-one", authorization_epoch=personal.authorization_epoch,
                enabled=False,
            ))
            await db.commit()
            return {
                "pending": str(pending.id), "ready": str(ready.id), "personal": str(personal.id),
                "foreign": str(foreign.id), "unapproved": str(unapproved.id),
            }

    with client.portal() as portal:
        devices = portal.call(seed)

    pending_allowed = bootstrap is None
    policy = {devices["pending"]: pending_allowed, devices["ready"]: False, devices["personal"]: False}
    policy_reads = []

    async def read_policy(db_session, user_id):
        policy_reads.append(user_id)
        return await account_policy(db_session, user_id)

    with monkeypatch.context() as listing_only:
        listing_only.setattr(device_transport, "account_policy", read_policy)
        listing_only.setattr(devices_controller, "account_policy", read_policy)
        response = client.get("/api/devices").json()
    assert policy_reads == [owner]
    assert response["transport_policy"] == {"tunnel_access": True, "promotion_revision": 0}
    listing = response["devices"]
    assert {device["id"]: device["transport"]["rtc_allowed"] for device in listing} == policy
    for device_id, allowed in policy.items():
        internal = client.get(
            f"/internal/devices/{device_id}", headers={"x-internal-secret": "relay-test-secret"},
        )
        assert internal.json()["rtc_allowed"] is allowed

    mints = []

    async def gateway(*args, **kwargs):
        mints.append("gateway")
        return "gw_test"

    def ice(*args):
        mints.append("ice")
        return []

    monkeypatch.setattr(ephemeral_tokens, "mint", gateway)
    monkeypatch.setattr(ice_servers, "ice_servers", ice)
    for path in ("/api/gateway-token", "/api/rtc/ice-servers"):
        for target in (None, "invalid", uuid4(), *devices.values()):
            params = {} if target is None else {"device_id": str(target)}
            response = client.post(path, params=params)
            expected = 201 if pending_allowed and target == devices["pending"] else 403
            assert response.status_code == expected, (path, target, response.text)
    assert sorted(mints) == (["gateway", "ice"] if pending_allowed else [])


def test_device_transport_rtc_mints_keep_the_legacy_unscoped_free_path(
    transport_client, monkeypatch,
):
    monkeypatch.delenv(ice_servers.CF_TURN_KEY_ID_ENV, raising=False)
    monkeypatch.delenv(ice_servers.CF_TURN_KEY_API_TOKEN_ENV, raising=False)
    client = transport_client
    sign_in_member(client)
    assert client.post("/api/gateway-token").json()["token"].startswith("gw_")
    assert client.post("/api/rtc/ice-servers").json() == {"iceServers": ice_servers.STUN_ONLY}


def test_device_listing_discloses_promotion_even_without_any_paired_devices(transport_client):
    client = transport_client
    user = sign_in_member(client, "notice-owner@example.com")

    async def seed():
        async with client.app.state.make_session() as db:
            db.add(AccountPolicy(id=user, account_type="admin", tunnel_access=True, promotion_revision=2))
            await db.commit()

    with client.portal() as portal:
        portal.call(seed)
    assert client.get("/api/devices").json() == {
        "devices": [], "transport_policy": {"tunnel_access": True, "promotion_revision": 2},
    }


@pytest.mark.asyncio
async def test_device_transport_readiness_survives_https_reregistration(db):
    from buildapp.device_transport import mark_hosted_ready_once

    user, device, key = await registered(db)
    assert device.hosted_ready_once is False
    assert await mark_hosted_ready_once(db, device) is True
    assert await mark_hosted_ready_once(db, device) is False
    await db.commit()

    challenge = await bridge_http_auth.registration_challenge(
        db, device.id, signed(key, device, "registration-challenge"), ISSUER, utc_now(),
    )
    proof = signed(
        key, device, "registration", challenge_id=challenge["challenge_id"],
        nonce=challenge["nonce"], origin=ORIGIN, endpoint_epoch="epoch-two",
        previous_endpoint_epoch="epoch-one", enabled=True,
    )
    await bridge_http_auth.register_endpoint(db, device.id, proof, ISSUER, utc_now())
    await db.commit()
    await db.refresh(device)
    assert device.hosted_ready_once is True
    assert device.owner_user_id == user


@pytest.mark.parametrize("snapshot_while_revoked", [False, True], ids=["approved", "revoked"])
def test_device_transport_readiness_resets_when_device_is_paired_again(
    transport_client, snapshot_while_revoked,
):
    from buildapp.device_transport import mark_hosted_ready_once

    client = transport_client

    async def seed():
        async with client.app.state.make_session() as db:
            owner, device, key = await registered(db, "first-owner@example.com")
            assert await mark_hosted_ready_once(db, device)
            await db.commit()
            return owner, device, key

    with client.portal() as portal:
        owner, old, key = portal.call(seed)
    client.set_session_data({SESSION_USER_ID: str(owner)})
    response = client.post(f"/api/devices/{old.id}/revoke")
    assert response.is_success, response.text

    async def read_revoked():
        async with client.app.state.make_session() as db:
            return await db.get(Device, old.id)

    snapshot = old
    if snapshot_while_revoked:
        with client.portal() as portal:
            snapshot = portal.call(read_revoked)
        assert snapshot.approved is False

    code = "ABCD-EFGH"
    code_hash = pairing_crypto.hash_code(code)
    challenge = pairing_crypto.registration_challenge(
        str(old.id), old.identity_public_key_b64, old.transport_public_key_b64, code_hash,
    )
    response = client.post("/api/devices/register", json={
        "device_id": str(old.id), "name": old.name,
        "identity_public_key_b64": old.identity_public_key_b64,
        "transport_public_key_b64": old.transport_public_key_b64,
        "pairing_code_hash": code_hash,
        "signature_b64": base64.b64encode(key.sign(challenge.encode())).decode(),
    })
    assert response.is_success, response.text
    response = client.post("/api/devices/approve", json={"code": code})
    assert response.is_success, response.text

    async def inspect():
        async with client.app.state.make_session() as db:
            current = await db.get(Device, old.id)
            assert current.approved is True and current.owner_user_id == owner
            assert current.hosted_ready_once is False
            if snapshot_while_revoked:
                assert current.authorization_epoch == snapshot.authorization_epoch
            assert await mark_hosted_ready_once(db, snapshot) is False
            await db.refresh(current)
            assert current.hosted_ready_once is False
            assert await mark_hosted_ready_once(db, current) is True

    with client.portal() as portal:
        portal.call(inspect)
