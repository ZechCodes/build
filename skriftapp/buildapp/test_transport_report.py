"""The transport report's pure logic: the challenge the bridge signs, the words
the api accepts, and how a session row absorbs events in any order
(``planning/v2/Transport Telemetry Spec.md`` §Wire, §Storage)."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone

import pytest

from buildapp import transport_report
from buildapp.transport_report import (
    ALLOWED_EVENTS,
    ALLOWED_PATHS,
    NO_PATH,
    SessionRow,
    apply_event,
    report_challenge,
)

T0 = datetime(2026, 9, 5, 17, 0, tzinfo=timezone.utc)


def at(seconds: int) -> datetime:
    return T0 + timedelta(seconds=seconds)


def test_the_challenge_is_byte_for_byte_what_the_bridge_signs():
    # bridge/src/transport_report.rs::report_challenge, same inputs.
    assert (
        report_challenge("dev-1", "sess-1", "carrying", "turn", 1750000000)
        == "transport.dev-1.sess-1.carrying.turn.1750000000"
    )
    assert (
        report_challenge("dev-1", "sess-1", "minted", NO_PATH, 1750000000)
        == "transport.dev-1.sess-1.minted.-.1750000000"
    )


def test_the_words_are_the_events_and_two_paths_the_bridge_sends():
    # ``channels_lost`` is what a bridge sends now; ``fell_back`` is what one a
    # release behind still sends, and the api takes both for that release.
    assert ALLOWED_EVENTS == frozenset(
        {"minted", "carrying", "channels_lost", "fell_back", "ended"}
    )
    assert ALLOWED_PATHS == frozenset({"direct", "turn"})
    assert NO_PATH == "-"


def test_an_ordinary_session_reads_direct_then_ended():
    row = SessionRow()
    apply_event(row, "minted", NO_PATH, at(0))
    apply_event(row, "carrying", "direct", at(1))
    apply_event(row, "ended", NO_PATH, at(60))
    assert (row.minted_at, row.first_carrying_at, row.ended_at) == (at(0), at(1), at(60))
    assert (row.first_path, row.current_path) == ("direct", "direct")
    assert (row.carrying_count, row.turn_count, row.fell_back_count) == (1, 0, 0)


def test_a_turn_session_counts_every_turn_carry_and_keeps_its_first_path():
    row = SessionRow()
    apply_event(row, "minted", NO_PATH, at(0))
    apply_event(row, "carrying", "turn", at(1))
    # An ICE restart lands on a direct path this time.
    apply_event(row, "carrying", "direct", at(30))
    assert (row.first_path, row.current_path) == ("turn", "direct")
    assert (row.carrying_count, row.turn_count) == (2, 1)


def test_losing_the_last_channel_puts_the_session_back_on_no_path():
    row = SessionRow()
    apply_event(row, "minted", NO_PATH, at(0))
    apply_event(row, "carrying", "direct", at(1))
    apply_event(row, "channels_lost", NO_PATH, at(5))
    assert row.current_path == "relay"
    assert row.fell_back_count == 1
    # …and carries again after (an ICE restart brought the channels back).
    apply_event(row, "carrying", "direct", at(9))
    assert (row.current_path, row.carrying_count, row.fell_back_count) == ("direct", 2, 1)


def test_the_retired_word_for_that_event_counts_in_the_same_column():
    # One release of tolerance: a bridge that has not been updated says
    # ``fell_back`` and lands in the column ``channels_lost`` lands in, so the
    # admin page reads one number over a mixed fleet.
    row = SessionRow()
    apply_event(row, "minted", NO_PATH, at(0))
    apply_event(row, "carrying", "direct", at(1))
    apply_event(row, "fell_back", NO_PATH, at(5))
    apply_event(row, "channels_lost", NO_PATH, at(7))
    assert (row.current_path, row.fell_back_count) == ("relay", 2)


def test_a_session_that_never_carries_never_connected():
    row = SessionRow()
    apply_event(row, "minted", NO_PATH, at(0))
    apply_event(row, "ended", NO_PATH, at(10))
    assert (row.first_path, row.current_path, row.first_carrying_at) == (None, "relay", None)


def test_events_out_of_order_or_repeated_are_absorbed():
    row = SessionRow()
    # A carrying that beat its minted still counts, and the minted lands after.
    apply_event(row, "carrying", "direct", at(1))
    apply_event(row, "minted", NO_PATH, at(0))
    assert (row.minted_at, row.first_carrying_at) == (at(0), at(1))
    # A second minted is a no-op; an event after ended does not unset the end.
    apply_event(row, "minted", NO_PATH, at(2))
    apply_event(row, "ended", NO_PATH, at(10))
    apply_event(row, "channels_lost", NO_PATH, at(11))
    assert row.minted_at == at(0)
    assert row.ended_at == at(10)
    assert row.fell_back_count == 1


def test_a_carrying_without_a_path_or_a_non_carrying_with_one_is_refused():
    row = SessionRow()
    with pytest.raises(ValueError):
        apply_event(row, "carrying", NO_PATH, at(0))
    with pytest.raises(ValueError):
        apply_event(row, "minted", "direct", at(0))
    with pytest.raises(ValueError):
        apply_event(row, "teleported", NO_PATH, at(0))


def test_the_freshness_window_and_replay_guard_are_the_notify_ones():
    # One scheme for every device-signed report: the same window, the same guard.
    from buildapp import web_push

    assert transport_report.report_timestamp_fresh is web_push.notify_timestamp_fresh
    assert isinstance(transport_report.replay_guard(), web_push.NotifyReplayGuard)
