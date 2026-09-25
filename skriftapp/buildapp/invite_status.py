"""How an admin page says what became of an invite: one short phrase per state, the
moment it refers to as a relative time, and that moment in full for the hover. Both
admin pages render an invite's state through this and the ``admin/_invite_status.html``
macro, so the waitlist and the invites list cannot drift apart in how they say it.

Pure over its inputs — no database, no clock — like the dashboards that call it."""

from __future__ import annotations

from dataclasses import dataclass
from datetime import datetime, timedelta

from buildapp.invites import InviteState, invite_state
from buildapp.models import Invite

NOT_INVITED_LABEL = "Not invited"
#: The phrase each state reads as. A state with a moment is followed by that moment.
STATE_LABELS = {
    InviteState.UNKNOWN: NOT_INVITED_LABEL,
    InviteState.OPEN: "Invited",
    InviteState.REDEEMED: "Joined",
    InviteState.EXPIRED: "Expired",
    InviteState.REVOKED: "Revoked",
}
ABSOLUTE_FORMAT = "%Y-%m-%d %H:%M UTC"
JUST_NOW = "just now"
#: Coarsest unit first: a moment reads in the largest unit it fills at least once.
RELATIVE_UNITS = (
    ("day", timedelta(days=1)),
    ("hour", timedelta(hours=1)),
    ("minute", timedelta(minutes=1)),
)


@dataclass(frozen=True)
class Moment:
    """A point in time as a page shows it: relative in the text, exact on hover, and
    machine-readable in ``<time datetime>``."""

    relative: str
    absolute: str
    iso: str


@dataclass(frozen=True)
class InviteStatus:
    state: InviteState
    label: str
    moment: Moment | None


def relative_time(then: datetime, now: datetime) -> str:
    elapsed = now - then
    for unit, size in RELATIVE_UNITS:
        count = elapsed // size
        if count >= 1:
            return f"{count} {unit}{'' if count == 1 else 's'} ago"
    return JUST_NOW


def moment(then: datetime, now: datetime) -> Moment:
    return Moment(
        relative=relative_time(then, now),
        absolute=then.strftime(ABSOLUTE_FORMAT),
        iso=then.isoformat(),
    )


def invite_status(invite: Invite | None, now: datetime) -> InviteStatus:
    """Invited and Joined carry when; Expired, Revoked and never-invited stand alone."""
    state = invite_state(invite, now)
    when = None
    if state is InviteState.OPEN:
        when = invite.created_at
    elif state is InviteState.REDEEMED:
        when = invite.redeemed_at
    return InviteStatus(
        state=state,
        label=STATE_LABELS[state],
        moment=None if when is None else moment(when, now),
    )
