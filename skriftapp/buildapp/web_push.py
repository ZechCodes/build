"""Content-free web push — the api side of the bridge's unread notifications.

A push fires exactly when something adds to the unread counter (#191): an
agent's conversation (``agent``) or a watched task (``task``, the user's word
for a task, #190). E2EE invariant: the server never sees content, so a push
payload carries **no content at all** — an opaque id, the generic kind and the
deep link built from them. The service worker renders the kind's copy; the
real state loads over the E2EE channel when the app opens.

The notify request the bridge POSTs is authenticated the same way as device
registration: an Ed25519 signature over a challenge binding every field, checked
against the device's pinned identity key. ``notify_challenge`` mirrors
``bridge/src/notify.rs`` byte-for-byte. A timestamp freshness window bounds
replay of a captured request.

Pure helpers only, plus a delivery function with an injectable sender — no DB or
framework imports, trivially unit-testable.
"""

from __future__ import annotations

import ipaddress
import json
import logging
from datetime import datetime, timedelta
from urllib.parse import urlsplit

import requests
from pywebpush import WebPushException, webpush

__all__ = [
    "AGENT_KIND",
    "ALLOWED_KINDS",
    "NOTIFY_FRESHNESS_WINDOW",
    "PUSH_SEND_TIMEOUT_SECONDS",
    "TASK_KIND",
    "VAPID_PRIVATE_KEY_ENV",
    "VAPID_PUBLIC_KEY_ENV",
    "VAPID_SUBJECT_ENV",
    "NotifyReplayGuard",
    "WebPushException",
    "notify_challenge",
    "notify_timestamp_fresh",
    "push_endpoint_allowed",
    "push_payload",
    "send_to_subscriptions",
    "subscription_gone",
]

logger = logging.getLogger(__name__)

# Notify kinds (contract #6), mirroring ``bridge/src/notify.rs``. A kind is a
# generic label, never content: the service worker renders its copy, and each
# kind deep-links by its own route — an agent by its conversation owner (a run
# id, which the SPA resolves), a task by its task id.
AGENT_KIND = "agent"
TASK_KIND = "task"
_DEEP_LINK_ROUTES = {AGENT_KIND: "task", TASK_KIND: "task"}
ALLOWED_KINDS = frozenset(_DEEP_LINK_ROUTES)

# How far a notify timestamp may drift from server time before it's rejected
# (replay bound + clock-skew allowance).
NOTIFY_FRESHNESS_WINDOW = timedelta(minutes=5)

# Per-subscription delivery timeout. Passed explicitly because pywebpush's
# default is ``10000`` handed to ``requests`` as SECONDS — one hanging push
# service endpoint would otherwise block the worker thread for hours.
PUSH_SEND_TIMEOUT_SECONDS = 10

VAPID_PRIVATE_KEY_ENV = "VAPID_PRIVATE_KEY"
VAPID_PUBLIC_KEY_ENV = "VAPID_PUBLIC_KEY"
VAPID_SUBJECT_ENV = "VAPID_SUBJECT"


def notify_challenge(device_id: str, task_id: str, kind: str, timestamp: int) -> str:
    """The canonical message the bridge signs for a notify. Binds the device, the
    task, the kind, and the timestamp so a captured signature cannot be replayed
    onto a different notification (mirrors ``bridge/src/notify.rs::notify_challenge``)."""
    return f"notify.{device_id}.{task_id}.{kind}.{timestamp}"


def notify_timestamp_fresh(timestamp: int, now: datetime) -> bool:
    """Whether a notify timestamp (unix seconds) is within the freshness window of
    ``now`` — in either direction, tolerating modest clock skew."""
    drift = abs(now.timestamp() - timestamp)
    return drift <= NOTIFY_FRESHNESS_WINDOW.total_seconds()


def push_payload(task_id: str, kind: str) -> str:
    """The push payload delivered to the service worker (contract #6):
    ``{"task_id", "kind", "url"}``. Still content-free — ``task_id`` is opaque and
    ``kind`` is a generic label; the ``url`` deep-links under ``/app/`` by the
    kind's hash route (``#/task/<run>`` or ``#/task/<task>``), and the real
    state loads only over the E2EE channel once the app opens."""
    route = _DEEP_LINK_ROUTES[kind]
    return json.dumps(
        {"task_id": task_id, "kind": kind, "url": f"/app/#/{route}/{task_id}"}
    )


# The push services a browser subscribes through, and so the only hosts the api
# ever sends to: a stored endpoint is a URL the server POSTs to, so an arbitrary
# one would let any signed-in caller aim the server at a host of their choosing.
# A host matches exactly, or as a subdomain of a suffix — never by substring.
PUSH_SERVICE_HOSTS = frozenset(
    {
        "fcm.googleapis.com",  # Chrome, and the browsers built on it
        "jmt17.google.com",  # Chromium's own builds
        "updates.push.services.mozilla.com",  # Firefox
    }
)
PUSH_SERVICE_DOMAINS = (
    "push.services.mozilla.com",  # Firefox
    "push.apple.com",  # Safari
    "notify.windows.com",  # Edge on Windows
)
_HOSTNAME_CHARACTERS = frozenset("abcdefghijklmnopqrstuvwxyz0123456789-.")


def push_endpoint_allowed(endpoint: str) -> bool:
    """Whether ``endpoint`` is an https URL on a known push service, on the
    default port, with no credentials and no IP literal — checked when the
    subscription is stored and again before anything is sent to it."""
    if any(
        character.isspace() or not character.isprintable() or character == "\\"
        for character in endpoint
    ):
        return False
    try:
        parts = urlsplit(endpoint)
    except ValueError:
        return False
    host = parts.hostname or ""
    if parts.scheme != "https":
        return False
    if parts.netloc.lower() not in (host, f"{host}:443"):
        return False  # another port, credentials, or anything else in the authority
    if not host or not set(host) <= _HOSTNAME_CHARACTERS:
        return False
    try:
        ipaddress.ip_address(host)
        return False
    except ValueError:
        pass
    return host in PUSH_SERVICE_HOSTS or any(
        host.endswith(f".{domain}") for domain in PUSH_SERVICE_DOMAINS
    )


class _NoRedirectSession(requests.Session):
    """A push service answers a delivery itself; a redirect would carry the
    server's POST to a host the allowlist never checked, so none is followed."""

    def request(self, *args, **kwargs):
        kwargs["allow_redirects"] = False
        return super().request(*args, **kwargs)


def _send_without_redirects(**kwargs):
    """``webpush`` over a session that follows no redirect; a 3xx answer is
    above 202, so pywebpush raises it as a failed delivery."""
    with _NoRedirectSession() as session:
        return webpush(requests_session=session, **kwargs)


def subscription_gone(status_code: int | None) -> bool:
    """Push-service statuses that mean the subscription no longer exists and
    should be pruned (as opposed to transient failures, which are retried by the
    next notify)."""
    return status_code in (404, 410)


def send_to_subscriptions(
    subscription_infos: list[dict],
    payload: str,
    vapid_private_key: str,
    vapid_subject: str,
    send=_send_without_redirects,
) -> tuple[int, list[str]]:
    """Push ``payload`` to every subscription; return ``(delivered, gone_endpoints)``.

    ``gone_endpoints`` are subscriptions the push service reports as dead — the
    caller owns the state change of deleting them. Transient errors are counted
    as neither delivered nor gone, and so is an endpoint off the push services,
    which is never sent to. ``send`` is injectable for tests.
    """
    delivered = 0
    gone: list[str] = []
    for subscription_info in subscription_infos:
        if not push_endpoint_allowed(str(subscription_info.get("endpoint", ""))):
            logger.warning("push endpoint is not a known push service; not sending")
            continue
        try:
            send(
                subscription_info=subscription_info,
                data=payload,
                vapid_private_key=vapid_private_key,
                vapid_claims={"sub": vapid_subject},
                timeout=PUSH_SEND_TIMEOUT_SECONDS,
            )
            delivered += 1
        except WebPushException as exc:
            status_code = getattr(exc.response, "status_code", None)
            if subscription_gone(status_code):
                gone.append(str(subscription_info["endpoint"]))
        except requests.exceptions.RequestException as exc:
            # pywebpush does not wrap transport failures. One dead endpoint is
            # transient for that browser only — log it and keep delivering to
            # the rest instead of 500ing the whole notify.
            logger.warning(
                "push delivery failed for %s: %s", subscription_info.get("endpoint"), exc
            )
    return delivered, gone


class NotifyReplayGuard:
    """Rejects a replayed notify: the freshness window alone leaves a captured
    signed request replayable for its whole span, so the ``(device_id, timestamp,
    signature)`` tuples seen recently are remembered and duplicates refused —
    the same scheme as the relay's device-auth ``ReplayGuard``.

    The memory spans **twice** the freshness window: a notify stamped a full
    window in the future stays fresh for another full window after receipt.
    In-process state — matches the single-replica api deployment (a deploy's
    two pods overlap for seconds; see ``deploy/k8s/app.yaml``).
    """

    def __init__(self, ttl: timedelta = 2 * NOTIFY_FRESHNESS_WINDOW):
        self._ttl = ttl
        self._seen: dict[tuple[str, int, str], datetime] = {}

    def check_and_record(
        self, device_id: str, timestamp: int, signature: str, now: datetime
    ) -> bool:
        """Record the tuple; ``False`` if it was already seen within the TTL."""
        self._evict_expired(now)
        key = (device_id, timestamp, signature)
        if key in self._seen:
            return False
        self._seen[key] = now
        return True

    def _evict_expired(self, now: datetime) -> None:
        self._seen = {
            key: seen_at for key, seen_at in self._seen.items() if now - seen_at <= self._ttl
        }
