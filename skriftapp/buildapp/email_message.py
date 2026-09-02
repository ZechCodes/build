"""The outbound message value and the one sender every Build email goes through: compose
renders both bodies from the shared layout, and send hands exactly those fields to the
configured Skrift email backend. A message with no unsubscribe link carries no
List-Unsubscribe header and no footer."""

from __future__ import annotations

from dataclasses import dataclass

from skrift.lib.email_backends import EmailBackend

from buildapp.email_template import render_email_html, render_email_text

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
) -> OutboundEmail:
    return OutboundEmail(
        to=to,
        subject=subject,
        text_body=render_email_text(
            heading=heading, paragraphs=paragraphs, unsubscribe_url=unsubscribe_url
        ),
        html_body=render_email_html(
            heading=heading, paragraphs=paragraphs, unsubscribe_url=unsubscribe_url
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
