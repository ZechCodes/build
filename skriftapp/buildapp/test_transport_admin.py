"""The admin transport page: how sessions are bucketed, what the page shows,
and that it is an admin page (``planning/v2/Transport Telemetry Spec.md``
§Classification)."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from uuid import uuid4

from litestar.handlers import HTTPRouteHandler
from skrift.admin.navigation import ADMIN_NAV_TAG
from skrift.auth.guards import auth_guard

from buildapp.db_test_support import admin_template_environment
from buildapp.models import Device, TransportSession
from buildapp.transport_admin import (
    DIRECT,
    NEVER_CONNECTED,
    TURN,
    UNSTABLE,
    WINDOWS,
    TransportAdminController,
    build_transport_dashboard,
    classify,
)

NOW = datetime(2026, 9, 5, 18, 0, tzinfo=timezone.utc)


def session(**fields) -> TransportSession:
    defaults = dict(
        session_id="sess-" + uuid4().hex[:6],
        device_id=uuid4(),
        owner_user_id=uuid4(),
        minted_at=NOW - timedelta(hours=1),
        first_carrying_at=None,
        first_path=None,
        current_path="relay",
        carrying_count=0,
        turn_count=0,
        fell_back_count=0,
        ended_at=None,
    )
    defaults.update(fields)
    return TransportSession(**defaults)


def test_the_four_buckets_are_the_ones_the_operator_asked_for():
    assert classify(session(first_path="direct", carrying_count=1)) == DIRECT
    assert classify(session(first_path="turn", carrying_count=1, turn_count=1)) == TURN
    # TURN at any point in the session's life is TURN, even after a direct restart.
    assert classify(session(first_path="direct", carrying_count=2, turn_count=1)) == TURN
    assert classify(session()) == NEVER_CONNECTED
    assert classify(session(first_path="direct", carrying_count=2, fell_back_count=1)) == UNSTABLE


def test_the_dashboard_counts_within_the_window_and_per_device():
    laptop = Device(
        id=uuid4(), name="laptop", owner_user_id=uuid4(), identity_public_key_b64="k",
        transport_public_key_b64="t", approved=True,
    )
    phone_bridge = Device(
        id=uuid4(), name="mini", owner_user_id=laptop.owner_user_id, identity_public_key_b64="k",
        transport_public_key_b64="t", approved=True,
    )
    rows = [
        session(device_id=laptop.id, first_path="direct", carrying_count=1),
        session(device_id=laptop.id, first_path="turn", carrying_count=1, turn_count=1),
        session(device_id=laptop.id),
        session(device_id=phone_bridge.id, first_path="direct", carrying_count=2, fell_back_count=1),
        # Outside a 24h window, inside 7d.
        session(device_id=phone_bridge.id, minted_at=NOW - timedelta(days=3)),
    ]
    dashboard = build_transport_dashboard(rows, [laptop, phone_bridge], window="24h", now=NOW)
    assert dashboard["window"] == "24h"
    assert dashboard["totals"] == {
        "sessions": 4,
        DIRECT: 1,
        TURN: 1,
        NEVER_CONNECTED: 1,
        UNSTABLE: 1,
    }
    by_name = {row["device"]: row for row in dashboard["devices"]}
    assert by_name["laptop"]["sessions"] == 3
    assert by_name["laptop"][TURN] == 1
    assert by_name["mini"][UNSTABLE] == 1
    assert dashboard["direct_share"] == "25%"
    assert dashboard["never_connected_share"] == "25%"

    week = build_transport_dashboard(rows, [laptop, phone_bridge], window="7d", now=NOW)
    assert week["totals"]["sessions"] == 5
    assert week["totals"][NEVER_CONNECTED] == 2


def test_recent_sessions_are_newest_first_with_their_path_words():
    rows = [
        session(session_id="old", minted_at=NOW - timedelta(hours=5), first_path="direct", carrying_count=1),
        session(session_id="new", minted_at=NOW - timedelta(minutes=5), first_path="turn", carrying_count=1, turn_count=1, ended_at=NOW),
    ]
    dashboard = build_transport_dashboard(rows, [], window="7d", now=NOW)
    recent = dashboard["recent"]
    assert [r["session_id"] for r in recent] == ["new", "old"]
    assert recent[0]["bucket"] == TURN
    assert recent[0]["device"] == "unknown device"
    assert recent[0]["ended"] is True and recent[1]["ended"] is False


def test_an_unknown_window_falls_back_to_the_default():
    dashboard = build_transport_dashboard([], [], window="forever", now=NOW)
    assert dashboard["window"] == "7d"
    assert list(WINDOWS) == ["24h", "7d", "30d"]


def test_the_page_is_an_admin_nav_page_behind_the_administrator_permission():
    (handler,) = [h for h in vars(TransportAdminController).values() if isinstance(h, HTTPRouteHandler)]
    assert ADMIN_NAV_TAG in (handler.tags or [])
    assert auth_guard in (handler.guards or [])
    assert any(getattr(g, "permission", None) == "administrator" or "administrator" in repr(g) for g in handler.guards)
    assert handler.opt["label"] == "Transport"
    assert "/transport" in {p for p in handler.paths}


def test_the_template_renders_the_four_figures_and_the_caveat():
    env = admin_template_environment()
    dashboard = build_transport_dashboard(
        [session(first_path="turn", carrying_count=1, turn_count=1)], [], window="7d", now=NOW
    )
    html = env.get_template("admin/transport.html").render(dashboard=dashboard, site_name=lambda: "Build")
    for label in ["Direct WebRTC", "TURN", "Never connected", "Unstable", "Cloudflare"]:
        assert label in html, label
    assert "prflx" in html, "the under-count caveat is on the page, under the TURN figure"


def test_the_page_never_says_fallback():
    """One event, one word. The stat cards, the bucket labels and the OPS grep
    all say a session lost its channels; the recent-sessions column headed
    "Fallbacks" was the one place the retired word survived, over the very count
    the rename is about (``fell_back_count``, a column name kept to avoid a
    migration and commented as such)."""
    env = admin_template_environment()
    dashboard = build_transport_dashboard(
        [session(first_path="direct", carrying_count=1, fell_back_count=2)], [], window="7d", now=NOW
    )
    html = env.get_template("admin/transport.html").render(dashboard=dashboard, site_name=lambda: "Build")
    assert "Channels lost" in html, "the column says what the number counts"
    assert "allback" not in html, "no surface of this page says fallback any more"
