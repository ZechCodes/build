"""The invite email. One message, composed through the shared layout, carrying the link
as the message's action so a mail client shows a button rather than a bare URL.

No unsubscribe link and no List-Unsubscribe header: an invite is a door key sent to an
address that asked for one, not a mailing anybody is subscribed to."""

from __future__ import annotations

from litestar.background_tasks import BackgroundTask
from skrift.lib.email_backends import EmailBackend

from buildapp import releases
from buildapp.email_message import OutboundEmail, compose_email, deliver_emails
from buildapp.email_template import LAPTOP_IMAGE, EmailAction, EmailStep, EmailSteps

#: The install command and docs link name the public site, as the docs page does: the
#: invitee installs from getbuild.ing whichever deployment sent the mail.
PUBLIC_SITE_URL = "https://getbuild.ing"
DOCS_URL = f"{PUBLIC_SITE_URL}/docs"

INVITE_SUBJECT = "You’re in: your Build invite"
INVITE_HEADING = "You’re in."
INVITE_PARAGRAPHS = ("Thanks for waiting. Your spot in the Build alpha is ready.",)
INVITE_ACTION_LABEL = "Create your account"
INVITE_LINK_NOTE = "This link is just for you ({email}). It works once, for the next 14 days."
GETTING_STARTED = EmailSteps(
    title="Getting started",
    steps=(
        EmailStep(
            text="Install the bridge on the machine where your code lives:",
            command=releases.install_command(PUBLIC_SITE_URL),
            detail=(
                "There’s also a desktop app. It’s optional, and its installer is at "
                f"{DOCS_URL}."
            ),
        ),
        EmailStep(text="In Build, enter the pairing code the bridge prints."),
        EmailStep(
            text="Check that the fingerprint matches the one the bridge printed, "
            "then approve."
        ),
    ),
)
INVITE_CLOSING = (
    "That’s it. Your agents run on your machine from there.",
    "Stuck, or something broke? Reply to this email. It comes straight to me.",
    "— Zech",
)


def build_invite_email(*, to: str, invite_url: str, public_base_url: str) -> OutboundEmail:
    """Welcome, the app on a laptop, the one button, whose link it is, then how to get
    from an account to a paired machine — the same three steps the app's first-run
    screen walks."""
    return compose_email(
        to=to,
        subject=INVITE_SUBJECT,
        heading=INVITE_HEADING,
        paragraphs=INVITE_PARAGRAPHS,
        unsubscribe_url=None,
        one_click=False,
        public_base_url=public_base_url,
        hero=LAPTOP_IMAGE,
        action=EmailAction(url=invite_url, label=INVITE_ACTION_LABEL),
        after_action=(
            INVITE_LINK_NOTE.format(email=to),
            GETTING_STARTED,
            *INVITE_CLOSING,
        ),
    )


async def send_invite_email(
    email_backend: EmailBackend, to: str, invite_url: str, public_base_url: str
) -> bool:
    """Send the invite and answer whether it went out. Fail-soft, like every other
    send: the row is committed, so a dead SMTP server costs a log line and a False —
    the admin pages turn that into an error the operator can act on (Resend)."""
    message = build_invite_email(
        to=to, invite_url=invite_url, public_base_url=public_base_url
    )
    return await deliver_emails(email_backend, (message,))


def invite_email_task(
    email_backend: EmailBackend, to: str, invite_url: str, public_base_url: str
) -> BackgroundTask:
    """The same send, deferred until after the response has gone out — for the JSON
    route, whose caller is a script that should not wait on SMTP."""
    return BackgroundTask(
        send_invite_email, email_backend, to, invite_url, public_base_url
    )
