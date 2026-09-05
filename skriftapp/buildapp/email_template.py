"""The one outbound email layout. Every colour and size is a literal here rather than a
landing.css custom property because custom properties, rgba() and max-width do not
survive Outlook and Gmail."""

from __future__ import annotations

from html import escape

EMAIL_MAX_WIDTH_PX = 560
EMAIL_BACKGROUND = "#030604"
EMAIL_ACCENT = "#00ff88"
EMAIL_TEXT_PRIMARY = "#f2fff8"
EMAIL_TEXT_SECONDARY = "#8fa89a"
EMAIL_TEXT_MUTED = "#54655c"
EMAIL_HAIRLINE = "#171a18"
EMAIL_HAIRLINE_WIDTH_PX = 1
EMAIL_FONT_STACK = (
    "'JetBrains Mono', ui-monospace, SFMono-Regular, Menlo, Consolas, monospace"
)
EMAIL_OUTER_PADDING = "40px 16px"
EMAIL_WORDMARK_FONT_SIZE_PX = 16
EMAIL_WORDMARK_LETTER_SPACING = ".08em"
EMAIL_WORDMARK_PADDING_BOTTOM_PX = 28
EMAIL_HEADING_FONT_SIZE_PX = 22
EMAIL_HEADING_LINE_HEIGHT = "1.35"
EMAIL_HEADING_PADDING_BOTTOM_PX = 20
EMAIL_BODY_FONT_SIZE_PX = 14
EMAIL_BODY_LINE_HEIGHT = "1.7"
EMAIL_PARAGRAPH_SPACING_PX = 16
EMAIL_FOOTER_FONT_SIZE_PX = 12
EMAIL_FOOTER_LINE_HEIGHT = "1.6"
EMAIL_FOOTER_PADDING_TOP_PX = 24

OUTLOOK_LAYOUT_OPENER = (
    f'<!--[if mso]><table role="presentation" width="{EMAIL_MAX_WIDTH_PX}" '
    'cellpadding="0" cellspacing="0" border="0" align="center"><tr><td><![endif]-->'
)
OUTLOOK_LAYOUT_CLOSER = "<!--[if mso]></td></tr></table><![endif]-->"

EMAIL_ACTION_PADDING_BOTTOM_PX = 24
EMAIL_ACTION_PADDING = "12px 20px"

WORDMARK = "build_"
FOOTER_PREFIX = "Not you?"
UNSUBSCRIBE_LINK_LABEL = "Unsubscribe"


def render_action_row(action_url: str, action_label: str) -> str:
    """The optional call-to-action, in the landing's button style. Inline colours and a
    padded anchor rather than a real button: Outlook and Gmail drop both."""
    return (
        f'<tr><td style="padding-bottom:{EMAIL_ACTION_PADDING_BOTTOM_PX}px">'
        f'<a href="{escape(action_url)}" '
        f'style="display:inline-block;padding:{EMAIL_ACTION_PADDING};'
        f"background:{EMAIL_ACCENT};color:{EMAIL_BACKGROUND};"
        f"font-size:{EMAIL_BODY_FONT_SIZE_PX}px;"
        f"letter-spacing:{EMAIL_WORDMARK_LETTER_SPACING};"
        f'text-decoration:none">{escape(action_label)}</a>'
        "</td></tr>"
    )


def render_footer_row(unsubscribe_url: str) -> str:
    return (
        f'<tr><td style="border-top:{EMAIL_HAIRLINE_WIDTH_PX}px solid {EMAIL_HAIRLINE};'
        f'padding-top:{EMAIL_FOOTER_PADDING_TOP_PX}px">'
        f'<p style="margin:0;color:{EMAIL_TEXT_MUTED};'
        f"font-size:{EMAIL_FOOTER_FONT_SIZE_PX}px;"
        f'line-height:{EMAIL_FOOTER_LINE_HEIGHT}">'
        f'{FOOTER_PREFIX} <a href="{escape(unsubscribe_url)}" '
        f'style="color:{EMAIL_TEXT_SECONDARY};text-decoration:underline">'
        f"{UNSUBSCRIBE_LINK_LABEL}</a></p>"
        "</td></tr>"
    )


def _action_markup(action_url: str | None, action_label: str | None) -> str:
    """An action needs both halves — a URL with nothing to call it is not a button."""
    if action_url is None or action_label is None:
        return ""
    return render_action_row(action_url, action_label)


def render_email_html(
    *,
    heading: str,
    paragraphs: tuple[str, ...],
    unsubscribe_url: str | None,
    action_url: str | None = None,
    action_label: str | None = None,
) -> str:
    paragraph_markup = "".join(
        f'<p style="margin:0 0 {EMAIL_PARAGRAPH_SPACING_PX}px 0;'
        f"color:{EMAIL_TEXT_SECONDARY};font-size:{EMAIL_BODY_FONT_SIZE_PX}px;"
        f'line-height:{EMAIL_BODY_LINE_HEIGHT}">{escape(paragraph)}</p>'
        for paragraph in paragraphs
    )
    footer_markup = (
        "" if unsubscribe_url is None else render_footer_row(unsubscribe_url)
    )
    return (
        "<!doctype html>"
        '<html lang="en"><head><meta charset="utf-8">'
        '<meta name="viewport" content="width=device-width,initial-scale=1">'
        "</head>"
        f'<body style="margin:0;background:{EMAIL_BACKGROUND}" bgcolor="{EMAIL_BACKGROUND}">'
        '<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" '
        f'style="background:{EMAIL_BACKGROUND};padding:{EMAIL_OUTER_PADDING}">'
        '<tr><td align="center">'
        f"{OUTLOOK_LAYOUT_OPENER}"
        '<table role="presentation" width="100%" cellpadding="0" cellspacing="0" border="0" '
        f'style="max-width:{EMAIL_MAX_WIDTH_PX}px;text-align:left;'
        f'font-family:{EMAIL_FONT_STACK}">'
        f'<tr><td style="padding-bottom:{EMAIL_WORDMARK_PADDING_BOTTOM_PX}px;'
        f"color:{EMAIL_ACCENT};font-size:{EMAIL_WORDMARK_FONT_SIZE_PX}px;"
        f'letter-spacing:{EMAIL_WORDMARK_LETTER_SPACING}">{WORDMARK}</td></tr>'
        f'<tr><td style="padding-bottom:{EMAIL_HEADING_PADDING_BOTTOM_PX}px;'
        f"color:{EMAIL_TEXT_PRIMARY};font-size:{EMAIL_HEADING_FONT_SIZE_PX}px;"
        f'line-height:{EMAIL_HEADING_LINE_HEIGHT}">{escape(heading)}</td></tr>'
        f"<tr><td>{paragraph_markup}</td></tr>"
        f"{_action_markup(action_url, action_label)}"
        f"{footer_markup}"
        "</table>"
        f"{OUTLOOK_LAYOUT_CLOSER}"
        "</td></tr></table>"
        "</body></html>"
    )


def render_email_text(
    *,
    heading: str,
    paragraphs: tuple[str, ...],
    unsubscribe_url: str | None,
    action_url: str | None = None,
    action_label: str | None = None,
) -> str:
    action_lines = (
        ()
        if action_url is None or action_label is None
        else (f"{action_label}: {action_url}",)
    )
    footer_lines = (
        ()
        if unsubscribe_url is None
        else (f"{UNSUBSCRIBE_LINK_LABEL}: {unsubscribe_url}",)
    )
    return "\n\n".join(
        (WORDMARK, heading, *paragraphs, *action_lines, *footer_lines)
    )
