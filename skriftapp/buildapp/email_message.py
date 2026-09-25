"""The outbound message value and the one sender every Build email goes through: compose
renders both bodies from the shared layout, and send hands exactly those fields to the
configured Skrift email backend. A message with no unsubscribe link carries no
List-Unsubscribe header and no footer.

Also the shared plumbing every mailing route needs: where this deployment lives
(``resolve_public_base_url``), where the backend comes from (``provide_email_backend``),
and the fail-soft delivery loop background tasks run (``deliver_emails``) — one home
each, because the waitlist and the invites both send mail."""

from __future__ import annotations

import logging
from dataclasses import dataclass

from litestar import Request
from skrift.config import Settings, get_settings
from skrift.lib.email_backends import EmailBackend

from buildapp.email_template import (
    EmailAction,
    EmailBlock,
    EmailImage,
    render_email_html,
    render_email_text,
)

logger = logging.getLogger(__name__)

LIST_UNSUBSCRIBE_HEADER = "List-Unsubscribe"
LIST_UNSUBSCRIBE_POST_HEADER = "List-Unsubscribe-Post"
ONE_CLICK_HEADER_VALUE = "List-Unsubscribe=One-Click"


@dataclass(frozen=True)
class OutboundEmail:
    to: str
    subject: str
    text_body: str
    html_body: str
    headers: dict[str, str]


def list_unsubscribe_headers(
    unsubscribe_url: str | None, *, one_click: bool
) -> dict[str, str]:
    if unsubscribe_url is None:
        return {}
    headers = {LIST_UNSUBSCRIBE_HEADER: f"<{unsubscribe_url}>"}
    if one_click:
        headers[LIST_UNSUBSCRIBE_POST_HEADER] = ONE_CLICK_HEADER_VALUE
    return headers


def compose_email(
    *,
    to: str,
    subject: str,
    heading: str,
    paragraphs: tuple[str, ...],
    unsubscribe_url: str | None,
    one_click: bool,
    public_base_url: str,
    action: EmailAction | None = None,
    after_action: tuple[EmailBlock, ...] = (),
    hero: EmailImage | None = None,
) -> OutboundEmail:
    """``public_base_url`` is where the HTML part's images are fetched from; the text
    part has none."""
    body = {
        "heading": heading,
        "paragraphs": paragraphs,
        "unsubscribe_url": unsubscribe_url,
        "action": action,
        "after_action": after_action,
    }
    return OutboundEmail(
        to=to,
        subject=subject,
        text_body=render_email_text(**body),
        html_body=render_email_html(
            **body, public_base_url=public_base_url, hero=hero
        ),
        headers=list_unsubscribe_headers(unsubscribe_url, one_click=one_click),
    )


async def send_email_message(email_backend: EmailBackend, message: OutboundEmail) -> None:
    await email_backend.send_email(
        to=message.to,
        subject=message.subject,
        text_body=message.text_body,
        html_body=message.html_body,
        headers=message.headers,
    )


async def deliver_emails(
    email_backend: EmailBackend, messages: tuple[OutboundEmail, ...]
) -> bool:
    """Send each message, logging rather than raising on failure, and answer whether
    every one went out.

    Deliberately fail-soft: this runs after the response has gone out and the row is
    committed, so a dead SMTP server must not lose the remaining messages — or surface
    as an error the visitor already got a 200 instead of. A caller still waiting on
    the send (an operator's form) reads the answer and says so."""
    delivered = True
    for message in messages:
        try:
            await send_email_message(email_backend, message)
        except Exception:
            logger.exception("email delivery failed for %s", message.to)
            delivered = False
    return delivered


def resolve_public_base_url(settings: Settings) -> str:
    """Where this deployment lives, as an origin with no trailing slash — the base of
    every link this app puts in an email or a redirect."""
    return (
        settings.email.public_base_url or settings.auth.redirect_base_url or ""
    ).rstrip("/")


def provide_email_backend(request: Request) -> EmailBackend:
    return request.app.state.email_backend


def provide_public_base_url() -> str:
    """This deployment's origin, as a dependency — so a handler that puts a link in a
    page, a payload or an email declares it and is handed it, rather than reaching for
    the settings itself."""
    return resolve_public_base_url(get_settings())
