"""What a visitor sees at the end of an invite link, and what a signed-in non-member
sees at /app/.

Every outcome is a row in ``OUTCOMES`` — status code, title, heading, message, the
links under it — so no handler ever branches on a state to pick copy. This module owns
the words; the markup around them comes from fragments in the landing directory, and
the only runtime value on any page (the signed-in address) is escaped.
"""

from __future__ import annotations

from dataclasses import dataclass
from html import escape

from litestar.enums import MediaType
from litestar.response import Response
from litestar.status_codes import (
    HTTP_200_OK,
    HTTP_403_FORBIDDEN,
    HTTP_404_NOT_FOUND,
    HTTP_410_GONE,
)

from buildapp.invites import ALREADY_MEMBER, EMAIL_MISMATCH, InviteState, RedemptionRefusal
from buildapp.landing_page import HOME_LINK, Link, render_panel_page

ACTION_SEPARATOR = " "

APP_PATH = "/app/"
LOGOUT_PATH = "/auth/logout"
WAITLIST_PATH = "/#waitlist"

OPEN_BUILD_LABEL = "OPEN BUILD"
REQUEST_ACCESS_LABEL = "REQUEST ACCESS"
SIGN_OUT_LABEL = "Sign out"

UNKNOWN_TITLE = "Invite not found — Build"
UNKNOWN_HEADING = "This invite link is not valid."
UNKNOWN_MESSAGE = (
    "Check the link in your invite email, or ask for a new one from whoever invited you."
)

REVOKED_TITLE = "Invite withdrawn — Build"
REVOKED_HEADING = "This invite was withdrawn."
REVOKED_MESSAGE = "The seat it held was given up. Ask for a new invite if you still want one."

EXPIRED_TITLE = "Invite expired — Build"
EXPIRED_HEADING = "This invite has expired."
EXPIRED_MESSAGE = "Invites last 14 days. Ask whoever invited you to send another."

REDEEMED_TITLE = "Invite already used — Build"
REDEEMED_HEADING = "This invite has already been used."
REDEEMED_MESSAGE = (
    "If that was you, Build is waiting. Otherwise, ask whoever invited you for another link."
)

MISMATCH_TITLE = "Wrong account — Build"
MISMATCH_HEADING = "This invite is for a different address."
MISMATCH_MESSAGE = (
    "Sign out and sign back in with the address the invite was sent to, then open the "
    "link again."
)

INVITE_ONLY_TITLE = "Invite only — Build"
INVITE_ONLY_HEADING = "Build is invite-only right now."
INVITE_ONLY_MESSAGE_TEMPLATE = (
    "You’re signed in as {address}, which has no invite yet. Invites go out as seats open."
)


def render_links(links: tuple[Link, ...]) -> str:
    return ACTION_SEPARATOR.join(link.render() for link in links)


@dataclass(frozen=True)
class PageOutcome:
    """One row of the outcome table: the page a reason renders, and the status it
    answers with."""

    status_code: int
    title: str
    heading: str
    message: str
    links: tuple[Link, ...] = ()

    def render(self) -> str:
        return render_panel_page(
            title=self.title,
            heading=self.heading,
            message=self.message,
            action=render_links(self.links),
        )

    def response(self) -> Response:
        return Response(
            self.render(), media_type=MediaType.HTML, status_code=self.status_code
        )


APP_LINK = Link(label=OPEN_BUILD_LABEL, href=APP_PATH, primary=True)
SIGN_OUT_LINK = Link(label=SIGN_OUT_LABEL, href=LOGOUT_PATH)
REQUEST_ACCESS_LINK = Link(label=REQUEST_ACCESS_LABEL, href=WAITLIST_PATH, primary=True)

#: Reason → page. The invite handler looks a reason up here and returns the response;
#: adding a state means adding a row, never a branch.
OUTCOMES: dict[RedemptionRefusal, PageOutcome] = {
    ALREADY_MEMBER: PageOutcome(
        status_code=HTTP_200_OK,
        title="Already a member — Build",
        heading="You already have access to Build.",
        message="This invite is still unused. Open Build with your existing membership.",
        links=(APP_LINK,),
    ),
    InviteState.UNKNOWN: PageOutcome(
        status_code=HTTP_404_NOT_FOUND,
        title=UNKNOWN_TITLE,
        heading=UNKNOWN_HEADING,
        message=UNKNOWN_MESSAGE,
        links=(HOME_LINK,),
    ),
    InviteState.REVOKED: PageOutcome(
        status_code=HTTP_410_GONE,
        title=REVOKED_TITLE,
        heading=REVOKED_HEADING,
        message=REVOKED_MESSAGE,
        links=(HOME_LINK,),
    ),
    InviteState.EXPIRED: PageOutcome(
        status_code=HTTP_410_GONE,
        title=EXPIRED_TITLE,
        heading=EXPIRED_HEADING,
        message=EXPIRED_MESSAGE,
        links=(HOME_LINK,),
    ),
    InviteState.REDEEMED: PageOutcome(
        status_code=HTTP_200_OK,
        title=REDEEMED_TITLE,
        heading=REDEEMED_HEADING,
        message=REDEEMED_MESSAGE,
        links=(APP_LINK,),
    ),
    EMAIL_MISMATCH: PageOutcome(
        status_code=HTTP_403_FORBIDDEN,
        title=MISMATCH_TITLE,
        heading=MISMATCH_HEADING,
        message=MISMATCH_MESSAGE,
        links=(SIGN_OUT_LINK,),
    ),
}

# A registration can lose the invite after Skrift has committed its account. The
# browser needs a fixed local destination for the refusal, independent of Skrift's
# optional `next` redirect and without putting the raw invite token in another URL.
CLAIM_OUTCOME_PATH_PREFIX = "/invite/outcome/"
CLAIM_OUTCOME_SLUGS: dict[RedemptionRefusal, str] = {
    InviteState.UNKNOWN: "unknown",
    InviteState.REVOKED: "revoked",
    InviteState.EXPIRED: "expired",
    InviteState.REDEEMED: "redeemed",
    EMAIL_MISMATCH: "wrong-account",
    ALREADY_MEMBER: "already-member",
}
CLAIM_OUTCOME_REASONS = {slug: reason for reason, slug in CLAIM_OUTCOME_SLUGS.items()}


def claim_outcome_path(reason: RedemptionRefusal) -> str:
    """A token-free URL for the exact claim refusal; unknown reasons are generic."""
    return f"{CLAIM_OUTCOME_PATH_PREFIX}{CLAIM_OUTCOME_SLUGS.get(reason, 'unknown')}"


def claim_outcome_for_slug(slug: str) -> PageOutcome:
    """Only static, allowlisted outcome pages may be selected from a URL segment."""
    return OUTCOMES[CLAIM_OUTCOME_REASONS.get(slug, InviteState.UNKNOWN)]


def invite_only_outcome(email: str) -> PageOutcome:
    """What /app/ shows an account with no invite. A row like every other — the 403 is
    written here and nowhere else — built per call because it names the address it is
    refusing, so the visitor can tell they are signed in as the wrong one."""
    return PageOutcome(
        status_code=HTTP_403_FORBIDDEN,
        title=INVITE_ONLY_TITLE,
        heading=INVITE_ONLY_HEADING,
        message=INVITE_ONLY_MESSAGE_TEMPLATE.format(address=escape(email)),
        links=(REQUEST_ACCESS_LINK, SIGN_OUT_LINK),
    )
