"""Tests pinning the one email layout: a single table capped at 560px, the accent used
once for the wordmark, every slot escaped, and nothing loaded from the network."""

from __future__ import annotations

from buildapp.email_template import (
    EMAIL_ACCENT,
    EMAIL_FONT_STACK,
    EMAIL_MAX_WIDTH_PX,
    UNSUBSCRIBE_LINK_LABEL,
    WORDMARK,
    render_email_html,
    render_email_text,
)

HEADING = "You’re on the list."
PARAGRAPHS = ("First paragraph of the message.", "Second paragraph of the message.")
UNSUBSCRIBE_URL = "https://getbuild.ing/waitlist/unsubscribe/token-value"


def _html() -> str:
    return render_email_html(
        heading=HEADING, paragraphs=PARAGRAPHS, unsubscribe_url=UNSUBSCRIBE_URL
    )


def _text() -> str:
    return render_email_text(
        heading=HEADING, paragraphs=PARAGRAPHS, unsubscribe_url=UNSUBSCRIBE_URL
    )


def test_html_is_one_table_layout_capped_at_560px():
    html = _html()
    assert f"max-width:{EMAIL_MAX_WIDTH_PX}px" in html
    assert "max-width:560px" in html
    assert "<table" in html
    assert "<img" not in html
    assert "gradient" not in html


def test_html_uses_the_accent_exactly_once_for_the_wordmark():
    html = _html()
    assert html.count(EMAIL_ACCENT) == 1
    assert WORDMARK in html


def test_html_and_text_carry_every_paragraph_and_the_unsubscribe_url():
    html = _html()
    text = _text()
    for body in (html, text):
        assert HEADING in body
        for paragraph in PARAGRAPHS:
            assert paragraph in body
        assert UNSUBSCRIBE_URL in body
    assert WORDMARK in text


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
    text = _text()
    unsubscribe_line = f"{UNSUBSCRIBE_LINK_LABEL}: {UNSUBSCRIBE_URL}"
    assert text.endswith(unsubscribe_line)
    assert text.splitlines()[-1] == unsubscribe_line


def test_html_declares_no_external_resource_and_no_custom_property():
    html = _html()
    assert html.count("http") == 1
    assert "var(" not in html


def test_font_stack_names_real_fallbacks():
    for family in ("ui-monospace", "SFMono-Regular", "Menlo", "Consolas", "monospace"):
        assert family in EMAIL_FONT_STACK
    assert EMAIL_FONT_STACK in _html()
