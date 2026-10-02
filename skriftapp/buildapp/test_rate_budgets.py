"""About 50 people behind one address (an office, a campus, a carrier's NAT)
using Build normally are never answered 429, while the waitlist join stays
tight (#173). The requests are what the landing page, the SPA and the bridge
make, counted from their code; they go through Skrift's own
``RateLimitMiddleware`` built from app.yaml, all inside one window, which is
the worst case for a per-minute budget."""

from __future__ import annotations

from pathlib import Path

import pytest
import yaml
from skrift.config import RateLimitConfig
from skrift.middleware.rate_limit import RateLimitMiddleware
from skrift.ratelimit import RateLimiter

from buildapp.devices_controller import HEARTBEAT_ROUTE_PATH
from buildapp.push_controller import NOTIFY_ROUTE_PATH
from buildapp.rtc_controller import ICE_SERVERS_ROUTE_PATH
from buildapp.transport_controller import REPORT_ROUTE_PATH
from buildapp.waitlist_controller import JOIN_ROUTE_PATH

APP_YAML = Path(__file__).resolve().parent.parent / "app.yaml"
SHARED_ADDRESS = "198.51.100.20"
PEOPLE = 50

# The /app static files a load always refetches: none are content-hashed.
APP_UNCACHED_STATIC = [
    ("GET", "/app/static/theme-boot.js"),
    ("GET", "/app/static/manifest.webmanifest"),
    ("GET", "/app/static/icon-192.png"),
]
# One online device: its devices poll, relay dial and peer link.
APP_FIRST_API_CALLS = [
    ("GET", "/api/devices"),
    ("POST", "/api/gateway-token"),
    ("POST", ICE_SERVERS_ROUTE_PATH),
]
APP_COLD_LOAD = [
    ("GET", "/app/"),
    *APP_UNCACHED_STATIC,
    ("GET", "/app/static/assets/index-3f9a.js"),
    ("GET", "/app/static/assets/rolldown-runtime-81c2.js"),
    ("GET", "/app/static/assets/preload-helper-5d0e.js"),
    ("GET", "/app/static/assets/index-77b1.css"),
    *[
        ("GET", f"/app/static/assets/inter-latin-{weight}-normal.woff2")
        for weight in range(400, 900, 100)
    ],
    ("GET", "/app/static/assets/libsodium-wrappers-a1b2.js"),
    ("GET", "/app/static/assets/ghostty-web-c3d4.js"),
    ("GET", "/app/sw.js"),
    *APP_FIRST_API_CALLS,
]
# The hashed assets are immutable, so a reload skips them.
APP_WARM_RELOAD = [
    ("GET", "/app/"),
    *APP_UNCACHED_STATIC,
    ("GET", "/app/sw.js"),
    *APP_FIRST_API_CALLS,
]
# The devices poll runs every 15 s.
APP_IDLE_MINUTE = [("GET", "/api/devices")] * 4
# Film mode, scrolled to the end; /landing/ is not cached, so each view
# fetches all of it.
LANDING_VIEW = [
    ("GET", "/"),
    *[("GET", f"/landing/generated/asset-{index}") for index in range(32)],
]
# Two beats, and a session minted, carrying, then carrying again off TURN.
BRIDGE_MINUTE = [
    ("POST", HEARTBEAT_ROUTE_PATH),
    ("POST", HEARTBEAT_ROUTE_PATH),
    ("POST", REPORT_ROUTE_PATH),
    ("POST", REPORT_ROUTE_PATH),
    ("POST", REPORT_ROUTE_PATH),
    ("POST", NOTIFY_ROUTE_PATH),
]
ONE_PERSONS_MINUTE = [
    *LANDING_VIEW,
    *APP_COLD_LOAD,
    *APP_IDLE_MINUTE,
    *APP_WARM_RELOAD,
    *BRIDGE_MINUTE,
]


async def _answered_ok(scope: dict, receive, send) -> None:
    await send({"type": "http.response.start", "status": 200, "headers": []})
    await send({"type": "http.response.body", "body": b""})


def _production_rate_limiter() -> RateLimitMiddleware:
    config = RateLimitConfig(**yaml.safe_load(APP_YAML.read_text())["rate_limit"])
    return RateLimitMiddleware(
        _answered_ok, config=config, limiter=RateLimiter(redis_client=None)
    )


async def _status(middleware: RateLimitMiddleware, method: str, path: str) -> int:
    scope = {
        "type": "http",
        "method": method,
        "path": path,
        "headers": [],
        "client": (SHARED_ADDRESS, 50000),
        "state": {"client_ip": SHARED_ADDRESS},
    }
    sent: list[dict] = []

    async def receive() -> dict:
        return {"type": "http.request", "body": b"", "more_body": False}

    async def send(message: dict) -> None:
        sent.append(message)

    await middleware(scope, receive, send)
    return sent[0]["status"]


@pytest.mark.asyncio
async def test_fifty_people_on_one_address_are_never_answered_429():
    middleware = _production_rate_limiter()
    refused = []
    for _ in range(PEOPLE):
        for method, path in ONE_PERSONS_MINUTE:
            if await _status(middleware, method, path) == 429:
                refused.append((method, path))
    assert refused == []


# A device being paired on the same address (#321): its bridge asks for its
# approval every half second, and the tab that approved it reads the devices
# every second while it comes up.
PAIRING_MINUTE = [
    *[("GET", "/api/devices/7d1f1d4e-8a54-4a4e-9a8e-2f7d2c1b9a10/status")] * 120,
    *[("GET", "/api/devices")] * 60,
]


@pytest.mark.asyncio
async def test_a_device_paired_on_a_busy_address_is_never_answered_429():
    middleware = _production_rate_limiter()
    refused = []
    for _ in range(PEOPLE):
        for method, path in ONE_PERSONS_MINUTE:
            await _status(middleware, method, path)
    for method, path in PAIRING_MINUTE:
        if await _status(middleware, method, path) == 429:
            refused.append((method, path))
    assert refused == []


@pytest.mark.asyncio
async def test_the_waitlist_join_stays_tight_on_a_busy_address():
    middleware = _production_rate_limiter()
    for _ in range(PEOPLE):
        for method, path in ONE_PERSONS_MINUTE:
            await _status(middleware, method, path)

    joins = [await _status(middleware, "POST", JOIN_ROUTE_PATH) for _ in range(4)]
    assert joins == [200, 200, 200, 429]
