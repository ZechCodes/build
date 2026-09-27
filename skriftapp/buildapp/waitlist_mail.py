"""The waitlist's outbound mail: the context a send needs, the two messages a new signup
produces — a confirmation to the signer, carrying the only unsubscribe link, and a
one-line internal notification to the maintainer, carrying none — and the background task
that delivers them after the response has gone out. Composing, delivering and resolving
this deployment's base URL are ``email_message``'s, shared with the invite mail."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass

from litestar.background_tasks import BackgroundTask
from skrift.config import Settings
from skrift.lib.email_backends import EmailBackend

from buildapp.email_message import (
    OutboundEmail,
    compose_email,
    deliver_emails,
    resolve_public_base_url,
)
from buildapp.waitlist_unsubscribe_token import mint_unsubscribe_token, unsubscribe_url

NOTIFY_ADDRESS_ENV = "WAITLIST_NOTIFY_ADDRESS"

CONFIRMATION_SUBJECT = "You’re on the Build list"
CONFIRMATION_HEADING = "You’re on the list."
CONFIRMATION_PARAGRAPHS = (
    "Build turns a task into shipped code, on your machine. Invites go out as seats open.",
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


def build_confirmation_email(
    *, to: str, unsubscribe_url: str, public_base_url: str
) -> OutboundEmail:
    return compose_email(
        to=to,
        subject=CONFIRMATION_SUBJECT,
        heading=CONFIRMATION_HEADING,
        paragraphs=CONFIRMATION_PARAGRAPHS,
        unsubscribe_url=unsubscribe_url,
        one_click=True,
        public_base_url=public_base_url,
    )


def build_owner_notification_email(
    *, to: str, signup_email: str, public_base_url: str
) -> OutboundEmail:
    return compose_email(
        to=to,
        subject=f"{OWNER_SUBJECT_PREFIX}{signup_email}",
        heading=OWNER_HEADING,
        paragraphs=(OWNER_PARAGRAPH_TEMPLATE.format(signup_email=signup_email),),
        unsubscribe_url=None,
        one_click=False,
        public_base_url=public_base_url,
    )


def build_signup_emails(
    *, signup_email: str, context: WaitlistEmailContext
) -> tuple[OutboundEmail, ...]:
    signup_unsubscribe_url = unsubscribe_url(
        context.public_base_url,
        mint_unsubscribe_token(signup_email, context.secret_key),
    )
    messages = (
        build_confirmation_email(
            to=signup_email,
            unsubscribe_url=signup_unsubscribe_url,
            public_base_url=context.public_base_url,
        ),
    )
    if not context.notify_address:
        return messages
    return messages + (
        build_owner_notification_email(
            to=context.notify_address,
            signup_email=signup_email,
            public_base_url=context.public_base_url,
        ),
    )


def waitlist_signup_email_task(
    *, email_backend: EmailBackend, signup_email: str, context: WaitlistEmailContext
) -> BackgroundTask:
    return BackgroundTask(
        deliver_emails,
        email_backend,
        build_signup_emails(signup_email=signup_email, context=context),
    )
