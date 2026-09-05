"""Tests for the page shell: one slot-substitution primitive and one document every
public HTML page renders through, so the landing page and the unsubscribe pages share a
head, a stylesheet and a footer instead of each declaring their own."""

from __future__ import annotations

from buildapp.landing_page import (
    PANEL_NAME,
    SHELL_NAME,
    fill_slots,
    read_landing_file,
    render_panel_page,
    render_shell,
)
from buildapp.test_root_landing import (
    FONT_PRELOAD_LINK,
    FOOTER_ASSURANCE_COPY,
    FOOTER_COPYRIGHT_COPY,
    STYLESHEET_LINK,
)

PAGE_TITLE = "Unsubscribe — Build"
PAGE_DESCRIPTION = "A short description of the page."
PAGE_BODY = '<main class="panel">Body content</main>'
PAGE_SCRIPTS = '<script type="module" src="/landing/main.js"></script>'
PANEL_HEADING = "A heading."
PANEL_MESSAGE = "One line of copy."
PANEL_ACTION = '<a href="/">home</a>'


def _rendered_shell(**overrides) -> str:
    slots = {
        "title": PAGE_TITLE,
        "description": PAGE_DESCRIPTION,
        "body": PAGE_BODY,
        "scripts": PAGE_SCRIPTS,
    }
    return render_shell(**{**slots, **overrides})


def test_fill_slots_replaces_every_occurrence_of_each_slot():
    filled = fill_slots(
        "{{first}} and {{second}} and {{first}}",
        {"first": "one", "second": "two"},
    )
    assert filled == "one and two and one"


def test_fill_slots_never_expands_a_placeholder_carried_in_a_value():
    assert fill_slots("{{first}}{{second}}", {"first": "{{second}}", "second": "B"}) == "{{second}}B"


def test_fill_slots_leaves_unknown_braces_alone():
    assert (
        fill_slots("{{known}} {{unknown}}", {"known": "filled"}) == "filled {{unknown}}"
    )


def test_render_shell_places_title_description_body_and_scripts():
    html = _rendered_shell()
    assert f"<title>{PAGE_TITLE}</title>" in html
    assert f'<meta name="description" content="{PAGE_DESCRIPTION}">' in html
    assert PAGE_BODY in html
    assert PAGE_SCRIPTS in html
    assert "{{" not in html


def test_render_shell_always_emits_the_footer_bar():
    html = _rendered_shell()
    assert FOOTER_ASSURANCE_COPY in html
    assert FOOTER_COPYRIGHT_COPY in html


def test_render_shell_with_empty_scripts_emits_no_script_element():
    assert "<script" not in _rendered_shell(scripts="")


def test_render_shell_links_the_stylesheet_and_preloads_the_font():
    html = _rendered_shell()
    assert STYLESHEET_LINK in html
    assert FONT_PRELOAD_LINK in html


def test_read_landing_file_reads_the_shell_document():
    shell = read_landing_file(SHELL_NAME)
    assert shell.startswith("<!doctype html>")
    assert shell.rstrip().endswith("</html>")


def test_render_panel_page_puts_one_panel_of_copy_in_the_shell():
    html = render_panel_page(
        title=PAGE_TITLE,
        heading=PANEL_HEADING,
        message=PANEL_MESSAGE,
        action=PANEL_ACTION,
    )
    assert f"<title>{PAGE_TITLE}</title>" in html
    for part in (PANEL_HEADING, PANEL_MESSAGE, PANEL_ACTION):
        assert part in html
    assert "{{" not in html


def test_the_panel_is_one_fragment_shared_by_every_short_public_page():
    assert PANEL_NAME == "panel.html"
    assert "{{heading}}" in read_landing_file(PANEL_NAME)


def test_a_panel_page_carries_no_description_and_no_script():
    html = render_panel_page(
        title=PAGE_TITLE, heading=PANEL_HEADING, message=PANEL_MESSAGE, action=""
    )
    assert '<meta name="description" content="">' in html
    assert "<script" not in html
