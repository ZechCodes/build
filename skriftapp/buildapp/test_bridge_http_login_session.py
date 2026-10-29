"""Replay the earliest cookie issued by real primary login, before bridge use."""

from __future__ import annotations

import base64
from uuid import uuid4

import jwt
import pytest
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
from skrift.auth.session_keys import SESSION_USER_ID
from sqlalchemy import func, select

from buildapp import skrift_app_test_support as framework
from buildapp.bridge_http_proofs import TYPES, audience
from buildapp.clock import utc_now
from buildapp.db_test_support import add_member
from buildapp.desktop_auth import BRIDGE_ACCOUNT_KEY, BRIDGE_BINDING_KEY
from buildapp.models import BridgeHttpFamily, Device
from buildapp.bridge_http_test_support import (
    build_login_app,
    framework_client,
    csrf,
    real_login,
    cookie_snapshot,
    restore,
)

ORIGIN = "https://bridge.example"


@pytest.fixture()
def login_app(tmp_path, monkeypatch):
    yield from build_login_app(tmp_path, monkeypatch)


def signed(key, device, purpose, **fields):
    now = int(utc_now().timestamp())
    return jwt.encode(
        {
            "iss": str(device.id),
            "sub": str(device.id),
            "aud": audience(framework.SECURE_ORIGIN, purpose, str(device.id)),
            "iat": now,
            "exp": now + 30,
            "jti": str(uuid4()),
            **fields,
        },
        key,
        algorithm="EdDSA",
        headers={"typ": TYPES[purpose]},
    )


def enroll_and_register(client, user):
    key = Ed25519PrivateKey.generate()

    async def seed(db):
        await add_member(db, user, "login-replay@example.com")
        device = Device(
            name="Replay regression bridge",
            owner_user_id=user,
            approved=True,
            identity_public_key_b64=base64.b64encode(
                key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
            ).decode(),
            transport_public_key_b64="unused",
        )
        db.add(device)
        await db.commit()
        return device

    device = framework.on_database(client, seed)
    result = client.post(f"/api/devices/{device.id}/http-endpoint/challenge", json={})
    assert result.status_code == 201, result.text
    pending = result.json()
    proof = signed(
        key,
        device,
        "registration",
        challenge_id=pending["challenge_id"],
        nonce=pending["nonce"],
        origin=ORIGIN,
        endpoint_epoch="login-epoch",
        previous_endpoint_epoch=None,
        enabled=True,
    )
    result = client.post(
        f"/api/devices/{device.id}/http-endpoint", json={"proof": proof}
    )
    assert result.status_code == 201, result.text
    return device, key


def mint(client, device, key):
    result = client.post("/api/bridge-discovery/challenges", json={"origin": ORIGIN})
    assert result.status_code == 201, result.text
    pending = result.json()
    proof = signed(
        key,
        device,
        "discovery",
        challenge_id=pending["challenge_id"],
        nonce=pending["nonce"],
        origin=ORIGIN,
        endpoint_epoch="login-epoch",
        issuer=framework.SECURE_ORIGIN,
    )
    result = client.post(
        "/api/bridge-discovery/complete",
        json={"challenge_id": pending["challenge_id"], "proof": proof},
    )
    assert result.status_code == 201, result.text
    return result.json()


def active(client, device, key, token):
    from buildapp.token_hash import token_hash

    assertion = signed(key, device, "introspection", token_hash=token_hash(token))
    result = client.post(
        "/api/bridge-http/introspect",
        json={"token": token, "client_assertion": assertion},
    )
    assert result.status_code == 200, result.text
    return result.json()["active"]


def family_count(client):
    async def read(db):
        return await db.scalar(select(func.count()).select_from(BridgeHttpFamily))

    return framework.on_database(client, read)


def test_real_earliest_login_cookie_cannot_remint_after_csrf_logout(login_app):
    with framework_client(login_app) as client:
        user, earliest = real_login(client)
        device, key = enroll_and_register(client, user)
        old = mint(client, device, key)
        pending = client.post(
            "/api/bridge-discovery/challenges", json={"origin": ORIGIN}
        ).json()
        first_bound_cookies = cookie_snapshot(client)
        other_user, _ = real_login(client)
        assert other_user == user
        unrelated = mint(client, device, key)
        unrelated_cookies = cookie_snapshot(client)
        assert family_count(client) == 2
        restore(client, first_bound_cookies)
        result = client.post(
            "/auth/logout",
            data={"_csrf": csrf(client.get("/auth/logout"))},
            follow_redirects=False,
        )
        assert (
            result.status_code == 302
            and SESSION_USER_ID not in client.get_session_data()
        )
        assert active(client, device, key, old["access_token"]) is False
        restore(client, earliest)
        replay = client.post(
            "/api/bridge-discovery/challenges", json={"origin": ORIGIN}
        )
        assert replay.status_code == 401, replay.text
        assert family_count(client) == 2
        assert (
            client.get(
                f"/api/bridge-discovery/challenges/{pending['challenge_id']}"
            ).status_code
            == 401
        )
        assert (
            client.post(
                f"/api/devices/{device.id}/http-credentials",
                json={"family_id": old["family_id"], "endpoint_epoch": "login-epoch"},
            ).status_code
            == 401
        )
        restore(client, unrelated_cookies)
        assert active(client, device, key, unrelated["access_token"]) is True
        assert (
            client.post(
                f"/api/devices/{device.id}/http-credentials",
                json={
                    "family_id": unrelated["family_id"],
                    "endpoint_epoch": "login-epoch",
                },
            ).status_code
            == 201
        )


def test_authenticated_legacy_cookie_missing_bridge_identity_cannot_initialize_one(
    login_app,
):
    with framework_client(login_app) as client:
        user, _ = real_login(client)

        async def enroll(db):
            await add_member(db, user, "login-replay@example.com")

        framework.on_database(client, enroll)
        legacy = client.get_session_data()
        # Model a pre-upgrade cookie using actual authenticated session values;
        # never manufacture a replacement stable bridge identity.
        legacy.pop(BRIDGE_BINDING_KEY, None)
        legacy.pop(BRIDGE_ACCOUNT_KEY, None)
        client.cookies.clear()
        for cookie in client.session_backend._create_session_cookies(
            client.session_backend.dump_data(legacy)
        ):
            client.cookies.set(
                cookie.key, cookie.value, domain="testserver.local", path="/"
            )
        result = client.post(
            "/api/bridge-discovery/challenges", json={"origin": ORIGIN}
        )
        assert result.status_code == 401, result.text
        assert BRIDGE_BINDING_KEY not in client.get_session_data()
        assert family_count(client) == 0


def test_failed_login_identity_persistence_returns_no_authenticated_cookie(
    login_app, monkeypatch
):
    from buildapp import bridge_http_login

    def unavailable_database(request):
        raise RuntimeError("isolated login identity persistence failure")

    with framework_client(login_app, raise_server_exceptions=False) as client:
        page = client.get("/auth/dummy/login", follow_redirects=False)
        monkeypatch.setattr(
            bridge_http_login, "auth_session_maker", unavailable_database
        )
        result = client.post(
            "/auth/dummy-login",
            data={
                "_csrf": csrf(page),
                "email": "failed-login@example.com",
                "name": "Failed identity persistence",
            },
            follow_redirects=False,
        )
        assert result.status_code == 500, result.text
        session = client.get_session_data()
        assert SESSION_USER_ID not in session
        assert BRIDGE_BINDING_KEY not in session
        assert (
            client.post(
                "/api/bridge-discovery/challenges", json={"origin": ORIGIN}
            ).status_code
            == 401
        )

        async def identities(db):
            from buildapp.models import BridgeHttpClient

            return await db.scalar(select(func.count()).select_from(BridgeHttpClient))

        assert framework.on_database(client, identities) == 0
