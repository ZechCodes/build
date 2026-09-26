"""Tests pinning the production config (app.yaml) against the deployment contract:
passkey-only auth, rate limiting on, env-driven Postgres URL, same-origin CSP with
only the relay websocket as an external connect target — and that dev conveniences
(dummy auth, localhost internal fallback, esm.sh/Google-fonts CSP) stay in
app.dev.yaml only."""

from __future__ import annotations

from pathlib import Path
from uuid import UUID

import pytest
import yaml
from skrift.config import RateLimitConfig
from skrift.ratelimit import RateLimiter

from buildapp.devices_controller import HEARTBEAT_ROUTE_PATH
from buildapp.invites import INVITE_PATH_PREFIX
from buildapp.push_controller import NOTIFY_ROUTE_PATH
from buildapp.rtc_controller import ICE_SERVERS_ROUTE_PATH
from buildapp.transport_controller import REPORT_ROUTE_PATH
from buildapp.waitlist_admin import SEND_PATH, WAITLIST_ADMIN_PATH
from buildapp.waitlist_controller import JOIN_ROUTE_PATH

SKRIFTAPP_DIR = Path(__file__).resolve().parent.parent
PRODUCTION_BASE_URL = "https://getbuild.ing"
JOIN_RATE_LIMIT_WINDOWS = [(3, 60.0), (100, 86400.0)]
INVITE_OPEN_RATE_LIMIT_WINDOWS = [(30, 60.0)]
LANDING_RATE_LIMIT_WINDOWS = [(2000, 60.0)]
# Sized for about 50 people behind one NAT (#173); the arithmetic is in app.yaml.
PAGE_RATE_LIMIT_WINDOWS = [(300, 60.0)]
APP_STATIC_RATE_LIMIT_WINDOWS = [(1200, 60.0)]
SPA_API_RATE_LIMIT_WINDOWS = [(600, 60.0)]
WAITLIST_INVITE_SEND_RATE_LIMIT_WINDOWS = [(20, 60.0), (300, 86400.0)]
# A bridge beats twice a minute, and a beat refused in passing is tried again
# after about 5, 10 and 20 s (bridge/src/presence.rs): fifty bridges behind one
# address all redialing through an api deploy (#173).
BRIDGE_BEAT_RATE_LIMIT_WINDOWS = [(300, 60.0)]
BRIDGE_REPORT_RATE_LIMIT_WINDOWS = [(600, 60.0)]
BRIDGE_NOTIFY_RATE_LIMIT_WINDOWS = [(300, 60.0)]
INVITE_CONTROLLERS = (
    "buildapp.invites_controller:InvitesController",
    "buildapp.invites_admin:InvitesAdminController",
    "buildapp.waitlist_admin:WaitlistAdminController",
)
SAMPLE_INVITE_PATH = f"{INVITE_PATH_PREFIX}inv_a-token"
SAMPLE_WAITLIST_SEND_PATH = SEND_PATH.format(signup_id=UUID(int=1))


def load_config(config_name: str) -> dict:
    return yaml.safe_load((SKRIFTAPP_DIR / config_name).read_text())


def _csp_directives(config: dict) -> dict[str, list[str]]:
    csp = config["security_headers"]["content_security_policy"]
    directives: dict[str, list[str]] = {}
    for clause in csp.split(";"):
        parts = clause.strip().split()
        if parts:
            directives[parts[0]] = parts[1:]
    return directives


def test_production_auth_is_passkey_only():
    methods = load_config("app.yaml")["auth"]["methods"]
    method_types = {m["type"] for m in methods.values()}
    assert method_types == {"passkey"}


def test_production_enables_oauth_for_the_public_desktop_client():
    config = load_config("app.yaml")
    assert config["oauth2_enabled"] is True


def test_dummy_auth_stays_dev_only():
    dev_types = {m["type"] for m in load_config("app.dev.yaml")["auth"]["methods"].values()}
    assert "dummy" in dev_types


def test_production_redirect_base_url_pins_passkey_origin():
    assert load_config("app.yaml")["auth"]["redirect_base_url"] == PRODUCTION_BASE_URL


def test_production_rate_limiting_enabled():
    assert load_config("app.yaml")["rate_limit"]["enabled"] is True


@pytest.mark.asyncio
async def test_landing_assets_have_an_independent_bounded_browser_budget():
    rate_limit = RateLimitConfig(**load_config("app.yaml")["rate_limit"])
    landing_policy = rate_limit.resolve("/landing/assets/devices/tablet.glb", "GET")
    root_policy = rate_limit.resolve("/", "GET")
    auth_policy = rate_limit.resolve("/auth/login", "GET")
    limiter = RateLimiter(redis_client=None)
    caller = "203.0.113.8"

    assert landing_policy.key == "ip"
    assert landing_policy.limits == LANDING_RATE_LIMIT_WINDOWS
    assert landing_policy.name not in {root_policy.name, auth_policy.name}

    for _ in range(2000):
        assert (await limiter.check(landing_policy.name, caller, landing_policy.limits)).allowed
    assert not (await limiter.check(landing_policy.name, caller, landing_policy.limits)).allowed

    assert root_policy.limits == PAGE_RATE_LIMIT_WINDOWS
    for _ in range(300):
        assert (await limiter.check(root_policy.name, caller, root_policy.limits)).allowed
    assert not (await limiter.check(root_policy.name, caller, root_policy.limits)).allowed

    for _ in range(10):
        assert (await limiter.check(auth_policy.name, caller, auth_policy.limits)).allowed
    assert not (await limiter.check(auth_policy.name, caller, auth_policy.limits)).allowed


@pytest.mark.parametrize(
    ("path", "method", "expected_policy"),
    (
        ("/landing/generated/waitlist-boot.js", "GET", "landing_assets"),
        ("/landing/generated/waitlist-boot.js", "POST", "default"),
        ("/landing", "GET", "default"),
        ("/docs", "GET", "default"),
        ("/install.sh", "GET", "default"),
        ("/nonmatch", "GET", "default"),
    ),
)
def test_landing_asset_budget_matches_only_static_gets(
    path: str, method: str, expected_policy: str
):
    rate_limit = RateLimitConfig(**load_config("app.yaml")["rate_limit"])
    policy = rate_limit.resolve(path, method)
    assert policy.name == expected_policy
    expected_limits = (
        LANDING_RATE_LIMIT_WINDOWS
        if expected_policy == "landing_assets"
        else [rate_limit.effective_default().pair]
    )
    assert policy.limits == expected_limits


@pytest.mark.parametrize(
    ("path", "method", "expected_policy", "expected_limits"),
    (
        ("/", "GET", "default", PAGE_RATE_LIMIT_WINDOWS),
        ("/docs", "GET", "default", PAGE_RATE_LIMIT_WINDOWS),
        ("/app/", "GET", "default", PAGE_RATE_LIMIT_WINDOWS),
        ("/app/sw.js", "GET", "default", PAGE_RATE_LIMIT_WINDOWS),
        ("/app/static/theme-boot.js", "GET", "app_static", APP_STATIC_RATE_LIMIT_WINDOWS),
        ("/app/static/version.json", "GET", "app_static", APP_STATIC_RATE_LIMIT_WINDOWS),
        (
            "/app/static/assets/index-3f9a.js",
            "GET",
            "app_static",
            APP_STATIC_RATE_LIMIT_WINDOWS,
        ),
        ("/app/static/theme-boot.js", "POST", "default", PAGE_RATE_LIMIT_WINDOWS),
        ("/api/devices", "GET", "spa_api", SPA_API_RATE_LIMIT_WINDOWS),
        ("/api/gateway-token", "POST", "spa_api", SPA_API_RATE_LIMIT_WINDOWS),
        (ICE_SERVERS_ROUTE_PATH, "POST", "spa_api", SPA_API_RATE_LIMIT_WINDOWS),
        ("/api/devices/lookup", "POST", "spa_api", SPA_API_RATE_LIMIT_WINDOWS),
        ("/api/push/subscribe", "POST", "spa_api", SPA_API_RATE_LIMIT_WINDOWS),
    ),
)
def test_pages_and_the_spa_have_budgets_for_a_shared_address(
    path: str, method: str, expected_policy: str, expected_limits: list
):
    rate_limit = RateLimitConfig(**load_config("app.yaml")["rate_limit"])
    policy = rate_limit.resolve(path, method)
    assert (policy.name, policy.key, policy.limits) == (
        expected_policy,
        "ip",
        expected_limits,
    )


@pytest.mark.asyncio
async def test_a_bridges_signed_posts_have_budgets_of_their_own():
    """One home's address carries its bridges' heartbeats, transport reports
    and push notifies and the people there browsing, and all of it shared the 60/minute default:
    14 beats refused 429 in one evening (#131). The device-signed posts count
    against budgets of their own, so a household spending its default budget
    leaves its bridges' beats untouched."""
    rate_limit = RateLimitConfig(**load_config("app.yaml")["rate_limit"])
    beat_policy = rate_limit.resolve(HEARTBEAT_ROUTE_PATH, "POST")
    report_policy = rate_limit.resolve(REPORT_ROUTE_PATH, "POST")
    notify_policy = rate_limit.resolve(NOTIFY_ROUTE_PATH, "POST")
    browsing_policy = rate_limit.resolve("/api/devices", "GET")
    limiter = RateLimiter(redis_client=None)
    household = "203.0.113.9"

    assert beat_policy.limits == BRIDGE_BEAT_RATE_LIMIT_WINDOWS
    assert report_policy.limits == BRIDGE_REPORT_RATE_LIMIT_WINDOWS
    assert notify_policy.limits == BRIDGE_NOTIFY_RATE_LIMIT_WINDOWS
    assert (
        len({beat_policy.name, report_policy.name, notify_policy.name, browsing_policy.name})
        == 4
    )

    (browsing_limit, _), = browsing_policy.limits
    for _ in range(browsing_limit):
        assert (
            await limiter.check(browsing_policy.name, household, browsing_policy.limits)
        ).allowed
    assert not (
        await limiter.check(browsing_policy.name, household, browsing_policy.limits)
    ).allowed
    for _ in range(300):
        assert (await limiter.check(beat_policy.name, household, beat_policy.limits)).allowed
    assert not (await limiter.check(beat_policy.name, household, beat_policy.limits)).allowed
    assert rate_limit.resolve(HEARTBEAT_ROUTE_PATH, "GET").name == browsing_policy.name


def test_the_waitlist_join_route_is_rate_limited_far_below_the_default():
    rate_limit = RateLimitConfig(**load_config("app.yaml")["rate_limit"])
    join_policy = rate_limit.resolve(JOIN_ROUTE_PATH, "POST")
    assert join_policy.key == "ip"
    assert join_policy.limits == JOIN_RATE_LIMIT_WINDOWS
    assert rate_limit.resolve(JOIN_ROUTE_PATH, "GET").name == "spa_api"


def test_production_database_url_is_env_driven():
    assert load_config("app.yaml")["db"]["url"] == "$DATABASE_URL"


def test_production_disallows_internal_localhost_fallback():
    config = load_config("app.yaml")
    assert config.get("internal_api", {}).get("allow_localhost", False) is False


def test_production_csp_has_no_third_party_origins():
    directives = _csp_directives(load_config("app.yaml"))
    allowed_external = {"wss://relay.getbuild.ing"}
    for directive, sources in directives.items():
        for source in sources:
            # nosemgrep: javascript.lang.security.detect-insecure-websocket.detect-insecure-websocket
            if source.startswith(("http://", "https://", "ws://", "wss://")):
                assert source in allowed_external, f"{directive} allows {source}"
            assert "esm.sh" not in source
            assert "googleapis" not in source and "gstatic" not in source


def test_production_csp_connect_src_is_self_plus_relay():
    # data: is load-bearing: ghostty-web instantiates its wasm from a data: URL.
    directives = _csp_directives(load_config("app.yaml"))
    assert set(directives["connect-src"]) == {"'self'", "data:", "wss://relay.getbuild.ing"}


def test_production_csp_core_directives_are_self():
    directives = _csp_directives(load_config("app.yaml"))
    assert directives["default-src"] == ["'self'"]
    assert "'self'" in directives["script-src"]
    assert "'unsafe-inline'" not in directives["script-src"]
    assert "'unsafe-eval'" not in directives["script-src"]
    assert directives["form-action"] == ["'self'", "getbuilding:"]
    assert directives["base-uri"] == ["'self'"]
    assert directives["frame-ancestors"] == ["'none'"]
    assert directives["object-src"] == ["'none'"]


def test_production_csp_nonce_enabled():
    assert load_config("app.yaml")["security_headers"]["csp_nonce"] is True


def test_waitlist_controller_registered_in_both_configs():
    for config_name in ("app.yaml", "app.dev.yaml"):
        controllers = load_config(config_name)["controllers"]
        assert "buildapp.waitlist_controller:WaitlistController" in controllers, (
            f"{config_name} does not serve the public waitlist endpoint"
        )


def test_rtc_controller_registered_in_every_config():
    for config_name in ("app.yaml", "app.dev.yaml", "app.mail.yaml"):
        controllers = load_config(config_name)["controllers"]
        assert "buildapp.rtc_controller:RtcController" in controllers, (
            f"{config_name} does not serve the ICE-servers route the SPA upgrades with"
        )


def test_every_invite_controller_is_registered_in_every_config():
    for config_name in ("app.yaml", "app.dev.yaml", "app.mail.yaml"):
        controllers = load_config(config_name)["controllers"]
        for controller in INVITE_CONTROLLERS:
            assert controller in controllers, f"{config_name} does not serve {controller}"


def test_opening_an_invite_link_is_rate_limited_because_the_token_is_a_secret():
    rate_limit = RateLimitConfig(**load_config("app.yaml")["rate_limit"])
    policy = rate_limit.resolve(SAMPLE_INVITE_PATH, "GET")
    assert policy.key == "ip"
    assert policy.limits == INVITE_OPEN_RATE_LIMIT_WINDOWS


def test_the_waitlist_invite_send_is_rate_limited_but_the_page_is_not():
    rate_limit = RateLimitConfig(**load_config("app.yaml")["rate_limit"])
    policy = rate_limit.resolve(SAMPLE_WAITLIST_SEND_PATH, "POST")
    assert policy.name == "waitlist_invite_send"
    assert policy.key == "ip"
    assert policy.limits == WAITLIST_INVITE_SEND_RATE_LIMIT_WINDOWS
    assert rate_limit.resolve(WAITLIST_ADMIN_PATH, "GET").limits == [
        rate_limit.effective_default().pair
    ]
