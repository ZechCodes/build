"""Web push — the api side of the bridge's unread notifications.

A push fires exactly when something adds to the unread counter (#191): an
agent's conversation (``agent``) or a watched task (``task``, the user's word
for a task, #190). Every payload carries an opaque id, the generic kind and the
deep link built from them.

E2EE invariant (#200): the api forwards opaque ciphertext and sees only sids.
A notify may carry ``sealed``: per subscription, a blob the bridge sealed to
that browser's notification key. The api matches a blob to a subscription by
``subscription_id(endpoint)``, checks the blob's shape (base64url characters and
length) and forwards it byte-identical in that subscription's payload; it never
decodes, parses, stores or logs a blob or a payload. Content authenticity rests
on the notification public key never reaching the api: the sealing is not
sender-authenticated, so anyone holding that key could seal content, and the
key travels only from the browser to the bridge over the E2EE session.
A subscription without a blob gets the #191 payload, which the service worker
renders as generic copy. See ``planning/v2/Push Content Security Checklist.md``.

The notify request the bridge POSTs is authenticated the same way as device
registration: an Ed25519 signature over a challenge binding every field,
including a digest of ``sealed``, checked against the device's pinned identity
key. ``notify_challenge`` mirrors ``bridge/src/notify.rs`` byte-for-byte. A
timestamp freshness window bounds replay of a captured request.

Pure helpers only, plus a delivery function with an injectable sender — no DB or
framework imports, trivially unit-testable.
"""

from __future__ import annotations

import base64
import hashlib
import ipaddress
import json
import logging
from collections.abc import Iterable, Sequence
from datetime import datetime, timedelta
from typing import NamedTuple
from urllib.parse import urlsplit

import requests
from pywebpush import WebPushException, webpush

__all__ = [
    "AGENT_KIND",
    "ALLOWED_KINDS",
    "MAX_SEALED_BLOB_CHARS",
    "MAX_SEALED_ENTRIES",
    "NOTIFY_FRESHNESS_WINDOW",
    "PUSH_SEND_TIMEOUT_SECONDS",
    "PUSH_TTL_SECONDS",
    "SUBSCRIPTION_ID_CHARS",
    "TASK_KIND",
    "VAPID_PRIVATE_KEY_ENV",
    "VAPID_PUBLIC_KEY_ENV",
    "VAPID_SUBJECT_ENV",
    "NotifyReplayGuard",
    "SealedEntry",
    "WebPushException",
    "notify_challenge",
    "notify_timestamp_fresh",
    "parse_sealed",
    "push_endpoint_allowed",
    "push_payload",
    "sealed_deliveries",
    "sealed_digest",
    "send_to_subscriptions",
    "subscription_gone",
    "subscription_id",
    "unknown_subscription_ids",
]

logger = logging.getLogger(__name__)

# Notify kinds (contract #6), mirroring ``bridge/src/notify.rs``. A kind is a
# generic label, never content: the service worker renders its copy, and each
# kind deep-links by its own route — an agent by its conversation owner (a run
# id, which the SPA resolves), a task by its task id.
AGENT_KIND = "agent"
TASK_KIND = "task"
_DEEP_LINK_ROUTES = {AGENT_KIND: "task", TASK_KIND: "tasks"}
ALLOWED_KINDS = frozenset(_DEEP_LINK_ROUTES)

# How far a notify timestamp may drift from server time before it's rejected
# (replay bound + clock-skew allowance).
NOTIFY_FRESHNESS_WINDOW = timedelta(minutes=5)

# Per-subscription delivery timeout. Passed explicitly because pywebpush's
# default is ``10000`` handed to ``requests`` as SECONDS — one hanging push
# service endpoint would otherwise block the worker thread for hours.
PUSH_SEND_TIMEOUT_SECONDS = 10

# The push service's time to live for every push, in seconds: 0 means deliver
# now or drop, which is also pywebpush's default, stated here because the service
# worker's freshness window for a sealed blob is built on it. The SPA's tests
# read this line with a regex, so it stays ``PUSH_TTL_SECONDS = <int>``.
PUSH_TTL_SECONDS = 0

# The shape of ``sealed`` (#200): at most one entry per bridge-side key (the
# bridge keeps 32), a 43-character sid, and a base64url blob of at most 2048
# characters. Nothing past the shape is checked: the blob is never decoded.
MAX_SEALED_ENTRIES = 32
MAX_SEALED_BLOB_CHARS = 2048
SUBSCRIPTION_ID_CHARS = 43
_SEALED_ENTRY_KEYS = frozenset({"subscription_id", "blob"})
_BASE64URL_CHARACTERS = frozenset(
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
)

VAPID_PRIVATE_KEY_ENV = "VAPID_PRIVATE_KEY"
VAPID_PUBLIC_KEY_ENV = "VAPID_PUBLIC_KEY"
VAPID_SUBJECT_ENV = "VAPID_SUBJECT"


class SealedEntry(NamedTuple):
    """One subscription's opaque blob, as the bridge sent it."""

    subscription_id: str
    blob: str


def subscription_id(endpoint: str) -> str:
    """The id the bridge knows a subscription by: unpadded base64url of the
    SHA-256 of its push endpoint, so the endpoint itself never leaves the api."""
    digest = hashlib.sha256(endpoint.encode("utf-8")).digest()
    return base64.urlsafe_b64encode(digest).rstrip(b"=").decode("ascii")


def _is_base64url(value: object, min_length: int, max_length: int) -> bool:
    return (
        isinstance(value, str)
        and min_length <= len(value) <= max_length
        and set(value) <= _BASE64URL_CHARACTERS
    )


def _parse_sealed_entry(entry: object) -> SealedEntry:
    if not isinstance(entry, dict) or set(entry) != _SEALED_ENTRY_KEYS:
        raise ValueError("a sealed entry is {subscription_id, blob}")
    sid, blob = entry["subscription_id"], entry["blob"]
    if not _is_base64url(sid, SUBSCRIPTION_ID_CHARS, SUBSCRIPTION_ID_CHARS):
        raise ValueError("malformed sealed subscription id")
    if not _is_base64url(blob, 1, MAX_SEALED_BLOB_CHARS):
        raise ValueError("malformed sealed blob")
    return SealedEntry(sid, blob)


def parse_sealed(value: object) -> tuple[SealedEntry, ...]:
    """The notify's ``sealed`` entries in request order, or ``ValueError`` for
    any other shape. Checks only the shape: a blob is never decoded."""
    if not isinstance(value, list) or len(value) > MAX_SEALED_ENTRIES:
        raise ValueError(f"sealed is a list of at most {MAX_SEALED_ENTRIES} entries")
    entries = tuple(_parse_sealed_entry(entry) for entry in value)
    if len({entry.subscription_id for entry in entries}) != len(entries):
        raise ValueError("a subscription id appears twice in sealed")
    return entries


def sealed_digest(sealed: Iterable[SealedEntry]) -> str:
    """Lowercase hex SHA-256 of the ``"{subscription_id}:{blob}\n"`` lines in
    request order: what a v2 notify challenge binds."""
    lines = "".join(f"{entry.subscription_id}:{entry.blob}\n" for entry in sealed)
    return hashlib.sha256(lines.encode("utf-8")).hexdigest()


def notify_challenge(
    device_id: str,
    task_id: str,
    kind: str,
    timestamp: int,
    sealed: Sequence[SealedEntry] = (),
) -> str:
    """The canonical message the bridge signs for a notify. Binds the device, the
    task, the kind, the timestamp and, when there is any, the sealed content, so
    a captured signature cannot be replayed onto a different notification
    (mirrors ``bridge/src/notify.rs::notify_challenge``). Without ``sealed`` it
    is the #191 challenge, unchanged."""
    challenge = f"notify.{device_id}.{task_id}.{kind}.{timestamp}"
    if sealed:
        challenge += f".{sealed_digest(sealed)}"
    return challenge


def notify_timestamp_fresh(timestamp: int, now: datetime) -> bool:
    """Whether a notify timestamp (unix seconds) is within the freshness window of
    ``now`` — in either direction, tolerating modest clock skew."""
    drift = abs(now.timestamp() - timestamp)
    return drift <= NOTIFY_FRESHNESS_WINDOW.total_seconds()


def push_payload(task_id: str, kind: str, sealed_blob: str | None = None) -> str:
    """The push payload delivered to the service worker (contract #6):
    ``{"task_id", "kind", "url"}``, plus ``"sealed"`` when the subscription has a
    blob. ``task_id`` is opaque and ``kind`` a generic label; the ``url``
    deep-links under ``/app/`` by the kind's hash route (``#/task/<run>`` or
    ``#/tasks/<task>``). The blob goes in verbatim: only the browser opens it."""
    route = _DEEP_LINK_ROUTES[kind]
    payload = {"task_id": task_id, "kind": kind, "url": f"/app/#/{route}/{task_id}"}
    if sealed_blob is not None:
        payload["sealed"] = sealed_blob
    return json.dumps(payload)


def sealed_deliveries(
    subscription_infos: list[dict],
    sealed: Iterable[SealedEntry],
    task_id: str,
    kind: str,
) -> list[tuple[dict, str]]:
    """Each subscription paired with its own payload: its blob when ``sealed``
    has one for its sid, else the #191 payload."""
    blobs = {entry.subscription_id: entry.blob for entry in sealed}
    return [
        (
            subscription_info,
            push_payload(
                task_id, kind, blobs.get(subscription_id(subscription_info["endpoint"]))
            ),
        )
        for subscription_info in subscription_infos
    ]


def unknown_subscription_ids(
    sealed: Iterable[SealedEntry], live_endpoints: Iterable[str]
) -> list[str]:
    """The sealed sids, in request order, that match none of ``live_endpoints``:
    the bridge deletes the keys it holds for them."""
    live = {subscription_id(endpoint) for endpoint in live_endpoints}
    return [entry.subscription_id for entry in sealed if entry.subscription_id not in live]


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
    deliveries: list[tuple[dict, str]],
    vapid_private_key: str,
    vapid_subject: str,
    send=_send_without_redirects,
) -> tuple[int, list[str]]:
    """Push each ``(subscription_info, payload)`` pair; return
    ``(delivered, gone_endpoints)``.

    ``gone_endpoints`` are subscriptions the push service reports as dead — the
    caller owns the state change of deleting them. Transient errors are counted
    as neither delivered nor gone, and so is an endpoint off the push services,
    which is never sent to. A failure is logged by exception type and sid only:
    neither a payload nor an exception's text (which may echo one) is logged.
    ``send`` is injectable for tests.
    """
    delivered = 0
    gone: list[str] = []
    for subscription_info, payload in deliveries:
        endpoint = str(subscription_info.get("endpoint", ""))
        if not push_endpoint_allowed(endpoint):
            logger.warning("push endpoint is not a known push service; not sending")
            continue
        try:
            send(
                subscription_info=subscription_info,
                data=payload,
                vapid_private_key=vapid_private_key,
                vapid_claims={"sub": vapid_subject},
                timeout=PUSH_SEND_TIMEOUT_SECONDS,
                ttl=PUSH_TTL_SECONDS,
            )
            delivered += 1
        except WebPushException as exc:
            status_code = getattr(exc.response, "status_code", None)
            if subscription_gone(status_code):
                gone.append(endpoint)
        except requests.exceptions.RequestException as exc:
            # pywebpush does not wrap transport failures. One dead endpoint is
            # transient for that browser only — log it and keep delivering to
            # the rest instead of 500ing the whole notify.
            logger.warning(
                "push delivery failed for subscription %s: %s",
                subscription_id(endpoint),
                type(exc).__name__,
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
