"""Tests for the three unsubscribe pages: the confirm page asks before it mutates and
posts back to the same token path, the removed and invalid pages carry no form at all,
every page renders through the landing shell so it looks like the rest of the site, and
the panel's markup lives in the landing directory rather than in Python."""

from __future__ import annotations

import re
from pathlib import Path

from buildapp import unsubscribe_pages
from buildapp.test_root_landing import (
    FOOTER_ASSURANCE_COPY,
    FOOTER_COPYRIGHT_COPY,
    STYLESHEET_LINK,
)
from buildapp.landing_page import HOME_LINK, HOME_LINK_LABEL
from buildapp.unsubscribe_pages import (
    CONFIRM_BUTTON_LABEL,
    CONFIRM_HEADING_TEMPLATE,
    CONFIRM_MESSAGE,
    CONFIRM_TITLE,
    INVALID_HEADING,
    INVALID_MESSAGE,
    INVALID_TITLE,
    REMOVED_HEADING,
    REMOVED_MESSAGE_TEMPLATE,
    REMOVED_TITLE,
    render_confirm_page,
    render_invalid_page,
    render_removed_page,
)
from buildapp.waitlist_unsubscribe_token import unsubscribe_path

SIGNER_ADDRESS = "alice@example.com"
TOKEN = "a-signed-token"
FORM_TAG_PATTERN = r"<form([^>]*)>"
ACTION_PATTERN = r'action="([^"]*)"'
BUTTON_MARKUP = f'<button class="button-primary" type="submit">{CONFIRM_BUTTON_LABEL}</button>'
ADDRESS_MARKUP = (
    f'<span class="title-accent unsubscribe-address">{SIGNER_ADDRESS}</span>'
)
HOME_LINK_MARKUP = HOME_LINK.render()
MODULE_DOCSTRING_DELIMITER = '"""'
UNSUBSCRIBE_PAGES_SOURCE_PATH = Path(unsubscribe_pages.__file__)
MARKUP_INJECTION = "<b>"
ESCAPED_INJECTION = "&lt;b&gt;"


def _every_page() -> tuple[str, ...]:
    return (
        render_confirm_page(SIGNER_ADDRESS, TOKEN),
        render_removed_page(),
        render_invalid_page(),
    )


def test_confirm_page_names_the_address_in_the_heading():
    html = render_confirm_page(SIGNER_ADDRESS, TOKEN)
    assert CONFIRM_HEADING_TEMPLATE.format(address=ADDRESS_MARKUP) in html
    assert ADDRESS_MARKUP in html


def test_confirm_page_has_one_post_form_targeting_the_token_path():
    html = render_confirm_page(SIGNER_ADDRESS, TOKEN)
    form_tags = re.findall(FORM_TAG_PATTERN, html)
    assert len(form_tags) == 1
    assert html.count("<form") == 1
    assert 'method="post"' in form_tags[0]
    assert re.findall(ACTION_PATTERN, form_tags[0]) == [unsubscribe_path(TOKEN)]


def test_confirm_page_has_one_remove_me_button():
    html = render_confirm_page(SIGNER_ADDRESS, TOKEN)
    assert html.count("<button") == 1
    assert BUTTON_MARKUP in html
    assert CONFIRM_BUTTON_LABEL == "REMOVE ME"


def test_the_panel_markup_lives_in_the_landing_directory_not_in_python():
    source_after_the_docstring = UNSUBSCRIBE_PAGES_SOURCE_PATH.read_text().split(
        MODULE_DOCSTRING_DELIMITER, 2
    )[2]
    assert "<" not in source_after_the_docstring
    assert "class=" not in source_after_the_docstring


def test_confirm_page_escapes_markup_in_the_address():
    html = render_confirm_page(f"{MARKUP_INJECTION}{SIGNER_ADDRESS}", TOKEN)
    assert MARKUP_INJECTION not in html
    assert ESCAPED_INJECTION in html


def test_confirm_page_escapes_markup_in_the_token():
    html = render_confirm_page(SIGNER_ADDRESS, f"{MARKUP_INJECTION}{TOKEN}")
    assert MARKUP_INJECTION not in html
    assert ESCAPED_INJECTION in html


def test_removed_and_invalid_pages_contain_no_form():
    for html in (render_removed_page(), render_invalid_page()):
        assert "<form" not in html
        assert "<button" not in html


def test_removed_page_links_home():
    assert HOME_LINK_MARKUP in render_removed_page()


def test_every_page_links_the_landing_stylesheet_and_carries_the_footer():
    for html in _every_page():
        assert STYLESHEET_LINK in html
        assert FOOTER_ASSURANCE_COPY in html
        assert FOOTER_COPYRIGHT_COPY in html


def test_every_page_uses_no_absolute_url():
    for html in _every_page():
        assert "http://" not in html
        assert "https://" not in html


def test_copy_is_verbatim():
    assert CONFIRM_TITLE == "Unsubscribe — Build"
    assert CONFIRM_HEADING_TEMPLATE == "Remove {address} from the list?"
    assert CONFIRM_MESSAGE == (
        "You’ll stop getting Build email. You can join again any time."
    )
    assert HOME_LINK_LABEL == "home page"
    assert REMOVED_TITLE == "Unsubscribed — Build"
    assert REMOVED_HEADING == "You’re off the list."
    assert REMOVED_MESSAGE_TEMPLATE == (
        "Nothing else will arrive. Changed your mind? Join again from the {home_link}."
    )
    assert INVALID_TITLE == "Link expired — Build"
    assert INVALID_HEADING == "This link is no longer valid."
    assert INVALID_MESSAGE == (
        "Links expire. Use the unsubscribe link in the most recent email you have "
        "from us."
    )
    confirm_html = render_confirm_page(SIGNER_ADDRESS, TOKEN)
    assert f"<title>{CONFIRM_TITLE}</title>" in confirm_html
    assert CONFIRM_MESSAGE in confirm_html
    removed_html = render_removed_page()
    assert f"<title>{REMOVED_TITLE}</title>" in removed_html
    assert REMOVED_HEADING in removed_html
    assert REMOVED_MESSAGE_TEMPLATE.format(home_link=HOME_LINK_MARKUP) in removed_html
    invalid_html = render_invalid_page()
    assert f"<title>{INVALID_TITLE}</title>" in invalid_html
    assert INVALID_HEADING in invalid_html
    assert INVALID_MESSAGE in invalid_html


def test_pages_carry_no_script():
    for html in _every_page():
        assert "<script" not in html
