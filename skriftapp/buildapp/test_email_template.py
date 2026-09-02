"""Tests pinning the one email layout: a single table capped at 560px in every client
including Outlook, the accent used once for the wordmark, every slot escaped, nothing
loaded from the network, and no footer at all on a message that carries no unsubscribe."""

from __future__ import annotations

import re

from buildapp.email_template import (
    EMAIL_ACCENT,
    EMAIL_BODY_FONT_SIZE_PX,
    EMAIL_BODY_LINE_HEIGHT,
    EMAIL_FONT_STACK,
    EMAIL_FOOTER_FONT_SIZE_PX,
    EMAIL_FOOTER_PADDING_TOP_PX,
    EMAIL_HAIRLINE_WIDTH_PX,
    EMAIL_HEADING_FONT_SIZE_PX,
    EMAIL_HEADING_PADDING_BOTTOM_PX,
    EMAIL_MAX_WIDTH_PX,
    EMAIL_OUTER_PADDING,
    EMAIL_PARAGRAPH_SPACING_PX,
    EMAIL_WORDMARK_FONT_SIZE_PX,
    EMAIL_WORDMARK_LETTER_SPACING,
    EMAIL_WORDMARK_PADDING_BOTTOM_PX,
    FOOTER_PREFIX,
    OUTLOOK_LAYOUT_CLOSER,
    OUTLOOK_LAYOUT_OPENER,
    UNSUBSCRIBE_LINK_LABEL,
    WORDMARK,
    render_email_html,
    render_email_text,
)
from buildapp.email_test_support import UNSUBSCRIBE_URL

HEADING = "You’re on the list."
PARAGRAPHS = ("First paragraph of the message.", "Second paragraph of the message.")
SIZE_LITERAL_PATTERN = r"(\d+(?:\.\d+)?)px"


def render_html_body(unsubscribe_url: str | None = UNSUBSCRIBE_URL) -> str:
    return render_email_html(
        heading=HEADING, paragraphs=PARAGRAPHS, unsubscribe_url=unsubscribe_url
    )


def render_text_body(unsubscribe_url: str | None = UNSUBSCRIBE_URL) -> str:
    return render_email_text(
        heading=HEADING, paragraphs=PARAGRAPHS, unsubscribe_url=unsubscribe_url
    )


def test_html_is_one_table_layout_capped_at_560px():
    html = render_html_body()
    assert EMAIL_MAX_WIDTH_PX == 560
    assert f"max-width:{EMAIL_MAX_WIDTH_PX}px" in html
    assert "<table" in html
    assert "<img" not in html
    assert "gradient" not in html


def test_html_caps_the_width_for_outlook_which_ignores_max_width():
    html = render_html_body()
    assert OUTLOOK_LAYOUT_OPENER in html
    assert OUTLOOK_LAYOUT_CLOSER in html
    assert f'width="{EMAIL_MAX_WIDTH_PX}"' in OUTLOOK_LAYOUT_OPENER
    assert html.index(OUTLOOK_LAYOUT_OPENER) < html.index("max-width")
    assert html.index(OUTLOOK_LAYOUT_CLOSER) > html.index("max-width")


def test_every_size_in_the_layout_comes_from_a_named_constant():
    html = render_html_body()
    named_sizes = {
        str(EMAIL_MAX_WIDTH_PX),
        str(EMAIL_BODY_FONT_SIZE_PX),
        str(EMAIL_FOOTER_FONT_SIZE_PX),
        str(EMAIL_HEADING_FONT_SIZE_PX),
        str(EMAIL_WORDMARK_FONT_SIZE_PX),
        str(EMAIL_WORDMARK_PADDING_BOTTOM_PX),
        str(EMAIL_HEADING_PADDING_BOTTOM_PX),
        str(EMAIL_FOOTER_PADDING_TOP_PX),
        str(EMAIL_PARAGRAPH_SPACING_PX),
        str(EMAIL_HAIRLINE_WIDTH_PX),
        *re.findall(SIZE_LITERAL_PATTERN, EMAIL_OUTER_PADDING),
    }
    assert set(re.findall(SIZE_LITERAL_PATTERN, html)) <= named_sizes
    assert EMAIL_OUTER_PADDING in html
    assert f"line-height:{EMAIL_BODY_LINE_HEIGHT}" in html
    assert f"letter-spacing:{EMAIL_WORDMARK_LETTER_SPACING}" in html


def test_html_uses_the_accent_exactly_once_for_the_wordmark():
    html = render_html_body()
    assert html.count(EMAIL_ACCENT) == 1
    assert WORDMARK in html


def test_html_and_text_carry_every_paragraph_and_the_unsubscribe_url():
    for body in (render_html_body(), render_text_body()):
        assert HEADING in body
        for paragraph in PARAGRAPHS:
            assert paragraph in body
        assert UNSUBSCRIBE_URL in body
    assert WORDMARK in render_text_body()


def test_a_message_without_an_unsubscribe_url_carries_no_footer_line():
    html = render_html_body(None)
    text = render_text_body(None)
    for body in (html, text):
        assert UNSUBSCRIBE_LINK_LABEL not in body
        assert FOOTER_PREFIX not in body
        assert HEADING in body
    assert "<a " not in html
    assert "http" not in html
    assert text.endswith(PARAGRAPHS[-1])


def test_html_escapes_markup_in_every_slot():
    html = render_email_html(
        heading="<script>alert(1)</script>",
        paragraphs=("<script>alert(2)</script>",),
        unsubscribe_url='https://getbuild.ing/u/"onmouseover="alert(3)',
    )
    assert "<script" not in html
    assert html.count("&lt;script&gt;") == 2
    assert '"onmouseover="' not in html
    assert "&quot;onmouseover=&quot;" in html


def test_text_body_ends_with_the_unsubscribe_line():
    text = render_text_body()
    unsubscribe_line = f"{UNSUBSCRIBE_LINK_LABEL}: {UNSUBSCRIBE_URL}"
    assert text.endswith(unsubscribe_line)
    assert text.splitlines()[-1] == unsubscribe_line


def test_html_declares_no_external_resource_and_no_custom_property():
    html = render_html_body()
    assert html.count("http") == 1
    assert "var(" not in html


def test_font_stack_names_real_fallbacks():
    for family in ("ui-monospace", "SFMono-Regular", "Menlo", "Consolas", "monospace"):
        assert family in EMAIL_FONT_STACK
    assert EMAIL_FONT_STACK in render_html_body()
