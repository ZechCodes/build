"""Device listings expose the existing registration without changing legacy fields."""

import base64
import json

import pytest
from litestar.testing import TestClient
from skrift.auth.session_keys import SESSION_USER_ID

from buildapp import bridge_http_auth, pairing_crypto
from buildapp.bridge_http_models import BridgeHttpEndpoint
from buildapp.clock import utc_now
from buildapp.db_test_support import (
    add_member, asgi_app, in_memory_session_maker, session_backend_config, sign_in, sign_in_member,
)
from buildapp.devices_controller import DevicesController
from buildapp.models import Device
from buildapp.test_bridge_http_auth import ISSUER, ORIGIN, registered, signed


@pytest.fixture
def transport_client():
    session_config = session_backend_config()
    app = asgi_app(
        [DevicesController],
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
    legacy_bytes = json.dumps(legacy, separators=(",", ":")).encode()
    assert response.content.startswith(b'{"devices":[' + legacy_bytes[:-1] + b',"transport":')
    personal = endpoint_state == "current"
    assert response.json() == {"devices": [{
        **legacy, "transport": {
            "source": "personal" if personal else "webrtc",
            "rtc_allowed": True,
            "reason": None,
            "origin": ORIGIN if personal else None,
            "assignment_revision": None,
        },
    }]}


def test_device_transport_entitlement_neither_grants_membership_nor_enforces_rtc(transport_client):
    from buildapp.models import AccountPolicy

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
