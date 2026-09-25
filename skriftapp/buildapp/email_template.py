"""The one outbound email layout. Every colour and size is a literal here rather than a
landing.css custom property because custom properties, rgba() and max-width do not
survive Outlook and Gmail.

Images are PNGs this deployment serves under ``/landing/email/``, linked by absolute URL
from its public base URL: mail clients run no scripts and fetch nothing relative. Many
block remote images, so every image carries its size and alt text and the message reads
in order without them."""

from __future__ import annotations

from dataclasses import dataclass
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
#: The mark's alt text, which is all a client blocking images shows of the header.
EMAIL_BRAND_FONT_SIZE_PX = 16
EMAIL_BRAND_LETTER_SPACING = ".08em"
EMAIL_MARK_PADDING_BOTTOM_PX = 28
EMAIL_HERO_PADDING_BOTTOM_PX = 24
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
EMAIL_STEPS_TITLE_PADDING_BOTTOM_PX = 12
EMAIL_STEP_NUMBER_WIDTH_PX = 28
EMAIL_COMMAND_PADDING = "10px 12px"
EMAIL_COMMAND_FONT_SIZE_PX = 13
#: How far the text part indents a step's command and detail under its number.
TEXT_STEP_INDENT = "   "

#: The text part's first line, where the HTML part has the mark.
TEXT_BRAND_LINE = "Build"
#: Where the root controller serves ``landing/email/``; see ``RootController.landing_asset``.
EMAIL_ASSET_DIRECTORY = "email"
EMAIL_ASSET_PATH = f"/landing/{EMAIL_ASSET_DIRECTORY}"
FOOTER_PREFIX = "Not you?"
UNSUBSCRIBE_LINK_LABEL = "Unsubscribe"


@dataclass(frozen=True)
class EmailAction:
    """A message's call-to-action. One value, because a URL with nothing to call it is
    not a button and a label with nowhere to go is not a link — neither half is
    meaningful without the other, so neither travels alone."""

    url: str
    label: str

    def html_row(self) -> str:
        """The landing's button style. Inline colours and a padded anchor rather than a
        real button: Outlook and Gmail drop both."""
        return (
            f'<tr><td style="padding-bottom:{EMAIL_ACTION_PADDING_BOTTOM_PX}px">'
            f'<a href="{escape(self.url)}" '
            f'style="display:inline-block;padding:{EMAIL_ACTION_PADDING};'
            f"background:{EMAIL_ACCENT};color:{EMAIL_BACKGROUND};"
            f"font-size:{EMAIL_BODY_FONT_SIZE_PX}px;"
            f"letter-spacing:{EMAIL_BRAND_LETTER_SPACING};"
            f'text-decoration:none">{escape(self.label)}</a>'
            "</td></tr>"
        )

    def text_line(self) -> str:
        return f"{self.label}: {self.url}"


@dataclass(frozen=True)
class EmailImage:
    """A hosted PNG shown at ``width_px`` × ``height_px``. The file is twice that size so
    it stays sharp on high-density screens; ``file_1x``, when there is one, is offered
    through ``srcset`` to the clients that read it."""

    file: str
    width_px: int
    height_px: int
    alt: str
    file_1x: str | None = None

    def url(self, public_base_url: str, file: str) -> str:
        return escape(f"{public_base_url}{EMAIL_ASSET_PATH}/{file}")

    def html(self, public_base_url: str, style: str) -> str:
        """The tag, sized by attributes because Outlook reads only those; ``style``
        sizes it everywhere else and styles its alt text."""
        src = self.url(public_base_url, self.file)
        srcset = (
            ""
            if self.file_1x is None
            else f' srcset="{self.url(public_base_url, self.file_1x)} 1x, {src} 2x"'
        )
        return (
            f'<img src="{src}"{srcset} alt="{escape(self.alt)}" '
            f'width="{self.width_px}" height="{self.height_px}" '
            f'style="display:block;border:0;{style}">'
        )


#: The Build mark, 32 px tall. brand-mark.svg rendered by design/email/README.md.
MARK_IMAGE = EmailImage(
    file="brand-mark@2x.png",
    file_1x="brand-mark.png",
    width_px=27,
    height_px=32,
    alt="Build",
)
#: The app on the landing's laptop, the full width of the layout. Rendered by
#: design/email/README.md.
LAPTOP_IMAGE = EmailImage(
    file="laptop.png", width_px=560, height_px=308, alt="Build on a laptop"
)
#: The mark keeps its size; its alt text is the accent, as the wordmark it replaced was.
MARK_STYLE = (
    f"width:{MARK_IMAGE.width_px}px;height:{MARK_IMAGE.height_px}px;"
    f"color:{EMAIL_ACCENT};font-size:{EMAIL_BRAND_FONT_SIZE_PX}px;"
    f"letter-spacing:{EMAIL_BRAND_LETTER_SPACING}"
)
#: The hero shrinks with a narrow screen and never grows past its own width.
HERO_STYLE = (
    f"width:100%;height:auto;color:{EMAIL_TEXT_SECONDARY};"
    f"font-size:{EMAIL_BODY_FONT_SIZE_PX}px"
)


@dataclass(frozen=True)
class EmailStep:
    """One numbered step: what to do, and optionally the command to paste and a line
    after it. The command gets its own block so it can be copied whole."""

    text: str
    command: str | None = None
    detail: str | None = None

    def html_cell(self) -> str:
        command = (
            ""
            if self.command is None
            else f'<p style="margin:0 0 {EMAIL_PARAGRAPH_SPACING_PX}px 0;'
            f"padding:{EMAIL_COMMAND_PADDING};background:{EMAIL_HAIRLINE};"
            f"color:{EMAIL_TEXT_PRIMARY};font-size:{EMAIL_COMMAND_FONT_SIZE_PX}px;"
            f'word-break:break-all"><code>{escape(self.command)}</code></p>'
        )
        detail = "" if self.detail is None else paragraph_html(self.detail)
        return f"{paragraph_html(self.text)}{command}{detail}"

    def text_block(self, number: int) -> str:
        indented = tuple(
            f"{TEXT_STEP_INDENT}{line}"
            for line in (self.command, self.detail)
            if line is not None
        )
        return "\n\n".join((f"{number}. {self.text}", *indented))


@dataclass(frozen=True)
class EmailSteps:
    """A titled, numbered list of steps. Numbers are table cells rather than an
    ``<ol>`` because Outlook indents list markers however it likes."""

    title: str
    steps: tuple[EmailStep, ...]

    def html_rows(self) -> str:
        title = (
            f'<tr><td style="padding-bottom:{EMAIL_STEPS_TITLE_PADDING_BOTTOM_PX}px;'
            f"color:{EMAIL_TEXT_PRIMARY};font-size:{EMAIL_BODY_FONT_SIZE_PX}px;"
            f'line-height:{EMAIL_BODY_LINE_HEIGHT}">{escape(self.title)}</td></tr>'
        )
        rows = "".join(
            '<tr><td><table role="presentation" width="100%" cellpadding="0" '
            'cellspacing="0" border="0"><tr>'
            f'<td valign="top" width="{EMAIL_STEP_NUMBER_WIDTH_PX}" '
            f"style=\"width:{EMAIL_STEP_NUMBER_WIDTH_PX}px;color:{EMAIL_TEXT_PRIMARY};"
            f"font-size:{EMAIL_BODY_FONT_SIZE_PX}px;"
            f'line-height:{EMAIL_BODY_LINE_HEIGHT}">{number}.</td>'
            f'<td valign="top">{step.html_cell()}</td>'
            "</tr></table></td></tr>"
            for number, step in enumerate(self.steps, start=1)
        )
        return f"{title}{rows}"

    def text_blocks(self) -> tuple[str, ...]:
        return (
            self.title,
            *(
                step.text_block(number)
                for number, step in enumerate(self.steps, start=1)
            ),
        )


#: What may follow the action: a plain paragraph, or a titled list of steps.
EmailBlock = str | EmailSteps


def paragraph_html(text: str) -> str:
    return (
        f'<p style="margin:0 0 {EMAIL_PARAGRAPH_SPACING_PX}px 0;'
        f"color:{EMAIL_TEXT_SECONDARY};font-size:{EMAIL_BODY_FONT_SIZE_PX}px;"
        f'line-height:{EMAIL_BODY_LINE_HEIGHT}">{escape(text)}</p>'
    )


def block_html_rows(block: EmailBlock) -> str:
    if isinstance(block, EmailSteps):
        return block.html_rows()
    return f"<tr><td>{paragraph_html(block)}</td></tr>"


def block_text(block: EmailBlock) -> tuple[str, ...]:
    return block.text_blocks() if isinstance(block, EmailSteps) else (block,)


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


def hero_html_row(hero: EmailImage | None, public_base_url: str) -> str:
    if hero is None:
        return ""
    style = f"max-width:{hero.width_px}px;{HERO_STYLE}"
    return (
        f'<tr><td style="padding-bottom:{EMAIL_HERO_PADDING_BOTTOM_PX}px">'
        f"{hero.html(public_base_url, style)}</td></tr>"
    )


def render_email_html(
    *,
    heading: str,
    paragraphs: tuple[str, ...],
    unsubscribe_url: str | None,
    public_base_url: str,
    action: EmailAction | None = None,
    after_action: tuple[EmailBlock, ...] = (),
    hero: EmailImage | None = None,
) -> str:
    """``hero`` is an image under the opening copy and above the action."""
    paragraph_markup = "".join(paragraph_html(paragraph) for paragraph in paragraphs)
    after_action_markup = "".join(block_html_rows(block) for block in after_action)
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
        f'<tr><td style="padding-bottom:{EMAIL_MARK_PADDING_BOTTOM_PX}px">'
        f"{MARK_IMAGE.html(public_base_url, MARK_STYLE)}</td></tr>"
        f'<tr><td style="padding-bottom:{EMAIL_HEADING_PADDING_BOTTOM_PX}px;'
        f"color:{EMAIL_TEXT_PRIMARY};font-size:{EMAIL_HEADING_FONT_SIZE_PX}px;"
        f'line-height:{EMAIL_HEADING_LINE_HEIGHT}">{escape(heading)}</td></tr>'
        f"<tr><td>{paragraph_markup}</td></tr>"
        f"{hero_html_row(hero, public_base_url)}"
        f"{'' if action is None else action.html_row()}"
        f"{after_action_markup}"
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
    action: EmailAction | None = None,
    after_action: tuple[EmailBlock, ...] = (),
) -> str:
    action_lines = () if action is None else (action.text_line(),)
    footer_lines = (
        ()
        if unsubscribe_url is None
        else (f"{UNSUBSCRIBE_LINK_LABEL}: {unsubscribe_url}",)
    )
    after_action_lines = tuple(
        line for block in after_action for line in block_text(block)
    )
    return "\n\n".join(
        (
            TEXT_BRAND_LINE,
            heading,
            *paragraphs,
            *action_lines,
            *after_action_lines,
            *footer_lines,
        )
    )
