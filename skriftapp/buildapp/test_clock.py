"""What time it is has one home. Every module that stamps a row — invites, devices,
push subscriptions, transport reports — reads the clock from here, so "now" is always
timezone-aware UTC and a test has one function to patch."""

from __future__ import annotations

import inspect
from datetime import timezone

from buildapp import (
    devices_controller,
    invites_admin,
    invites_controller,
    push_controller,
    transport_controller,
)
from buildapp.clock import utc_now


def test_now_is_timezone_aware_utc():
    now = utc_now()
    assert now.tzinfo is not None
    assert now.utcoffset() == timezone.utc.utcoffset(now)


def test_no_module_keeps_its_own_copy_of_the_clock():
    for module in (
        invites_controller,
        invites_admin,
        devices_controller,
        push_controller,
        transport_controller,
    ):
        assert "def _now" not in inspect.getsource(module), module.__name__
