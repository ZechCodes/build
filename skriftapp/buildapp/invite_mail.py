"""The invite email. One message, composed through the shared layout, carrying the link
as the message's action so a mail client shows a button rather than a bare URL.

No unsubscribe link and no List-Unsubscribe header: an invite is a door key sent to an
address that asked for one, not a mailing anybody is subscribed to."""

from __future__ import annotations

from litestar.background_tasks import BackgroundTask
from skrift.lib.email_backends import EmailBackend

from buildapp.email_message import OutboundEmail, compose_email, deliver_emails
from buildapp.email_template import EmailAction

INVITE_SUBJECT = "Your Build invite"
INVITE_HEADING = "You’re in."
INVITE_PARAGRAPHS = (
    "Build turns an issue into shipped code, on your machine.",
    "This invite is for {email} and works once, for the next 14 days.",
)
INVITE_ACTION_LABEL = "ACCEPT INVITE"


def build_invite_email(*, to: str, invite_url: str) -> OutboundEmail:
    return compose_email(
        to=to,
        subject=INVITE_SUBJECT,
        heading=INVITE_HEADING,
        paragraphs=tuple(
            paragraph.format(email=to) for paragraph in INVITE_PARAGRAPHS
        ),
        unsubscribe_url=None,
        one_click=False,
        action=EmailAction(url=invite_url, label=INVITE_ACTION_LABEL),
    )


def invite_email_task(
    email_backend: EmailBackend, to: str, invite_url: str
) -> BackgroundTask:
    """Deliver the invite after the response has gone out. Fail-soft, like every other
    send: the row is committed and the operator holds the URL, so a dead SMTP server
    costs a log line, not the invite."""
    return BackgroundTask(
        deliver_emails,
        email_backend,
        (build_invite_email(to=to, invite_url=invite_url),),
    )
