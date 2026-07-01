"""Content-free web push — the api side of the bridge's attention notifications.

E2EE invariant: the server never sees task content, so a push payload carries
**no content at all** — just ``{"kind": "attention", "url": "/app/"}``. The
service worker renders it generically; the real state loads over the E2EE
channel when the app opens.

The notify request the bridge POSTs is authenticated the same way as device
registration: an Ed25519 signature over a challenge binding every field, checked
against the device's pinned identity key. ``notify_challenge`` mirrors
``bridge/src/notify.rs`` byte-for-byte. A timestamp freshness window bounds
replay of a captured request.

Pure helpers only, plus a delivery function with an injectable sender — no DB or
framework imports, trivially unit-testable.
"""

from __future__ import annotations

import json
from datetime import datetime, timedelta

from pywebpush import WebPushException, webpush

__all__ = [
    "ATTENTION_KIND",
    "NOTIFY_FRESHNESS_WINDOW",
    "VAPID_PRIVATE_KEY_ENV",
    "VAPID_PUBLIC_KEY_ENV",
    "VAPID_SUBJECT_ENV",
    "WebPushException",
    "attention_payload",
    "notify_challenge",
    "notify_timestamp_fresh",
    "send_to_subscriptions",
    "subscription_gone",
]

# The only notify kind: "a task needs your attention". Content-free by contract.
ATTENTION_KIND = "attention"

# How far a notify timestamp may drift from server time before it's rejected
# (replay bound + clock-skew allowance).
NOTIFY_FRESHNESS_WINDOW = timedelta(minutes=5)

VAPID_PRIVATE_KEY_ENV = "VAPID_PRIVATE_KEY"
VAPID_PUBLIC_KEY_ENV = "VAPID_PUBLIC_KEY"
VAPID_SUBJECT_ENV = "VAPID_SUBJECT"


def notify_challenge(device_id: str, kind: str, timestamp: int) -> str:
    """The canonical message the bridge signs for a notify. Binds the device, the
    kind, and the timestamp so a captured signature cannot be replayed onto a
    different notification (mirrors ``bridge/src/notify.rs::notify_challenge``)."""
    return f"notify.{device_id}.{kind}.{timestamp}"


def notify_timestamp_fresh(timestamp: int, now: datetime) -> bool:
    """Whether a notify timestamp (unix seconds) is within the freshness window of
    ``now`` — in either direction, tolerating modest clock skew."""
    drift = abs(now.timestamp() - timestamp)
    return drift <= NOTIFY_FRESHNESS_WINDOW.total_seconds()


def attention_payload() -> str:
    """The one payload we ever push: generic, content-free, points at the app."""
    return json.dumps({"kind": ATTENTION_KIND, "url": "/app/"})


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
    send=webpush,
) -> tuple[int, list[str]]:
    """Push ``payload`` to every subscription; return ``(delivered, gone_endpoints)``.

    ``gone_endpoints`` are subscriptions the push service reports as dead — the
    caller owns the state change of deleting them. Transient errors are counted
    as neither delivered nor gone. ``send`` is injectable for tests.
    """
    delivered = 0
    gone: list[str] = []
    for subscription_info in subscription_infos:
        try:
            send(
                subscription_info=subscription_info,
                data=payload,
                vapid_private_key=vapid_private_key,
                vapid_claims={"sub": vapid_subject},
            )
            delivered += 1
        except WebPushException as exc:
            status_code = getattr(exc.response, "status_code", None)
            if subscription_gone(status_code):
                gone.append(str(subscription_info["endpoint"]))
    return delivered, gone
