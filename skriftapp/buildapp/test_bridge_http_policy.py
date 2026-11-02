"""Bounded bridge origins and client lifetimes through Skrift's assembled stack."""

from __future__ import annotations

from types import SimpleNamespace
import secrets
from uuid import uuid4

import pytest
from litestar.exceptions import NotAuthorizedException
from skrift.auth.session_keys import SESSION_USER_ID

from buildapp.bridge_http_policy import connect_policy
from buildapp.desktop_auth import (
    bridge_http_client_binding,
    BRIDGE_BINDING_KEY,
    BRIDGE_ACCOUNT_KEY,
)
from buildapp.bridge_http_test_support import (
    build_login_app,
    framework_client,
    real_login,
)


def request(user=None, *, state=None, binding=None):
    session = {SESSION_USER_ID: str(user)} if user else {}
    if binding:
        session.update({BRIDGE_BINDING_KEY: binding, BRIDGE_ACCOUNT_KEY: str(user)})
    return SimpleNamespace(
        session=session,
        scope={"state": state or {}},
    )


def test_browser_binding_survives_requests_and_refuses_account_switch():
    req = request(uuid4(), binding="browser:" + secrets.token_urlsafe(32))
    first = bridge_http_client_binding(req)
    assert first.startswith("browser:") and len(first) > 40
    assert bridge_http_client_binding(req) == first
    req.session[SESSION_USER_ID] = str(uuid4())
    with pytest.raises(NotAuthorizedException):
        bridge_http_client_binding(req)


def test_browser_families_are_independent_even_for_the_same_account():
    user = uuid4()
    first = request(user, binding="browser:" + secrets.token_urlsafe(32))
    second = request(user, binding="browser:" + secrets.token_urlsafe(32))
    assert bridge_http_client_binding(first) != bridge_http_client_binding(second)


def test_desktop_family_requires_verified_guard_identity():
    from buildapp.clock import utc_now

    exp = int(utc_now().timestamp()) + 300
    req = request(
        state={
            "build_user_id": str(uuid4()),
            "build_oauth_client_id": "build-desktop",
            "build_oauth_access_jti": "a" * 32,
            "build_oauth_access_exp": exp,
        }
    )
    assert bridge_http_client_binding(req) == f"desktop:{'a' * 32}:{exp}"
    req.scope["state"]["build_oauth_access_exp"] = exp - 600
    with pytest.raises(NotAuthorizedException):
        bridge_http_client_binding(req)
    with pytest.raises(NotAuthorizedException):
        bridge_http_client_binding(request(state={"build_user_id": str(uuid4())}))
    with pytest.raises(NotAuthorizedException):
        bridge_http_client_binding(request())


def test_connect_policy_changes_only_connect_src_and_preserves_all_other_directives():
    base = "default-src 'self'; script-src 'self' 'nonce-abc'; connect-src 'self' data: wss://relay.example; object-src 'none'"
    policy = connect_policy(base, ["https://bridge.example", "https://bridge.example"])
    assert policy == base.replace(
        "wss://relay.example", "wss://relay.example https://bridge.example"
    )
    assert "connect-src https:" not in policy


def test_connect_policy_inherits_default_and_never_broadens_to_a_scheme():
    policy = connect_policy(
        "default-src 'self'; object-src 'none'", ["https://bridge.example"]
    )
    assert (
        policy
        == "default-src 'self'; object-src 'none'; connect-src 'self' https://bridge.example"
    )


async def seed_endpoint(db, user_id, origin, *, enabled=True, approved=True, epoch=1):
    from buildapp.models import BridgeHttpEndpoint, Device

    device = Device(
        name="bridge",
        owner_user_id=user_id,
        approved=approved,
        identity_public_key_b64="a",
        transport_public_key_b64="b",
        authorization_epoch=1,
    )
    db.add(device)
    await db.flush()
    endpoint = BridgeHttpEndpoint(
        identity_fingerprint="0" * 64,
        device_id=device.id,
        user_id=user_id,
        origin=origin,
        endpoint_epoch="endpoint-1",
        authorization_epoch=epoch,
        enabled=enabled,
    )
    db.add(endpoint)
    await db.flush()
    return endpoint


@pytest.mark.asyncio
async def test_document_query_bounds_endpoints_and_rechecks_current_owner_and_epoch(db):
    from buildapp.bridge_http_policy import document_origins
    from buildapp.clock import utc_now
    from buildapp.db_test_support import add_account

    user = await add_account(db, "policy@example.com")
    other = await add_account(db, "other@example.com")
    for index in range(19):
        await seed_endpoint(db, user, f"https://bridge-{index}.example")
    await seed_endpoint(db, other, "https://other.example")
    await seed_endpoint(db, user, "https://disabled.example", enabled=False)
    await seed_endpoint(db, user, "https://unapproved.example", approved=False)
    await seed_endpoint(db, user, "https://old-epoch.example", epoch=2)
    assert len(await document_origins(db, user, "browser:one", None, utc_now())) == 16
    origins = await document_origins(db, other, "browser:two", None, utc_now())
    assert origins == ["https://other.example"]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "change", ["user", "client", "expired", "consumed", "purpose", "invalid-origin"]
)
async def test_only_live_initiating_account_challenge_widens_the_document(db, change):
    from datetime import timedelta

    from buildapp.bridge_http_policy import document_origins
    from buildapp.clock import utc_now
    from buildapp.db_test_support import add_account
    from buildapp.models import BridgeHttpChallenge

    user = await add_account(db, "policy@example.com")
    other = await add_account(db, "other@example.com")
    now = utc_now()
    challenge = BridgeHttpChallenge(
        user_id=user,
        client_binding="browser:one",
        origin="https://candidate.example",
        purpose="discovery",
        nonce="n",
        nonce_hash="h",
        expires_at=now + timedelta(seconds=30),
    )
    db.add(challenge)
    await db.flush()
    assert await document_origins(db, user, "browser:one", str(challenge.id), now) == [
        "https://candidate.example"
    ]
    if change == "user":
        challenge.user_id = other
    elif change == "client":
        challenge.client_binding = "browser:two"
    elif change == "expired":
        challenge.expires_at = now
    elif change == "consumed":
        challenge.consumed_at = now
    elif change == "purpose":
        challenge.purpose = "registration"
    else:
        challenge.origin = "https://candidate.example; script-src *"
    await db.flush()
    assert await document_origins(db, user, "browser:one", str(challenge.id), now) == []


@pytest.fixture()
def skrift_app(tmp_path, monkeypatch):
    yield from build_login_app(tmp_path, monkeypatch)


def enroll(client, email="policy@example.com"):
    from buildapp.db_test_support import add_member
    from buildapp.skrift_app_test_support import on_database

    user, _ = real_login(client, email)

    async def work(db):
        await add_member(db, user, email)

    on_database(client, work)
    return user


def test_real_skrift_assembly_has_one_account_csp_keeps_nonce_and_nonapp_defaults(
    skrift_app, tmp_path, monkeypatch
):
    import re
    from datetime import timedelta

    from buildapp import controllers
    from buildapp.clock import utc_now
    from buildapp.models import BridgeHttpChallenge
    from buildapp.skrift_app_test_support import on_database

    (tmp_path / "index.html").write_text(
        '<html><div id="app">{{ACCOUNT_ID}}</div></html>'
    )
    monkeypatch.setattr(controllers, "STATIC_DIR", tmp_path)
    with framework_client(skrift_app) as client:
        user = enroll(client)
        client.get(
            "/app/"
        )  # Establish this signed cookie's unpredictable client family.
        binding = client.get_session_data()["build_bridge_http_binding"]

        async def work(db):
            await seed_endpoint(db, user, "https://owned.example")
            from buildapp.db_test_support import add_account

            other = await add_account(db, "other@example.com")
            await seed_endpoint(db, other, "https://other.example")
            challenge = BridgeHttpChallenge(
                user_id=user,
                client_binding=binding,
                origin="https://candidate.example",
                purpose="discovery",
                nonce="n",
                nonce_hash="h",
                expires_at=utc_now() + timedelta(seconds=30),
            )
            db.add(challenge)
            await db.commit()
            return challenge.id

        challenge = on_database(client, work)
        response = client.get(
            f"/app/?bridge_discovery={challenge}&origin=https://untrusted.example"
        )
        assert response.status_code == 200, response.text
        policies = response.headers.get_list("content-security-policy")
        assert len(policies) == 1
        policy = policies[0]
        assert (
            "https://owned.example" in policy and "https://candidate.example" in policy
        )
        assert "other.example" not in policy and "untrusted.example" not in policy
        assert re.search(r"script-src [^;]*'nonce-[^']+'", policy)
        assert (
            "object-src 'none'" in policy
            and "form-action 'self' getbuilding:" in policy
        )
        assert "wss://relay.getbuild.ing" in policy
        assert response.headers["cache-control"] == "private, no-store"
        assert response.headers["referrer-policy"] == "no-referrer"
        assert response.headers["x-content-type-options"] == "nosniff"
        login = client.get("/auth/logout")
        assert len(login.headers.get_list("content-security-policy")) == 1
        assert "owned.example" not in login.headers["content-security-policy"]
        assert "candidate.example" not in login.headers["content-security-policy"]
        client.cookies.clear()
        anonymous = client.get(
            f"/app/?bridge_discovery={challenge}", follow_redirects=False
        )
        assert anonymous.status_code == 302
        assert "candidate.example" not in anonymous.headers["content-security-policy"]


@pytest.mark.asyncio
async def test_stale_epoch_reassigned_and_noncanonical_registered_rows_cannot_widen_csp(
    db,
):
    from buildapp.bridge_http_policy import document_origins
    from buildapp.clock import utc_now
    from buildapp.db_test_support import add_account
    from buildapp.models import Device

    user = await add_account(db, "policy@example.com")
    other = await add_account(db, "other@example.com")
    await seed_endpoint(db, user, "https://stale.example", epoch=2)
    await seed_endpoint(db, user, "https://malicious.example; script-src *")
    endpoint = await seed_endpoint(db, user, "https://previous-owner.example")
    device = await db.get(Device, endpoint.device_id)
    device.owner_user_id = other
    await db.flush()
    assert await document_origins(db, user, "browser:one", None, utc_now()) == []
    assert await document_origins(db, other, "browser:two", None, utc_now()) == []


def test_real_skrift_logout_revokes_only_current_family_after_valid_csrf(
    skrift_app, tmp_path, monkeypatch
):
    import re

    from buildapp import controllers
    from buildapp.models import BridgeHttpFamily
    from buildapp.skrift_app_test_support import on_database

    (tmp_path / "index.html").write_text("<html>Build</html>")
    monkeypatch.setattr(controllers, "STATIC_DIR", tmp_path)
    with framework_client(skrift_app) as client:
        user = enroll(client)
        assert client.get("/app/").status_code == 200
        binding = client.get_session_data()["build_bridge_http_binding"]

        async def work(db):
            endpoint = await seed_endpoint(db, user, "https://owned.example")
            families = [
                BridgeHttpFamily(
                    issuer="https://testserver.local",
                    user_id=user,
                    device_id=endpoint.device_id,
                    client_binding=client_binding,
                    origin=endpoint.origin,
                    endpoint_epoch=endpoint.endpoint_epoch,
                    authorization_epoch=endpoint.authorization_epoch,
                    scope="bridge:read bridge:write",
                )
                for client_binding in (binding, "browser:another")
            ]
            db.add_all(families)
            await db.commit()
            return [family.id for family in families]

        family_ids = on_database(client, work)

        async def revoked(db):
            return [
                (await db.get(BridgeHttpFamily, family_id)).revoked_at is not None
                for family_id in family_ids
            ]

        refused = client.post(
            "/auth/logout", data={"_csrf": "wrong"}, follow_redirects=False
        )
        assert (
            refused.status_code == 302 and refused.headers["location"] == "/auth/logout"
        )
        assert on_database(client, revoked) == [False, False]
        page = client.get("/auth/logout")
        csrf = re.search(r'name="_csrf" value="([^"]+)"', page.text).group(1)
        logged_out = client.post(
            "/auth/logout", data={"_csrf": csrf}, follow_redirects=False
        )
        assert (
            logged_out.status_code == 302 and logged_out.headers["location"] == "/app/"
        )
        assert on_database(client, revoked) == [True, False]
        assert SESSION_USER_ID not in client.get_session_data()


def test_real_skrift_desktop_csp_requires_exact_verified_access_token(
    skrift_app, tmp_path, monkeypatch
):
    from datetime import timedelta

    from skrift.auth.tokens import create_signed_token, verify_signed_token
    from skrift.config import get_settings

    from buildapp import controllers
    from buildapp.clock import utc_now
    from buildapp.models import BridgeHttpChallenge
    from buildapp.skrift_app_test_support import on_database

    (tmp_path / "index.html").write_text("<html>Build desktop</html>")
    monkeypatch.setattr(controllers, "STATIC_DIR", tmp_path)
    with framework_client(skrift_app) as client:
        user = enroll(client)
        payload = {
            "type": "access",
            "user_id": str(user),
            "client_id": "build-desktop",
            "scope": "openid",
        }
        first = create_signed_token(payload, get_settings().secret_key, 300)
        second = create_signed_token(payload, get_settings().secret_key, 300)
        claims = verify_signed_token(first, get_settings().secret_key)
        binding = f"desktop:{claims['jti']}:{claims['exp']}"
        client.cookies.clear()

        async def work(db):
            challenge = BridgeHttpChallenge(
                user_id=user,
                client_binding=binding,
                origin="https://desktop-candidate.example",
                purpose="discovery",
                nonce="n",
                nonce_hash="h",
                expires_at=utc_now() + timedelta(seconds=30),
            )
            db.add(challenge)
            await db.commit()
            return challenge.id

        challenge_id = on_database(client, work)
        path = f"/app/desktop?bridge_discovery={challenge_id}"
        allowed = client.get(path, headers={"Authorization": f"Bearer {first}"})
        assert allowed.status_code == 200
        assert (
            "https://desktop-candidate.example"
            in allowed.headers["content-security-policy"]
        )
        other_token = client.get(path, headers={"Authorization": f"Bearer {second}"})
        assert other_token.status_code == 200
        assert (
            "desktop-candidate.example"
            not in other_token.headers["content-security-policy"]
        )
        unverified = create_signed_token(
            payload, "wrong-signing-secret-for-attack", 300
        )
        refused = client.get(path, headers={"Authorization": f"Bearer {unverified}"})
        assert refused.status_code == 401
        assert (
            "desktop-candidate.example"
            not in refused.headers["content-security-policy"]
        )
