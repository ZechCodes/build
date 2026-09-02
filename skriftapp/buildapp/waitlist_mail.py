"""The waitlist's outbound mail: the context a send needs (public base URL, signing key,
owner notify address) and the two messages a new signup produces — a confirmation to the
signer and a one-line notification to the maintainer."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass

from skrift.config import Settings

from buildapp.email_message import OutboundEmail, compose_email

NOTIFY_ADDRESS_ENV = "WAITLIST_NOTIFY_ADDRESS"

CONFIRMATION_SUBJECT = "You’re on the Build list"
CONFIRMATION_HEADING = "You’re on the list."
CONFIRMATION_PARAGRAPHS = (
    "Build is the agentic coding IDE for teams. Invites go out weekly.",
    "The next email you get from us is your invite. Nothing else.",
)
OWNER_SUBJECT_PREFIX = "Waitlist: "
OWNER_HEADING = "New waitlist signup"
OWNER_PARAGRAPH_TEMPLATE = "{signup_email} joined the Build waitlist."


@dataclass(frozen=True)
class WaitlistEmailContext:
    public_base_url: str
    secret_key: str
    notify_address: str


def resolve_public_base_url(settings: Settings) -> str:
    return (settings.email.public_base_url or settings.auth.redirect_base_url or "").rstrip(
        "/"
    )


def resolve_waitlist_email_context(
    settings: Settings, environment: Mapping[str, str]
) -> WaitlistEmailContext:
    public_base_url = resolve_public_base_url(settings)
    if not public_base_url:
        raise ValueError(
            "email.public_base_url (or auth.redirect_base_url) must be set to send waitlist mail"
        )
    return WaitlistEmailContext(
        public_base_url=public_base_url,
        secret_key=settings.secret_key,
        notify_address=environment.get(NOTIFY_ADDRESS_ENV, "").strip(),
    )


def build_confirmation_email(*, to: str, unsubscribe_url: str) -> OutboundEmail:
    return compose_email(
        to=to,
        subject=CONFIRMATION_SUBJECT,
        heading=CONFIRMATION_HEADING,
        paragraphs=CONFIRMATION_PARAGRAPHS,
        unsubscribe_url=unsubscribe_url,
        one_click=True,
    )


def build_owner_notification_email(
    *, to: str, signup_email: str, unsubscribe_url: str
) -> OutboundEmail:
    return compose_email(
        to=to,
        subject=f"{OWNER_SUBJECT_PREFIX}{signup_email}",
        heading=OWNER_HEADING,
        paragraphs=(OWNER_PARAGRAPH_TEMPLATE.format(signup_email=signup_email),),
        unsubscribe_url=unsubscribe_url,
        one_click=False,
    )
