"""Verified practical content for the public landing page and its documentation.

The story is a static build. Facts that can drift -- harness status, repository
visibility, merged work -- are rendered here from the same release table as the app
and a small, reviewable snapshot. A private repository never turns local commit
messages into pretend public activity.

The homepage asks for exactly two of these at request time (``render_homepage_slots``);
everything else on it is baked into the generated document.
"""

from __future__ import annotations

import json
from html import escape
from typing import Any

from buildapp import releases
from buildapp.landing_page import fill_slots, read_landing_file

CONTENT_FILE_NAME = "content.json"
ACTIVITY_FRAGMENT_NAME = "activity-section.html"
DOCS_PATH = "/docs"
PRIVACY_PATH = "/privacy"


def load_content() -> dict[str, Any]:
    return json.loads(read_landing_file(CONTENT_FILE_NAME))


def render_homepage_slots() -> dict[str, str]:
    """The two values the generated homepage cannot carry: the activity feed, which
    is only as current as the last refresh, and the repository link."""
    content = load_content()
    repository_url = content["source"]["repository_url"]
    return {
        "activity_section": _render_activity_section(content["activity"], repository_url),
        "repository_url": escape(repository_url, quote=True),
    }


def render_docs_body() -> str:
    host_install_command = escape(releases.install_command("https://getbuild.ing"))
    desktop_install_command = escape(
        releases.install_command("https://getbuild.ing", desktop=True)
    )
    harnesses = "\n".join(
        '<li><div><strong>{name}</strong><span class="status status--{status}">'
        "{status}</span></div><p>{detail}</p></li>".format(
            name=escape(harness["name"]),
            status=escape(harness["status"]),
            detail=escape(harness["detail"]),
        )
        for harness in load_content()["harnesses"]
    )
    return f"""<main class="public-doc content-container">
  <a class="doc-back" href="/" aria-label="Build home"><img src="/landing/brand-mark.svg" alt="">build</a>
  <h1>Build documentation</h1>
  <section id="setup" aria-labelledby="setup-title">
    <h2 id="setup-title">Set up a host</h2>
    <p>Install the Build host on the macOS or Linux computer that holds your projects and runs your coding agents. The host installer is public.</p>
    <p class="install-command"><code>{host_install_command}</code></p>
    <p>The optional desktop client has a separate public installer:</p>
    <p class="install-command"><code>{desktop_install_command}</code></p>
    <p>Installer downloads are public. Alpha access is required to pair and use a host. Sign in to the app, then pair the host using the code and fingerprint printed by the bridge.</p>
    <a class="primary-action" href="/app/">Open alpha setup</a>
  </section>
  <section id="architecture" aria-labelledby="architecture-title">
    <h2 id="architecture-title">Connection architecture</h2>
    <p>The app server handles authentication, device registration, and rendezvous. The browser and host negotiate an encrypted WebRTC data channel. Direct connections reveal each peer's IP to the other; when direct connection fails, Cloudflare TURN can relay DTLS ciphertext.</p>
    <p>Your coding agents, worktrees, terminals, and Git operations run on the paired host.</p>
    <a href="/privacy">Read the storage and transport details</a>
  </section>
  <section id="harnesses" aria-labelledby="harnesses-title">
    <h2 id="harnesses-title">Coding harness integrations</h2>
    <p>Harnesses must already be installed and authenticated on the host. Build selects among the integrations the connected bridge reports.</p>
    <ul class="integration-list">{harnesses}</ul>
  </section>
</main>"""


def render_privacy_body() -> str:
    return """<main class="public-doc content-container">
  <a class="doc-back" href="/" aria-label="Build home"><img src="/landing/brand-mark.svg" alt="">build</a>
  <h1>Architecture and privacy</h1>
  <p>Build stores the account and device records needed to authenticate you, approve a host, and establish a connection. Device records include the host's registered identity and connection status.</p>
  <h2>Connection diagnostics</h2>
  <p>The hosted Build service stores content-free transport diagnostics: session, account, and device identifiers; connection timestamps; the current and first carrying path (relay, direct, or TURN); and counters for carrying, TURN, and lost-channel events. These diagnostic rows remain as transport history after the associated account or device is deleted.</p>
  <h2>Optional browser notifications</h2>
  <p>If you enable browser push notifications, the hosted Build service stores the browser's push endpoint and its p256dh and auth encryption keys with your account. Unsubscribing removes that endpoint for the signed-in account.</p>
  <h2>Where agent work runs</h2>
  <p>Coding agents execute on your paired host. Worktrees, terminals, files, diffs, and Git operations are produced there and sent to the browser over the encrypted application session.</p>
  <h2>How a browser reaches a host</h2>
  <p>The app and relay coordinate WebRTC negotiation. A direct connection exposes each peer's IP address to the other. If direct connection is unavailable, Cloudflare TURN may relay DTLS ciphertext and can observe connection metadata such as source IP, timing, and traffic size.</p>
  <h2>Provider boundary</h2>
  <p>The hosted Build service does not receive or store your coding-harness credentials. Each harness is installed and authenticated on the host, and its own provider terms and data handling still apply.</p>
  <a href="/docs#architecture">Read the setup and connection guide</a>
</main>"""


def _render_activity_section(activity: dict[str, Any], repository_url: str) -> str:
    """The whole "built in the open" section, or nothing: an empty feed on a
    landing page says less than no feed."""
    rows = _render_activity(activity)
    if not rows:
        return ""
    return fill_slots(
        read_landing_file(ACTIVITY_FRAGMENT_NAME),
        {"activity": rows, "repository_url": escape(repository_url, quote=True)},
    )


def _render_activity(activity: dict[str, Any]) -> str:
    entries = [
        entry
        for entry in activity["entries"]
        if entry.get("release_status") == "merged_not_released"
        and all(
            entry.get(field)
            for field in ("summary", "category", "merged_at", "source_url")
        )
    ][:5]
    if not entries:
        return ""
    rows = "\n".join(_render_activity_entry(entry) for entry in entries)
    return f'<ol class="activity-list">{rows}</ol>'


def _render_activity_entry(entry: dict[str, str]) -> str:
    return (
        "<li>"
        f'<a href="{escape(entry["source_url"], quote=True)}">{escape(entry["summary"])}</a>'
        f'<span>{escape(entry["category"])}</span>'
        f'<time datetime="{escape(entry["merged_at"], quote=True)}">{escape(entry["merged_at"][:10])}</time>'
        '<span class="merge-status">Merged, not necessarily released</span>'
        "</li>"
    )
