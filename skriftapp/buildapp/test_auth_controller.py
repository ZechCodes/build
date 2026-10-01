"""``BuildAuthController`` is Skrift's ``AuthController`` with four handlers replaced by
name (#314). Skrift is a pinned alpha: these tests pin the route inventory and the
config that loads the controller, so a Skrift upgrade that renames a handler (leaving
Skrift's unguarded one in place) or adds a route, which could create accounts, fails
here before it ships."""

from __future__ import annotations

import pytest
import yaml
from litestar import Litestar
from skrift.controllers.auth import AuthController

from buildapp.auth_controller import SIGNIN_VIEW, SIGNUP_VIEW, BuildAuthController, page_view
from buildapp.skrift_app_test_support import SKRIFTAPP_DIR

AUTH_PREFIX = "/auth"
ROUTED_METHODS = {"GET", "POST"}
OURS = "buildapp.auth_controller:BuildAuthController"
SKRIFTS = "skrift.controllers.auth:AuthController"
CONFIGS = ["app.yaml", "app.dev.yaml", "app.mail.yaml"]

#: Every route Skrift 0.2.0a10's AuthController serves. A new one is a decision: can it
#: create an account, and does it need the invite check?
AUTH_ROUTES = {
    ("GET", "/auth/login"),
    ("GET", "/auth/logout"),
    ("GET", "/auth/passkeys"),
    ("GET", "/auth/verify"),
    ("GET", "/auth/verify-email/claim/{token:str}"),
    ("GET", "/auth/verify-email/pending"),
    ("GET", "/auth/verify/{factor_key:str}"),
    ("GET", "/auth/{provider:str}/callback"),
    ("GET", "/auth/{provider:str}/login"),
    ("POST", "/auth/dummy-login"),
    ("POST", "/auth/logout"),
    ("POST", "/auth/passkeys/complete"),
    ("POST", "/auth/passkeys/options"),
    ("POST", "/auth/passkeys/{enrollment_id:uuid}/delete"),
    ("POST", "/auth/verify/{factor_key:str}/complete"),
    ("POST", "/auth/verify/{factor_key:str}/options"),
    ("POST", "/auth/{provider:str}/complete"),
    ("POST", "/auth/{provider:str}/options"),
    ("POST", "/auth/{provider:str}/register/complete"),
    ("POST", "/auth/{provider:str}/register/options"),
}
#: The routes Build answers itself before handing on to Skrift's handler.
REPLACED = {
    ("GET", "/auth/login"),
    ("GET", "/auth/{provider:str}/login"),
    ("POST", "/auth/{provider:str}/register/complete"),
    ("POST", "/auth/{provider:str}/register/options"),
}


def route_owners(controller) -> dict[tuple[str, str], str]:
    """(method, path) → the name of the class whose function answers it, for every GET and POST
    under /auth."""
    owners = {}
    for route in Litestar([controller]).routes:
        if not route.path.startswith(AUTH_PREFIX):
            continue
        for handler in route.route_handlers:
            for method in handler.http_methods & ROUTED_METHODS:
                owner_name = handler.fn.__qualname__.split(".")[0]
                owners[(method, route.path)] = owner_name
    return owners


def test_skrifts_auth_routes_are_the_ones_pinned():
    assert set(route_owners(AuthController)) == AUTH_ROUTES


def test_build_serves_exactly_skrifts_auth_routes():
    assert set(route_owners(BuildAuthController)) == AUTH_ROUTES


def test_the_replaced_handlers_are_builds_and_the_rest_skrifts():
    owners = route_owners(BuildAuthController)
    assert {route for route, owner in owners.items() if owner == BuildAuthController.__name__} == REPLACED
    assert {route for route, owner in owners.items() if owner == AuthController.__name__} == AUTH_ROUTES - REPLACED


@pytest.mark.parametrize("config", CONFIGS)
def test_every_config_loads_builds_auth_controller_and_not_skrifts(config):
    controllers = (SKRIFTAPP_DIR / config).read_text()
    assert f"  - {OURS}\n" in controllers
    assert SKRIFTS not in controllers


def test_production_signs_in_with_passkeys_alone():
    """The invite check guards passkey registration only. Skrift's dummy and OAuth methods
    create accounts on their own routes, so adding either to production would open
    uninvited signup."""
    methods = yaml.safe_load((SKRIFTAPP_DIR / "app.yaml").read_text())["auth"]["methods"]
    assert {key: method["type"] for key, method in methods.items()} == {"passkey": "passkey"}


@pytest.mark.parametrize("requested", [None, "signup", "signin", "SIGNUP", "x"])
def test_without_an_invite_every_requested_view_is_sign_in(requested):
    assert page_view(None, requested) == SIGNIN_VIEW


@pytest.mark.parametrize(("requested", "view"), [(None, SIGNUP_VIEW), ("signin", SIGNIN_VIEW), ("x", SIGNUP_VIEW)])
def test_an_invite_visitor_gets_signup_unless_they_ask_for_sign_in(requested, view):
    assert page_view(object(), requested) == view
