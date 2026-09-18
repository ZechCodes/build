"""Verified practical content for the public landing page and its documentation.

The cinematic story is intentionally static. Facts that can drift -- supported host
builds, harness status, repository visibility -- are rendered here from the same
release table as the app and a small, reviewable snapshot. A private repository never
turns local commit messages into pretend public activity.
"""

from __future__ import annotations

import json
from html import escape
from typing import Any

from buildapp import releases
from buildapp.landing_page import fill_slots, read_landing_file

CONTENT_FILE_NAME = "content.json"
PRACTICAL_FRAGMENT_NAME = "practical.html"
DOCS_PATH = "/docs"
PRIVACY_PATH = "/privacy"


def load_content() -> dict[str, Any]:
    return json.loads(read_landing_file(CONTENT_FILE_NAME))


def render_practical_content() -> str:
    content = load_content()
    source = content["source"]
    return fill_slots(
        read_landing_file(PRACTICAL_FRAGMENT_NAME),
        {
            "platforms": _render_platforms(),
            "activity": _render_activity(content["activity"]),
            "repository_url": escape(source["repository_url"], quote=True),
        },
    )


def render_docs_body() -> str:
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
    <p>Install Build on the macOS or Linux computer that holds your projects and runs your coding agents. After alpha access is confirmed, the app provides the authenticated installer, then asks you to pair the host using the code and fingerprint printed by the bridge.</p>
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


def _render_platforms() -> str:
    return "\n".join(
        '          <li><span>{label}</span><span class="availability">Host installer</span></li>'.format(
            label=escape(label)
        )
        for _key, label in releases.PLATFORMS
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
        return (
            '<div class="activity-empty" role="status">'
            f'<p>{escape(activity["reason"])}</p>'
            "</div>"
        )
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
