"""Response-lifetime failure fencing through ASGI and the configured cookie stack."""

import asyncio
from types import SimpleNamespace
from uuid import uuid4

import pytest
from litestar.response import Redirect
from skrift.auth.session_keys import SESSION_USER_ID
from skrift.hooks import hooks
from sqlalchemy import func, select

from buildapp.auth_controller import BuildAuthController
from buildapp.bridge_http_test_support import build_login_app, framework_client, csrf
from buildapp.desktop_auth import BRIDGE_BINDING_KEY
from buildapp.models import BridgeHttpClient
from buildapp.skrift_app_test_support import on_database


def completion_scope(handler=None):
    return {
        "type": "http",
        "route_handler": handler or BuildAuthController.dummy_login_submit,
        "session": {
            SESSION_USER_ID: str(uuid4()),
            BRIDGE_BINDING_KEY: "existing-test-identity",
        },
    }


async def receive():
    return {"type": "http.request", "body": b"", "more_body": False}


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "status,marked", [(400, False), (401, False), (500, False), (302, True)]
)
async def test_guard_clears_authentication_before_error_or_marked_response_start(
    status, marked
):
    from buildapp.bridge_http_login import FAILURE_KEY, GUARD_KEY
    from buildapp.bridge_http_login_guard import BrowserLoginGuard

    scope = completion_scope()
    observed = []

    async def inner(scope, receive, send):
        assert scope[GUARD_KEY] is True
        if marked:
            scope[FAILURE_KEY] = True
        await send({"type": "http.response.start", "status": status, "headers": []})

    async def send(message):
        observed.append(dict(scope["session"]))

    await BrowserLoginGuard(inner)(scope, receive, send)
    assert observed == [{}]
    assert scope[FAILURE_KEY] is True


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", [RuntimeError, asyncio.CancelledError])
async def test_guard_clears_then_reraises_escaping_failure_or_cancellation(failure):
    from buildapp.bridge_http_login import FAILURE_KEY
    from buildapp.bridge_http_login_guard import BrowserLoginGuard

    scope = completion_scope()

    async def inner(scope, receive, send):
        raise failure("ordinary completion test")

    async def send(message):
        raise AssertionError("unexpected response")

    with pytest.raises(failure):
        await BrowserLoginGuard(inner)(scope, receive, send)
    assert scope["session"] == {} and scope[FAILURE_KEY] is True


@pytest.mark.asyncio
async def test_guard_sees_exception_converted_to_success_shaped_redirect():
    from buildapp.bridge_http_login import FAILURE_KEY
    from buildapp.bridge_http_login_guard import BrowserLoginGuard

    scope = completion_scope()
    observed = []

    async def inner(scope, receive, send):
        try:
            raise RuntimeError("ordinary converted exception")
        except RuntimeError:
            await send({"type": "http.response.start", "status": 302, "headers": []})

    async def send(message):
        observed.append(dict(scope["session"]))

    await BrowserLoginGuard(inner)(scope, receive, send)
    assert observed == [{}] and scope[FAILURE_KEY] is True


@pytest.mark.asyncio
async def test_guard_preserves_success_and_unrelated_existing_sessions():
    from buildapp.bridge_http_login_guard import BrowserLoginGuard

    for scope, status in (
        (completion_scope(), 200),
        (completion_scope(SimpleNamespace(fn=lambda: None)), 500),
    ):
        original = dict(scope["session"])

        async def inner(scope, receive, send):
            await send({"type": "http.response.start", "status": status, "headers": []})

        async def send(message):
            assert scope["session"] == original

        await BrowserLoginGuard(inner)(scope, receive, send)


@pytest.mark.parametrize(
    "name,expected",
    [
        ("complete_primary_method_registration", "borrowed"),
        ("complete_primary_method_auth", "borrowed"),
        ("oauth_callback", "owned"),
        ("complete_second_factor_verification", "owned"),
        ("verify_email_claim", "owned"),
        ("dummy_login_submit", "owned"),
    ],
)
def test_only_pinned_resolved_completion_callables_select_transaction_owner(
    name, expected
):
    from buildapp.bridge_http_login import completion_owner

    assert (
        completion_owner(completion_scope(getattr(BuildAuthController, name)))
        == expected
    )
    assert (
        completion_owner(
            {
                "type": "http",
                "path": "/auth/passkey/register/complete",
                "route_handler": SimpleNamespace(fn=lambda: None),
            }
        )
        is None
    )


def test_dependency_inventory_mismatch_cannot_select_transaction_owner(monkeypatch):
    from buildapp import bridge_http_login

    monkeypatch.setattr(bridge_http_login, "completion_runtime_matches", lambda: False)
    assert bridge_http_login.completion_owner(completion_scope()) is None


@pytest.fixture()
def error_completion_app(tmp_path, monkeypatch, response_status):
    if response_status == 302:
        monkeypatch.setattr(
            BuildAuthController.dummy_login_submit,
            "exception_handlers",
            {
                RuntimeError: lambda request, error: Redirect(
                    path="/ordinary-error", status_code=302
                )
            },
        )
    yield from build_login_app(tmp_path, monkeypatch)


@pytest.mark.parametrize("response_status", [500, 302])
@pytest.mark.parametrize("failure_hook", ["after_user_created", "login_redirect"])
def test_configured_guard_clears_real_cookie_after_later_completion_hook_failure(
    error_completion_app, response_status, failure_hook
):
    reached = []

    def later_failure(*args):
        request = args[-1]
        reached.append(dict(request.session))
        assert (
            SESSION_USER_ID in request.session and BRIDGE_BINDING_KEY in request.session
        )
        raise RuntimeError("ordinary later completion failure")

    register = (
        hooks.add_filter if failure_hook == "login_redirect" else hooks.add_action
    )
    remove = (
        hooks.remove_filter if failure_hook == "login_redirect" else hooks.remove_action
    )
    register(failure_hook, later_failure, priority=99)
    try:
        with framework_client(
            error_completion_app, raise_server_exceptions=False
        ) as client:
            page = client.get("/auth/dummy/login", follow_redirects=False)
            result = client.post(
                "/auth/dummy-login",
                data={
                    "_csrf": csrf(page),
                    "email": "late-hook@example.com",
                    "name": "Late hook",
                },
                follow_redirects=False,
            )
            assert reached, "failure must occur after the durable identity hook"
            assert result.status_code == response_status, result.text
            session = client.get_session_data()
            assert SESSION_USER_ID not in session and BRIDGE_BINDING_KEY not in session

            async def saved_identity(db):
                return await db.scalar(
                    select(func.count()).select_from(BridgeHttpClient)
                )

            # Post-commit paths may retain an unused identity; no cookie carries it.
            assert on_database(client, saved_identity) == 1
    finally:
        remove(failure_hook, later_failure)
