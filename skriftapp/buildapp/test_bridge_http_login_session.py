"""Replay the earliest cookie issued by real primary login, before bridge use."""

from __future__ import annotations

from contextlib import contextmanager, ExitStack

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


@pytest.fixture()
def completed_login_requests():
    from skrift.hooks import hooks

    captured = []

    def observe(login_result, request):
        captured.append(request)

    hooks.add_action("after_login", observe, priority=0)
    try:
        yield captured
    finally:
        hooks.remove_action("after_login", observe)


def completion_request(template, handler, db):
    """A trusted resolved completion scope with its actual request unit of work."""
    from advanced_alchemy.extensions.litestar import SQLAlchemyPlugin
    from advanced_alchemy.extensions.litestar._utils import set_aa_scope_state
    from litestar import Request

    scope = {
        key: value.copy() if isinstance(value, dict) else value
        for key, value in template.scope.items()
    }
    scope["route_handler"] = handler
    config = template.app.plugins.get(SQLAlchemyPlugin).config[0]
    set_aa_scope_state(scope, config.session_scope_key, db)
    return Request(scope)


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
    result = client.post(
        f"/api/devices/{device.id}/http-endpoint/challenge",
        json={"proof": signed(key, device, "registration-challenge")},
    )
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


def test_real_earliest_login_cookie_cannot_remint_after_csrf_logout(
    login_app, tmp_path, monkeypatch
):
    from buildapp import controllers

    (tmp_path / "index.html").write_text("<html>Cached Build shell</html>")
    monkeypatch.setattr(controllers, "STATIC_DIR", tmp_path)
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
        shell = client.get("/app/")
        assert shell.status_code == 200
        assert ORIGIN not in shell.headers["content-security-policy"]
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


def test_failed_login_identity_persistence_returns_no_authenticated_cookie(login_app):
    from sqlalchemy import event
    from sqlalchemy.orm import Session
    from buildapp.models import BridgeHttpClient

    def fail_client_commit(db):
        if any(isinstance(row, BridgeHttpClient) for row in db.new):
            raise RuntimeError("isolated pending bridge identity commit failure")

    event.listen(Session, "before_commit", fail_client_commit)
    try:
        with framework_client(login_app, raise_server_exceptions=False) as client:
            page = client.get("/auth/dummy/login", follow_redirects=False)
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
                from skrift.db.models.user import User

                assert (
                    await db.scalar(
                        select(User.id).where(User.email == "failed-login@example.com")
                    )
                    is not None
                )
                return await db.scalar(
                    select(func.count()).select_from(BridgeHttpClient)
                )

            assert framework.on_database(client, identities) == 0
    finally:
        event.remove(Session, "before_commit", fail_client_commit)


def test_framework_deferred_auth_completion_establishes_a_new_persisted_identity(
    login_app, completed_login_requests
):
    """Exercise the shared promotion seam after a factor has already verified.

    External-factor verification is not simulated by this test: the account was
    authenticated through real primary login, and this case checks the separate
    framework pending-auth promotion boundary calls the same durable login hook.
    """
    from buildapp.models import BridgeHttpClient
    from buildapp.token_hash import token_hash
    from skrift.auth.session_service import PendingAuthState
    from skrift.config import get_settings
    from skrift.controllers.auth import _complete_pending_login
    from skrift.db.models.user import User

    with framework_client(login_app) as client:
        user_id, _ = real_login(client)
        request = completed_login_requests[-1]
        initial_binding = request.session[BRIDGE_BINDING_KEY]

        async def promote(db):
            user = await db.get(User, user_id)
            pending = PendingAuthState(
                pending_auth_id=str(uuid4()),
                method_key="passkey",
                method_type="passkey",
                stage="verify",
                user_id=str(user_id),
            )
            result = await _complete_pending_login(
                request, get_settings(), user, pending
            )
            assert result.status_code == 302
            new_binding = request.session[BRIDGE_BINDING_KEY]
            assert new_binding != initial_binding
            row = await db.scalar(
                select(BridgeHttpClient).where(
                    BridgeHttpClient.user_id == user_id,
                    BridgeHttpClient.binding_hash == token_hash(new_binding),
                )
            )
            assert row is not None and row.revoked_at is None

        framework.on_database(client, promote)


def test_framework_signup_completion_commits_new_account_passkey_and_bridge_identity_together(
    login_app, completed_login_requests
):
    """Use Skrift's post-verification signup/completion services on a new user.

    The credential record is a fixture representing already verified output.
    This is a transaction/completion regression, not passkey cryptography proof.
    """
    from types import SimpleNamespace
    from buildapp.auth_controller import BuildAuthController
    from buildapp.desktop_auth import auth_session_maker
    from buildapp.models import BridgeHttpClient
    from buildapp.token_hash import token_hash
    from skrift.auth.identities import ResolvedPrimaryIdentity
    from skrift.config import get_settings
    from skrift.controllers.auth import (
        _create_primary_passkey_signup_login,
        _finalize_primary_login,
    )
    from skrift.db.models.second_factor import SecondFactorEnrollment
    from skrift.db.models.user import User
    from sqlalchemy.ext.asyncio import async_object_session

    with framework_client(login_app) as client:
        real_login(client)
        template = completed_login_requests[-1]
        credential = SimpleNamespace(
            credential_id="verified-signup-fixture",
            public_key="fixture-public-key",
            sign_count=0,
            transports=[],
            enrollment_metadata={},
        )

        async def complete_signup(db):
            request = completion_request(
                template, BuildAuthController.complete_primary_method_registration, db
            )
            result = await _create_primary_passkey_signup_login(
                db,
                method_key="passkey",
                factor_key="passkey",
                email="new-uncommitted-signup@example.com",
                name="New signup",
                registration_result=credential,
            )
            user_id = result.user.id
            assert async_object_session(result.user) is db
            async with auth_session_maker(request)() as observer:
                assert await observer.get(User, user_id) is None
            identity = ResolvedPrimaryIdentity(
                method_key="passkey",
                method_type="passkey",
                subject_id=credential.credential_id,
                email=result.user.email,
                name=result.user.name,
                picture_url=None,
            )
            response = await _finalize_primary_login(
                request, db, get_settings(), result, identity=identity
            )
            assert response.status_code == 302
            binding = request.session[BRIDGE_BINDING_KEY]
            assert request.session[BRIDGE_ACCOUNT_KEY] == str(user_id)
            async with auth_session_maker(request)() as observer:
                assert await observer.get(User, user_id) is None
                assert (
                    await observer.scalar(
                        select(SecondFactorEnrollment).where(
                            SecondFactorEnrollment.user_id == user_id
                        )
                    )
                    is None
                )
                assert (
                    await observer.scalar(
                        select(BridgeHttpClient).where(
                            BridgeHttpClient.user_id == user_id
                        )
                    )
                    is None
                )
            # This is the framework's normal boundary after all completion hooks.
            await db.commit()
            async with auth_session_maker(request)() as observer:
                assert await observer.get(User, user_id) is not None
                assert (
                    await observer.scalar(
                        select(SecondFactorEnrollment).where(
                            SecondFactorEnrollment.user_id == user_id
                        )
                    )
                    is not None
                )
                saved = await observer.scalar(
                    select(BridgeHttpClient).where(
                        BridgeHttpClient.user_id == user_id,
                        BridgeHttpClient.binding_hash == token_hash(binding),
                    )
                )
                assert saved is not None and saved.revoked_at is None

        framework.on_database(client, complete_signup)


async def new_verified_signup(db):
    """Database output of verification, without exercising external authenticator crypto."""
    from types import SimpleNamespace
    from skrift.controllers.auth import _create_primary_passkey_signup_login

    credential = SimpleNamespace(
        credential_id="ordinary-verified-credential",
        public_key="fixture-public-key",
        sign_count=1,
        transports=[],
        enrollment_metadata={},
    )
    return await _create_primary_passkey_signup_login(
        db,
        method_key="passkey",
        factor_key="passkey",
        email="ordinary-signup@example.com",
        name="Ordinary signup",
        registration_result=credential,
    )


async def finalize_verified_login(request, db, login_result):
    from skrift.auth.identities import ResolvedPrimaryIdentity
    from skrift.config import get_settings
    from skrift.controllers.auth import _finalize_primary_login

    identity = ResolvedPrimaryIdentity(
        method_key=login_result.method_key,
        method_type=login_result.method_type,
        subject_id="ordinary-verified-credential",
        email=login_result.user.email,
        name=login_result.user.name,
        picture_url=None,
    )
    return await _finalize_primary_login(
        request, db, get_settings(), login_result, identity=identity
    )


async def visible_identity(request, user_id, binding):
    from buildapp.desktop_auth import auth_session_maker
    from buildapp.models import BridgeHttpClient
    from buildapp.token_hash import token_hash

    async with auth_session_maker(request)() as observer:
        return await observer.scalar(
            select(BridgeHttpClient.id).where(
                BridgeHttpClient.user_id == user_id,
                BridgeHttpClient.binding_hash == token_hash(binding),
            )
        )


def test_existing_passkey_counter_and_identity_remain_private_until_normal_commit(
    login_app, completed_login_requests
):
    from buildapp.auth_controller import BuildAuthController
    from skrift.auth.oauth_account_service import (
        find_login_result_for_passkey_credential,
    )
    from skrift.db.models.second_factor import SecondFactorEnrollment
    from buildapp.desktop_auth import auth_session_maker

    with framework_client(login_app) as client:
        real_login(client)
        template = completed_login_requests[-1]

        async def seed(db):
            result = await new_verified_signup(db)
            await db.commit()
            return result.user.id

        user_id = framework.on_database(client, seed)

        async def complete(db):
            result = await find_login_result_for_passkey_credential(
                db,
                factor_key="passkey",
                method_key="passkey",
                credential_id="ordinary-verified-credential",
            )
            credential = await db.scalar(
                select(SecondFactorEnrollment).where(
                    SecondFactorEnrollment.user_id == user_id
                )
            )
            credential.sign_count = 2
            request = completion_request(
                template, BuildAuthController.complete_primary_method_auth, db
            )
            await finalize_verified_login(request, db, result)
            binding = request.session[BRIDGE_BINDING_KEY]
            assert db.in_transaction()
            assert await visible_identity(request, user_id, binding) is None
            async with auth_session_maker(request)() as observer:
                assert (
                    await observer.scalar(
                        select(SecondFactorEnrollment.sign_count).where(
                            SecondFactorEnrollment.user_id == user_id
                        )
                    )
                    == 1
                )
            await db.commit()
            assert await visible_identity(request, user_id, binding) is not None
            async with auth_session_maker(request)() as observer:
                assert (
                    await observer.scalar(
                        select(SecondFactorEnrollment.sign_count).where(
                            SecondFactorEnrollment.user_id == user_id
                        )
                    )
                    == 2
                )

        framework.on_database(client, complete)


def reject_identity_flush(session, context, instances):
    from buildapp.models import BridgeHttpClient

    if any(isinstance(row, BridgeHttpClient) for row in session.new):
        raise RuntimeError("ordinary flush failure")


def reject_later_hook(*args):
    raise RuntimeError("ordinary later hook failure")


def reject_caller_commit(session):
    raise RuntimeError("ordinary commit failure after flush")


@contextmanager
def failing_completion_phase(db, phase):
    from sqlalchemy import event
    from skrift.hooks import hooks

    with ExitStack() as cleanup:
        if phase == "flush":
            event.listen(db.sync_session, "before_flush", reject_identity_flush)
            cleanup.callback(
                event.remove, db.sync_session, "before_flush", reject_identity_flush
            )
        elif phase in ("after_user_created", "login_redirect"):
            register = (
                hooks.add_filter if phase == "login_redirect" else hooks.add_action
            )
            remove = (
                hooks.remove_filter
                if phase == "login_redirect"
                else hooks.remove_action
            )
            register(phase, reject_later_hook, priority=99)
            cleanup.callback(remove, phase, reject_later_hook)
        yield


@contextmanager
def failing_caller_commit(db, phase):
    from sqlalchemy import event

    if phase == "caller_commit":
        event.listen(db.sync_session, "before_commit", reject_caller_commit)
    try:
        yield
    finally:
        if phase == "caller_commit":
            event.remove(db.sync_session, "before_commit", reject_caller_commit)


@pytest.mark.parametrize(
    "failure_stage", ["flush", "caller_commit", "after_user_created", "login_redirect"]
)
def test_borrowed_signup_failure_leaves_rollback_to_caller_and_no_authentication(
    login_app, completed_login_requests, failure_stage
):
    from skrift.db.models.user import User
    from buildapp.auth_controller import BuildAuthController
    from buildapp.bridge_http_login_guard import BrowserLoginGuard
    from buildapp.desktop_auth import auth_session_maker
    from buildapp.models import BridgeHttpClient

    with framework_client(login_app) as client:
        real_login(client)
        template = completed_login_requests[-1]

        async def complete(db):
            result = await new_verified_signup(db)
            user_id = result.user.id
            request = completion_request(
                template, BuildAuthController.complete_primary_method_registration, db
            )

            async def inner(scope, receive, send):
                await finalize_verified_login(request, db, result)
                # A real query proves INSERT/flush happened before the commit fault.
                identity = await db.scalar(
                    select(BridgeHttpClient).where(BridgeHttpClient.user_id == user_id)
                )
                assert identity is not None and identity not in db.new
                with failing_caller_commit(db, failure_stage):
                    await db.commit()

            async def receive():
                return {"type": "http.request", "body": b"", "more_body": False}

            async def send(message):
                raise AssertionError("failure must precede authenticated response")

            messages = {
                "flush": "ordinary flush",
                "caller_commit": "ordinary commit",
                "after_user_created": "ordinary later",
                "login_redirect": "ordinary later",
            }
            with failing_completion_phase(db, failure_stage):
                with pytest.raises(RuntimeError, match=messages[failure_stage]):
                    await BrowserLoginGuard(inner)(request.scope, receive, send)
            assert request.session == {}
            assert db.in_transaction(), (
                "hook and guard must not end the borrowed unit of work"
            )
            async with auth_session_maker(request)() as observer:
                assert await observer.get(User, user_id) is None
            await db.rollback()  # The caller owns this boundary on failure.
            async with auth_session_maker(request)() as observer:
                assert await observer.get(User, user_id) is None
                assert (
                    await observer.scalar(
                        select(BridgeHttpClient).where(
                            BridgeHttpClient.user_id == user_id
                        )
                    )
                    is None
                )

        framework.on_database(client, complete)


@pytest.mark.parametrize(
    "problem",
    [
        "unknown_handler",
        "no_guard",
        "missing_session",
        "mismatched_session",
        "detached_user",
    ],
)
def test_unclassified_or_mismatched_borrowed_completion_fails_closed(
    login_app, completed_login_requests, problem
):
    from types import SimpleNamespace
    from advanced_alchemy.extensions.litestar import SQLAlchemyPlugin
    from advanced_alchemy.extensions.litestar._utils import set_aa_scope_state
    from litestar.exceptions import NotAuthorizedException
    from skrift.auth.oauth_account_service import LoginResult
    from skrift.db.models.user import User
    from buildapp.auth_controller import BuildAuthController
    from buildapp.bridge_http_login import establish_browser_login, GUARD_KEY
    from buildapp.desktop_auth import auth_session_maker

    with framework_client(login_app) as client:
        user_id, _ = real_login(client)
        template = completed_login_requests[-1]

        async def complete(db):
            user = await db.get(User, user_id)
            request = completion_request(
                template, BuildAuthController.complete_primary_method_registration, db
            )
            config = request.app.plugins.get(SQLAlchemyPlugin).config[0]
            async with auth_session_maker(request)() as other:
                if problem == "unknown_handler":
                    request.scope["route_handler"] = SimpleNamespace(fn=lambda: None)
                if problem == "no_guard":
                    request.scope.pop(GUARD_KEY, None)
                if problem == "missing_session":
                    set_aa_scope_state(request.scope, config.session_scope_key, None)
                if problem == "mismatched_session":
                    set_aa_scope_state(request.scope, config.session_scope_key, other)
                if problem == "detached_user":
                    db.expunge(user)
                with pytest.raises(NotAuthorizedException):
                    await establish_browser_login(
                        LoginResult(user, None, False, "passkey", "passkey"), request
                    )
            assert request.session == {}

        framework.on_database(client, complete)
        assert family_count(client) == 0


@pytest.mark.parametrize("detach", [False, True])
def test_known_postcommit_identity_does_not_commit_unrelated_caller_work(
    login_app, completed_login_requests, detach
):
    from skrift.auth.oauth_account_service import LoginResult
    from skrift.db.models.user import User
    from buildapp.auth_controller import BuildAuthController
    from buildapp.bridge_http_login import establish_browser_login
    from buildapp.desktop_auth import auth_session_maker

    with framework_client(login_app) as client:
        user_id, _ = real_login(client)
        template = completed_login_requests[-1]

        async def complete(db):
            user = await db.get(User, user_id)
            original = user.name
            if detach:
                db.expunge(user)
            else:
                user.name = "unrelated uncommitted caller change"
            request = completion_request(
                template, BuildAuthController.dummy_login_submit, db
            )
            await establish_browser_login(
                LoginResult(user, None, False, "dummy", "dummy"), request
            )
            assert (
                await visible_identity(
                    request, user_id, request.session[BRIDGE_BINDING_KEY]
                )
                is not None
            )
            async with auth_session_maker(request)() as observer:
                assert (
                    await observer.scalar(select(User.name).where(User.id == user_id))
                    == original
                )
            await db.rollback()

        framework.on_database(client, complete)


def test_pending_factor_transition_mints_no_identity_until_final_promotion(
    login_app, completed_login_requests
):
    from skrift.auth.oauth_account_service import LoginResult
    from skrift.auth.session_service import (
        PendingAuthTransitionDecision,
        get_pending_authentication,
    )
    from skrift.config import get_settings
    from skrift.controllers.auth import _complete_pending_login
    from skrift.db.models.user import User
    from skrift.hooks import hooks
    from buildapp.auth_controller import BuildAuthController
    from buildapp.models import BridgeHttpClient

    with framework_client(login_app) as client:
        user_id, _ = real_login(client)
        template = completed_login_requests[-1]

        async def complete(db):
            user = await db.get(User, user_id)
            request = completion_request(
                template, BuildAuthController.complete_second_factor_verification, db
            )
            request.session.clear()

            def require_factor(*args):
                return PendingAuthTransitionDecision(
                    promote_immediately=False,
                    next_url="/auth/verify",
                    stage="second_factor_required",
                )

            hooks.add_filter("auth_pending_authentication", require_factor, priority=99)
            try:
                response = await finalize_verified_login(
                    request, db, LoginResult(user, None, False, "dummy", "dummy")
                )
            finally:
                hooks.remove_filter("auth_pending_authentication", require_factor)
            assert response.url == "/auth/verify"
            assert (
                SESSION_USER_ID not in request.session
                and BRIDGE_BINDING_KEY not in request.session
            )
            assert (
                await db.scalar(select(func.count()).select_from(BridgeHttpClient)) == 1
            )
            pending = get_pending_authentication(request)
            assert pending is not None
            await _complete_pending_login(request, get_settings(), user, pending)
            assert request.session[SESSION_USER_ID] == str(user_id)
            assert (
                await visible_identity(
                    request, user_id, request.session[BRIDGE_BINDING_KEY]
                )
                is not None
            )

        framework.on_database(client, complete)
