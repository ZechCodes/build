"""The one outbound email layout, mirroring the landing page's design tokens as hex
literals because custom properties and rgba() do not survive Outlook or Gmail:
EMAIL_BACKGROUND mirrors --color-background, EMAIL_ACCENT --color-accent,
EMAIL_TEXT_PRIMARY/SECONDARY/MUTED the three text tokens, and EMAIL_HAIRLINE the
rgba(255,255,255,.08) hairline composited over the ground."""

from __future__ import annotations

from html import escape

EMAIL_MAX_WIDTH_PX = 560
EMAIL_BACKGROUND = "#030604"
EMAIL_ACCENT = "#00ff88"
EMAIL_TEXT_PRIMARY = "#f2fff8"
EMAIL_TEXT_SECONDARY = "#8fa89a"
EMAIL_TEXT_MUTED = "#54655c"
EMAIL_HAIRLINE = "#171a18"
EMAIL_FONT_STACK = (
    "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
)
WORDMARK = "build_"
FOOTER_PREFIX = "Not you?"
UNSUBSCRIBE_LINK_LABEL = "Unsubscribe"


def render_email_html(
    *, heading: str, paragraphs: tuple[str, ...], unsubscribe_url: str
) -> str:
    paragraph_markup = "".join(
        f'<p style="margin:0 0 16px 0;color:{EMAIL_TEXT_SECONDARY};'
        f'font-size:14px;line-height:1.7">{escape(paragraph)}</p>'
        for paragraph in paragraphs
    )
    return (
        "<!doctype html>"
        '<html lang="en"><head><meta charset="utf-8">'
        '<meta name="viewport" content="width=device-width,initial-scale=1">'
        "</head>"
        f'<body style="margin:0;background:{EMAIL_BACKGROUND}" bgcolor="{EMAIL_BACKGROUND}">'
        '<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" '
        f'style="background:{EMAIL_BACKGROUND};padding:40px 16px">'
        '<tr><td align="center">'
        '<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" '
        f'style="max-width:{EMAIL_MAX_WIDTH_PX}px;text-align:left;font-family:{EMAIL_FONT_STACK}">'
        f'<tr><td style="padding-bottom:28px;color:{EMAIL_ACCENT};font-size:16px;'
        f'letter-spacing:.08em">{WORDMARK}</td></tr>'
        f'<tr><td style="padding-bottom:20px;color:{EMAIL_TEXT_PRIMARY};font-size:22px;'
        f'line-height:1.35">{escape(heading)}</td></tr>'
        f"<tr><td>{paragraph_markup}</td></tr>"
        f'<tr><td style="border-top:1px solid {EMAIL_HAIRLINE};padding-top:24px">'
        f'<p style="margin:0;color:{EMAIL_TEXT_MUTED};font-size:12px;line-height:1.6">'
        f'{FOOTER_PREFIX} <a href="{escape(unsubscribe_url)}" '
        f'style="color:{EMAIL_TEXT_SECONDARY};text-decoration:underline">'
        f"{UNSUBSCRIBE_LINK_LABEL}</a></p>"
        "</td></tr>"
        "</table>"
        "</td></tr></table>"
        "</body></html>"
    )


def render_email_text(
    *, heading: str, paragraphs: tuple[str, ...], unsubscribe_url: str
) -> str:
    blocks = (
        WORDMARK,
        heading,
        *paragraphs,
        f"{UNSUBSCRIBE_LINK_LABEL}: {unsubscribe_url}",
    )
    return "\n\n".join(blocks)
