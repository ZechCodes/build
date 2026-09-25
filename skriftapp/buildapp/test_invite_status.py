"""How both admin pages say what became of an invite: the phrase per state, which
states carry a moment and which, and the relative time that moment reads as."""

from __future__ import annotations

from datetime import datetime, timedelta, timezone
from uuid import uuid4

import pytest

from buildapp.invite_status import (
    NOT_INVITED_LABEL,
    invite_status,
    moment,
    relative_time,
)
from buildapp.invites import INVITE_TTL, InviteState
from buildapp.models import Invite

NOW = datetime(2026, 9, 6, 12, 0, tzinfo=timezone.utc)
SENT = NOW - timedelta(days=3)


def invite(**fields) -> Invite:
    defaults = dict(
        id=uuid4(),
        token_hash="hash",
        email="invitee@example.com",
        expires_at=SENT + INVITE_TTL,
        redeemed_by=None,
        redeemed_at=None,
        revoked_at=None,
        created_at=SENT,
    )
    defaults.update(fields)
    return Invite(**defaults)


@pytest.mark.parametrize(
    ("elapsed", "reads"),
    (
        (timedelta(seconds=0), "just now"),
        (timedelta(seconds=59), "just now"),
        (timedelta(minutes=1), "1 minute ago"),
        (timedelta(minutes=59), "59 minutes ago"),
        (timedelta(hours=1), "1 hour ago"),
        (timedelta(hours=23, minutes=59), "23 hours ago"),
        (timedelta(days=1), "1 day ago"),
        (timedelta(days=40), "40 days ago"),
    ),
)
def test_relative_time_reads_in_the_largest_unit_it_fills(elapsed, reads):
    assert relative_time(NOW - elapsed, NOW) == reads


def test_a_moment_is_relative_in_text_and_exact_on_hover():
    assert moment(SENT, NOW).relative == "3 days ago"
    assert moment(SENT, NOW).absolute == "2026-09-03 12:00 UTC"
    assert moment(SENT, NOW).iso == SENT.isoformat()


def test_no_invite_reads_not_invited_with_no_moment():
    status = invite_status(None, NOW)
    assert (status.state, status.label, status.moment) == (
        InviteState.UNKNOWN,
        NOT_INVITED_LABEL,
        None,
    )


def test_an_open_invite_reads_invited_when_it_was_sent():
    status = invite_status(invite(), NOW)
    assert (status.label, status.moment.relative) == ("Invited", "3 days ago")


def test_a_redeemed_invite_reads_joined_when_it_was_redeemed():
    status = invite_status(invite(redeemed_by=uuid4(), redeemed_at=NOW - timedelta(hours=2)), NOW)
    assert (status.label, status.moment.relative) == ("Joined", "2 hours ago")


@pytest.mark.parametrize(
    ("fields", "label"),
    (
        ({"expires_at": NOW - timedelta(days=1)}, "Expired"),
        ({"revoked_at": NOW}, "Revoked"),
    ),
)
def test_a_dead_link_reads_as_its_state_alone(fields, label):
    status = invite_status(invite(**fields), NOW)
    assert (status.label, status.moment) == (label, None)
