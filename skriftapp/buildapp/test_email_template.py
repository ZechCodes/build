"""Tests pinning the one email layout: a single table capped at 560px in every client
including Outlook, the Build mark as the header, the accent used once (the mark's alt
text), every slot escaped, nothing loaded from anywhere but this deployment's own
email images, and no footer at all on a message that carries no unsubscribe."""

from __future__ import annotations

import re

import pytest

from buildapp.email_template import (
    EMAIL_ACCENT,
    EMAIL_ACTION_PADDING,
    EMAIL_ACTION_PADDING_BOTTOM_PX,
    EMAIL_COMMAND_FONT_SIZE_PX,
    EMAIL_COMMAND_PADDING,
    EMAIL_STEP_NUMBER_WIDTH_PX,
    EMAIL_STEPS_TITLE_PADDING_BOTTOM_PX,
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
    EMAIL_HERO_PADDING_BOTTOM_PX,
    EMAIL_MARK_PADDING_BOTTOM_PX,
    EMAIL_BRAND_FONT_SIZE_PX,
    EMAIL_BRAND_LETTER_SPACING,
    FOOTER_PREFIX,
    MARK_IMAGE,
    OUTLOOK_LAYOUT_CLOSER,
    OUTLOOK_LAYOUT_OPENER,
    PHONE_TABLET_IMAGE,
    TEXT_BRAND_LINE,
    UNSUBSCRIBE_LINK_LABEL,
    TEXT_STEP_INDENT,
    EmailAction,
    EmailImage,
    EmailStep,
    EmailSteps,
    render_email_text,
)
from buildapp.email_template import render_email_html as render_layout_html
from buildapp.email_test_support import PUBLIC_BASE_URL, UNSUBSCRIBE_URL

HEADING = "You’re on the list."
PARAGRAPHS = ("First paragraph of the message.", "Second paragraph of the message.")
SIZE_LITERAL_PATTERN = r"(\d+(?:\.\d+)?)px"
ACTION_URL = "https://getbuild.ing/invite/inv_token"
ACTION_LABEL = "ACCEPT INVITE"
ACTION = EmailAction(url=ACTION_URL, label=ACTION_LABEL)
MARK_URL = f"{PUBLIC_BASE_URL}/landing/email/brand-mark.png"
MARK_2X_URL = f"{PUBLIC_BASE_URL}/landing/email/brand-mark@2x.png"
HERO_URL = f"{PUBLIC_BASE_URL}/landing/email/phone-tablet.png"
#: Every px size an image adds: its width and height attributes, mirrored in its style.
IMAGE_SIZES = {
    str(size)
    for image in (MARK_IMAGE, PHONE_TABLET_IMAGE)
    for size in (image.width_px, image.height_px)
}


def render_email_html(**body) -> str:
    """The layout as every sender calls it: from this deployment's origin."""
    return render_layout_html(public_base_url=PUBLIC_BASE_URL, **body)


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
    assert html.count("<img") == 1
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
        str(EMAIL_BRAND_FONT_SIZE_PX),
        str(EMAIL_MARK_PADDING_BOTTOM_PX),
        str(EMAIL_HEADING_PADDING_BOTTOM_PX),
        str(EMAIL_FOOTER_PADDING_TOP_PX),
        str(EMAIL_PARAGRAPH_SPACING_PX),
        str(EMAIL_HAIRLINE_WIDTH_PX),
        *IMAGE_SIZES,
        *re.findall(SIZE_LITERAL_PATTERN, EMAIL_OUTER_PADDING),
    }
    assert set(re.findall(SIZE_LITERAL_PATTERN, html)) <= named_sizes
    assert EMAIL_OUTER_PADDING in html
    assert f"line-height:{EMAIL_BODY_LINE_HEIGHT}" in html
    assert f"letter-spacing:{EMAIL_BRAND_LETTER_SPACING}" in html


def test_html_uses_the_accent_exactly_once_for_the_marks_alt_text():
    html = render_html_body()
    assert html.count(EMAIL_ACCENT) == 1
    mark_tag = re.search(r"<img[^>]*>", html).group(0)
    assert f"color:{EMAIL_ACCENT}" in mark_tag


def test_the_header_is_the_build_mark_hosted_by_this_deployment():
    mark_tag = re.search(r"<img[^>]*>", render_html_body()).group(0)
    assert f'src="{MARK_2X_URL}"' in mark_tag
    assert f'srcset="{MARK_URL} 1x, {MARK_2X_URL} 2x"' in mark_tag
    assert 'alt="Build"' in mark_tag
    assert f'width="{MARK_IMAGE.width_px}"' in mark_tag
    assert f'height="{MARK_IMAGE.height_px}"' in mark_tag
    assert MARK_IMAGE.height_px == 32


def test_the_image_urls_follow_the_deployment_rather_than_a_fixed_host():
    html = render_layout_html(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=None,
        public_base_url="https://staging.example.test",
        hero=PHONE_TABLET_IMAGE,
    )
    assert 'src="https://staging.example.test/landing/email/brand-mark@2x.png"' in html
    assert 'src="https://staging.example.test/landing/email/phone-tablet.png"' in html
    assert PUBLIC_BASE_URL not in html


def test_a_base_url_with_a_trailing_slash_gives_no_double_slash():
    html = render_layout_html(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=None,
        public_base_url=f"{PUBLIC_BASE_URL}/",
        hero=PHONE_TABLET_IMAGE,
    )
    assert f'src="{MARK_2X_URL}"' in html
    assert f'src="{HERO_URL}"' in html
    assert f"{PUBLIC_BASE_URL}//" not in html


def test_no_underscore_wordmark_is_left_in_either_part():
    for body in (render_html_body(), render_text_body()):
        assert "build_" not in body.lower()
    assert render_text_body().split("\n\n")[0] == TEXT_BRAND_LINE == "Build"


def test_html_and_text_carry_every_paragraph_and_the_unsubscribe_url():
    for body in (render_html_body(), render_text_body()):
        assert HEADING in body
        for paragraph in PARAGRAPHS:
            assert paragraph in body
        assert UNSUBSCRIBE_URL in body


def test_a_hero_image_sits_under_the_copy_and_above_the_action():
    html = render_email_html(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=None,
        action=ACTION,
        hero=PHONE_TABLET_IMAGE,
    )
    hero_tag = re.search(rf'<img[^>]*src="{re.escape(HERO_URL)}"[^>]*>', html).group(0)
    assert 'alt="Build on a phone and a tablet"' in hero_tag
    assert f'width="{PHONE_TABLET_IMAGE.width_px}"' in hero_tag
    assert f'height="{PHONE_TABLET_IMAGE.height_px}"' in hero_tag
    assert PHONE_TABLET_IMAGE.width_px == EMAIL_MAX_WIDTH_PX
    assert html.index(PARAGRAPHS[-1]) < html.index(HERO_URL) < html.index(ACTION_URL)
    assert f"padding-bottom:{EMAIL_HERO_PADDING_BOTTOM_PX}px" in html


def test_a_message_without_a_hero_carries_only_the_mark():
    assert HERO_URL not in render_html_body()
    assert render_html_body() == render_email_html(
        heading=HEADING, paragraphs=PARAGRAPHS, unsubscribe_url=UNSUBSCRIBE_URL, hero=None
    )


def test_with_images_blocked_the_alt_texts_read_in_order():
    """What a client that blocks remote images shows: each image's alt text where the
    image would be, and the copy around them in reading order."""
    html = render_email_html(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=None,
        action=ACTION,
        hero=PHONE_TABLET_IMAGE,
    )
    blocked = re.sub(r'<img[^>]*alt="([^"]*)"[^>]*>', r"[\1]", html)
    positions = [
        blocked.index("[Build]"),
        blocked.index(HEADING),
        blocked.index(PARAGRAPHS[-1]),
        blocked.index("[Build on a phone and a tablet]"),
        blocked.index(ACTION_LABEL),
    ]
    assert positions == sorted(positions)


def test_an_image_escapes_its_url_and_alt_text():
    html = render_email_html(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=None,
        hero=EmailImage(file='x.png"onerror="alert(1)', width_px=1, height_px=1, alt="<b>"),
    )
    assert '"onerror="' not in html
    assert "<b>" not in html


def test_a_message_without_an_unsubscribe_url_carries_no_footer_line():
    html = render_html_body(None)
    text = render_text_body(None)
    for body in (html, text):
        assert UNSUBSCRIBE_LINK_LABEL not in body
        assert FOOTER_PREFIX not in body
        assert HEADING in body
    assert "<a " not in html
    assert set(re.findall(r'https?://[^"\s]+', html)) == {MARK_URL, MARK_2X_URL}
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


def test_html_loads_nothing_but_the_hosted_mark_and_declares_no_custom_property():
    html = render_html_body()
    assert set(re.findall(r'https?://[^"\s]+', html)) == {
        MARK_URL,
        MARK_2X_URL,
        UNSUBSCRIBE_URL,
    }
    assert "var(" not in html


def test_font_stack_names_real_fallbacks():
    for family in ("ui-monospace", "SFMono-Regular", "Menlo", "Consolas", "monospace"):
        assert family in EMAIL_FONT_STACK
    assert EMAIL_FONT_STACK in render_html_body()


def test_an_action_renders_as_a_button_in_html_and_a_labelled_line_in_text():
    html = render_email_html(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=None,
        action=ACTION,
    )
    text = render_email_text(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=None,
        action=ACTION,
    )
    assert f'href="{ACTION_URL}"' in html
    assert ACTION_LABEL in html
    assert text.splitlines()[-1] == f"{ACTION_LABEL}: {ACTION_URL}"


def test_the_action_row_sits_between_the_copy_and_any_footer():
    html = render_email_html(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=UNSUBSCRIBE_URL,
        action=ACTION,
    )
    assert html.index(PARAGRAPHS[-1]) < html.index(ACTION_URL) < html.index(FOOTER_PREFIX)


def test_the_action_row_escapes_both_its_url_and_its_label():
    html = render_email_html(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=None,
        action=EmailAction(
            url='https://getbuild.ing/invite/"onmouseover="alert(1)',
            label="<script>alert(2)</script>",
        ),
    )
    assert "<script" not in html
    assert '"onmouseover="' not in html


def test_a_message_with_no_action_renders_exactly_as_it_did_before():
    assert render_html_body() == render_email_html(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=UNSUBSCRIBE_URL,
        action=None,
    )
    assert render_text_body() == render_email_text(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=UNSUBSCRIBE_URL,
        action=None,
    )
    assert ACTION_URL not in render_html_body()


def test_an_action_is_one_value_so_neither_half_can_go_missing():
    with pytest.raises(TypeError):
        EmailAction(url=ACTION_URL)


STEPS = EmailSteps(
    title="Getting started",
    steps=(
        EmailStep(text="Install it:", command='curl -fsSL "https://x/i.sh" | sh', detail="Or not."),
        EmailStep(text="Pair it."),
    ),
)
AFTER_ACTION = ("A note after the button.", STEPS, "Sign-off.")


def render_with_steps(renderer):
    return renderer(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=None,
        action=ACTION,
        after_action=AFTER_ACTION,
    )


def test_what_follows_the_action_renders_after_it_in_both_parts():
    html = render_with_steps(render_email_html)
    text = render_with_steps(render_email_text)
    assert html.index(ACTION_URL) < html.index("A note after the button.")
    assert html.index("Getting started") < html.index("Pair it.") < html.index("Sign-off.")
    assert text.split("\n\n")[-7:] == [
        "A note after the button.",
        "Getting started",
        "1. Install it:",
        f'{TEXT_STEP_INDENT}curl -fsSL "https://x/i.sh" | sh',
        f"{TEXT_STEP_INDENT}Or not.",
        "2. Pair it.",
        "Sign-off.",
    ]


def test_steps_are_numbered_cells_and_the_command_is_its_own_code_block():
    html = render_with_steps(render_email_html)
    assert "<ol" not in html
    assert ">1.</td>" in html and ">2.</td>" in html
    assert "<code>curl -fsSL &quot;https://x/i.sh&quot; | sh</code>" in html


def test_every_size_the_steps_add_comes_from_a_named_constant():
    html = render_with_steps(render_email_html)
    named_sizes = {
        str(EMAIL_MAX_WIDTH_PX),
        str(EMAIL_BODY_FONT_SIZE_PX),
        str(EMAIL_HEADING_FONT_SIZE_PX),
        str(EMAIL_BRAND_FONT_SIZE_PX),
        str(EMAIL_MARK_PADDING_BOTTOM_PX),
        str(EMAIL_HEADING_PADDING_BOTTOM_PX),
        str(EMAIL_PARAGRAPH_SPACING_PX),
        str(EMAIL_ACTION_PADDING_BOTTOM_PX),
        *IMAGE_SIZES,
        str(EMAIL_STEPS_TITLE_PADDING_BOTTOM_PX),
        str(EMAIL_STEP_NUMBER_WIDTH_PX),
        str(EMAIL_COMMAND_FONT_SIZE_PX),
        *re.findall(SIZE_LITERAL_PATTERN, EMAIL_OUTER_PADDING),
        *re.findall(SIZE_LITERAL_PATTERN, EMAIL_ACTION_PADDING),
        *re.findall(SIZE_LITERAL_PATTERN, EMAIL_COMMAND_PADDING),
    }
    assert set(re.findall(SIZE_LITERAL_PATTERN, html)) <= named_sizes


def test_steps_escape_every_slot():
    html = render_email_html(
        heading=HEADING,
        paragraphs=PARAGRAPHS,
        unsubscribe_url=None,
        after_action=(
            "<script>alert(1)</script>",
            EmailSteps(
                title="<script>alert(2)</script>",
                steps=(
                    EmailStep(
                        text="<script>alert(3)</script>",
                        command="<script>alert(4)</script>",
                        detail="<script>alert(5)</script>",
                    ),
                ),
            ),
        ),
    )
    assert "<script" not in html
    assert html.count("&lt;script&gt;") == 5


def test_nothing_after_the_action_renders_exactly_as_before():
    for renderer in (render_email_html, render_email_text):
        assert renderer(
            heading=HEADING, paragraphs=PARAGRAPHS, unsubscribe_url=None, action=ACTION
        ) == renderer(
            heading=HEADING,
            paragraphs=PARAGRAPHS,
            unsubscribe_url=None,
            action=ACTION,
            after_action=(),
        )
