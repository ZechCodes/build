"""Every invite outcome is a row in one table — status code and copy — not a branch in
a handler. These tests pin the five link outcomes plus the mismatch page, the
invite-only page /app/ shows a non-member, and that all of it renders through the same
landing panel the unsubscribe pages use."""

from __future__ import annotations

from pathlib import Path

from buildapp import invite_pages
from buildapp.invite_pages import (
    INVITE_ONLY_HEADING,
    INVITE_ONLY_TITLE,
    OUTCOMES,
    invite_only_outcome,
)
from buildapp.invites import EMAIL_MISMATCH, InviteState
from buildapp.landing_page import PANEL_NAME, read_landing_file
from buildapp.test_root_landing import (
    FOOTER_ASSURANCE_COPY,
    FOOTER_COPYRIGHT_COPY,
    STYLESHEET_LINK,
)
from buildapp.unsubscribe_pages import render_invalid_page

SIGNED_IN_ADDRESS = "someone@example.com"
MARKUP_INJECTION = "<b>onerror=x"
ESCAPED_INJECTION = "&lt;b&gt;"
WAITLIST_LINK = 'href="/#waitlist"'
SIGN_OUT_LINK = 'href="/auth/logout"'
APP_LINK = 'href="/app/"'
MODULE_DOCSTRING_DELIMITER = '"""'


def _every_page() -> tuple[str, ...]:
    return tuple(outcome.render() for outcome in OUTCOMES.values()) + (
        invite_only_outcome(SIGNED_IN_ADDRESS).render(),
    )


def test_every_refusal_reason_the_domain_can_return_has_a_row():
    assert set(OUTCOMES) == {
        InviteState.UNKNOWN,
        InviteState.REVOKED,
        InviteState.REDEEMED,
        InviteState.EXPIRED,
        EMAIL_MISMATCH,
    }


def test_each_outcome_answers_with_the_status_code_its_state_deserves():
    assert OUTCOMES[InviteState.UNKNOWN].status_code == 404
    assert OUTCOMES[InviteState.REVOKED].status_code == 410
    assert OUTCOMES[InviteState.EXPIRED].status_code == 410
    assert OUTCOMES[InviteState.REDEEMED].status_code == 200
    assert OUTCOMES[EMAIL_MISMATCH].status_code == 403
    assert invite_only_outcome(SIGNED_IN_ADDRESS).status_code == 403


def test_each_outcome_renders_its_own_title_and_heading_through_the_shell():
    for reason, outcome in OUTCOMES.items():
        html = outcome.render()
        assert f"<title>{outcome.title}</title>" in html, reason
        assert outcome.heading in html, reason
        assert outcome.message in html, reason
        assert STYLESHEET_LINK in html, reason


def test_the_already_used_page_sends_the_visitor_to_the_app():
    html = OUTCOMES[InviteState.REDEEMED].render()
    assert APP_LINK in html


def test_the_mismatch_page_offers_a_sign_out_because_the_fix_is_another_account():
    html = OUTCOMES[EMAIL_MISMATCH].render()
    assert SIGN_OUT_LINK in html


def test_the_invite_only_page_names_the_signed_in_address_and_offers_both_exits():
    html = invite_only_outcome(SIGNED_IN_ADDRESS).render()
    assert f"<title>{INVITE_ONLY_TITLE}</title>" in html
    assert INVITE_ONLY_HEADING in html
    assert SIGNED_IN_ADDRESS in html
    assert WAITLIST_LINK in html
    assert SIGN_OUT_LINK in html


def test_the_invite_only_page_escapes_the_address_it_was_handed():
    html = invite_only_outcome(f"{MARKUP_INJECTION}{SIGNED_IN_ADDRESS}").render()
    assert MARKUP_INJECTION not in html
    assert ESCAPED_INJECTION in html


def test_the_copy_is_verbatim():
    assert INVITE_ONLY_TITLE == "Invite only — Build"
    assert INVITE_ONLY_HEADING == "Build is invite-only right now."
    assert OUTCOMES[InviteState.UNKNOWN].heading == "This invite link is not valid."
    assert OUTCOMES[InviteState.REVOKED].heading == "This invite was withdrawn."
    assert OUTCOMES[InviteState.EXPIRED].heading == "This invite has expired."
    assert OUTCOMES[InviteState.REDEEMED].heading == "This invite has already been used."
    assert OUTCOMES[EMAIL_MISMATCH].heading == "This invite is for a different address."


def test_every_page_uses_the_shared_panel_and_carries_the_site_footer():
    panel_opening_markup = read_landing_file(PANEL_NAME).split("{{", 1)[0]
    assert panel_opening_markup in render_invalid_page(), "the panel is the shared one"
    for html in _every_page():
        assert panel_opening_markup in html
        assert FOOTER_ASSURANCE_COPY in html
        assert FOOTER_COPYRIGHT_COPY in html
        assert "<script" not in html


def test_the_markup_lives_in_the_landing_directory_not_in_python():
    source_after_the_docstring = Path(invite_pages.__file__).read_text().split(
        MODULE_DOCSTRING_DELIMITER, 2
    )[2]
    assert "<div" not in source_after_the_docstring
    assert "class=" not in source_after_the_docstring
