"""The production adapter carries the same authority rules as service calls."""

import hashlib
import secrets
from uuid import UUID

from litestar.testing import TestClient
from litestar.di import Provide
from sqlalchemy import select

from buildapp.bridge_http_controller import BridgeHttpController
from buildapp.db_test_support import (
    asgi_app,
    in_memory_session_maker,
    session_backend_config,
    sign_in_member,
)
from buildapp.models import BridgeHttpFamily, BridgeHttpClient
from buildapp.desktop_auth import BRIDGE_ACCOUNT_KEY, BRIDGE_BINDING_KEY
from buildapp.token_hash import token_hash
from buildapp.test_bridge_http_auth import ISSUER, ORIGIN, paired, signed


def authenticated_adapter_context(client):
    """A hand-assembled adapter unit's explicit completed-login precondition."""
    user = sign_in_member(client)
    binding = "browser:" + secrets.token_urlsafe(32)
    session = client.get_session_data()
    session.update({BRIDGE_ACCOUNT_KEY: str(user), BRIDGE_BINDING_KEY: binding})
    client.set_session_data(session)

    async def seed():
        async with client.app.state.make_session() as db:
            db.add(BridgeHttpClient(user_id=user, binding_hash=token_hash(binding)))
            await db.commit()

    with client.portal() as portal:
        portal.call(seed)
    return user


def test_routes_discover_renew_revoke_and_never_accept_client_authority(monkeypatch):
    monkeypatch.setattr(
        BridgeHttpController,
        "dependencies",
        {"public_base_url": Provide(lambda: ISSUER, sync_to_thread=False)},
    )
    app = asgi_app(
        [BridgeHttpController],
        session_maker=in_memory_session_maker(),
        session_config=session_backend_config(),
    )
    with TestClient(app=app, session_config=session_backend_config()) as client:
        user = authenticated_adapter_context(client)

        async def seed():
            async with app.state.make_session() as db:
                _, device, key = await paired(db)
                device.owner_user_id = user
                await db.commit()
                return device, key

        with client.portal() as portal:
            device, key = portal.call(seed)
        challenge = client.post(
            f"/api/devices/{device.id}/http-endpoint/challenge",
            json={"proof": signed(key, device, "registration-challenge")},
        )
        assert challenge.status_code == 201, challenge.text
        proof = signed(
            key,
            device,
            "registration",
            challenge_id=challenge.json()["challenge_id"],
            nonce=challenge.json()["nonce"],
            origin=ORIGIN,
            endpoint_epoch="one",
            previous_endpoint_epoch=None,
            enabled=True,
        )
        result = client.post(
            f"/api/devices/{device.id}/http-endpoint", json={"proof": proof}
        )
        assert result.status_code == 201, result.text
        pending = client.post(
            "/api/bridge-discovery/challenges", json={"origin": ORIGIN}
        )
        assert pending.status_code == 201, pending.text
        pending = pending.json()
        assert pending["account_id"] == str(user)
        assert (
            client.get(
                f"/api/bridge-discovery/challenges/{pending['challenge_id']}"
            ).json()
            == pending
        )
        proof = signed(
            key,
            device,
            "discovery",
            challenge_id=pending["challenge_id"],
            nonce=pending["nonce"],
            origin=ORIGIN,
            endpoint_epoch="one",
            issuer=ISSUER,
        )
        result = client.post(
            "/api/bridge-discovery/complete",
            json={"challenge_id": pending["challenge_id"], "proof": proof},
        )
        assert result.status_code == 201, result.text
        minted = result.json()
        request = {"family_id": minted["family_id"], "endpoint_epoch": "one"}
        refreshed = client.post(
            f"/api/devices/{device.id}/http-credentials", json=request
        )
        assert refreshed.status_code == 201, refreshed.text
        assert refreshed.json()["access_token"] != minted["access_token"]
        assertion = signed(
            key,
            device,
            "introspection",
            token_hash=hashlib.sha256(minted["access_token"].encode()).hexdigest(),
        )
        active = client.post(
            "/api/bridge-http/introspect",
            json={"token": minted["access_token"], "client_assertion": assertion},
        )
        assert active.status_code == 200, active.text
        assert active.json()["account_id"] == str(user)
        assert "access_token" not in active.json()
        assert (
            client.delete(
                f"/api/devices/{device.id}/http-credentials/{minted['family_id']}"
            ).status_code
            == 204
        )
        assert (
            client.post(
                f"/api/devices/{device.id}/http-credentials", json=request
            ).status_code
            == 401
        )

        async def read():
            async with app.state.make_session() as db:
                return (await db.scalars(select(BridgeHttpFamily))).all()

        with client.portal() as portal:
            families = portal.call(read)
        assert len(families) == 1
        assert families[0].revoked_at is not None
        assert isinstance(families[0].user_id, UUID)


def test_discovery_routes_require_membership_and_json_and_bound_body(monkeypatch):
    monkeypatch.setattr(
        BridgeHttpController,
        "dependencies",
        {"public_base_url": Provide(lambda: ISSUER, sync_to_thread=False)},
    )
    app = asgi_app(
        [BridgeHttpController],
        session_maker=in_memory_session_maker(),
        session_config=session_backend_config(),
    )
    with TestClient(app=app, session_config=session_backend_config()) as client:
        assert (
            client.post(
                "/api/bridge-discovery/challenges", json={"origin": ORIGIN}
            ).status_code
            == 401
        )
        authenticated_adapter_context(client)
        assert (
            client.post(
                "/api/bridge-discovery/challenges", json={"origin": "http://host"}
            ).status_code
            == 400
        )
        assert (
            client.post(
                "/api/bridge-discovery/challenges",
                json={"origin": ORIGIN, "account_id": "forged"},
            ).status_code
            == 400
        )
        assert (
            client.post(
                "/api/bridge-discovery/challenges",
                content='{"origin":"' + ORIGIN + '"}',
                headers={"Content-Type": "text/plain"},
            ).status_code
            == 400
        )
        assert client.post(
            "/api/bridge-discovery/challenges", json={"origin": "a" * 3000}
        ).status_code in (400, 413)
