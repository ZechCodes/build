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
import logging
from datetime import datetime, timedelta

import requests
from pywebpush import WebPushException, webpush

__all__ = [
    "ALLOWED_KINDS",
    "ATTENTION_KIND",
    "BLOCKED_KIND",
    "NOTIFY_FRESHNESS_WINDOW",
    "PLAN_READY_KIND",
    "PUSH_SEND_TIMEOUT_SECONDS",
    "TASK_DONE_KIND",
    "VAPID_PRIVATE_KEY_ENV",
    "VAPID_PUBLIC_KEY_ENV",
    "VAPID_SUBJECT_ENV",
    "NotifyReplayGuard",
    "WebPushException",
    "notify_challenge",
    "notify_timestamp_fresh",
    "push_payload",
    "send_to_subscriptions",
    "subscription_gone",
]

logger = logging.getLogger(__name__)

# Notify kinds (contract #6). A kind is a generic status label, never task text;
# the service worker renders kind-specific copy, and the real state loads over the
# E2EE channel once the app opens.
PLAN_READY_KIND = "plan_ready"
TASK_DONE_KIND = "task_done"
BLOCKED_KIND = "blocked"
ATTENTION_KIND = "attention"
ALLOWED_KINDS = frozenset({PLAN_READY_KIND, TASK_DONE_KIND, BLOCKED_KIND, ATTENTION_KIND})

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
    ``kind`` is a generic status label; the ``url`` deep-links to the task view
    under ``/app/`` (hash route ``#/task/<id>``), and the real task state loads
    only over the E2EE channel once the app opens."""
    return json.dumps(
        {"task_id": task_id, "kind": kind, "url": f"/app/#/task/{task_id}"}
    )


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
    In-process state — matches the single-replica api deployment.
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
