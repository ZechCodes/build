"""Web-push unit tests: the notify challenge/signature contract with the bridge,
timestamp freshness, the content-free payload rule, and gone-subscription pruning.

E2EE invariant under test: nothing task-shaped may appear in a push payload —
the server only ever says "something needs your attention".
"""

from __future__ import annotations

import base64
import json
from datetime import datetime, timedelta, timezone

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import (
    Encoding,
    PublicFormat,
)

from buildapp import pairing_crypto, web_push


def _ed25519_pair() -> tuple[Ed25519PrivateKey, str]:
    private_key = Ed25519PrivateKey.generate()
    public_b64 = base64.b64encode(
        private_key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
    ).decode("ascii")
    return private_key, public_b64


def _sign(private_key: Ed25519PrivateKey, message: str) -> str:
    return base64.b64encode(private_key.sign(message.encode("utf-8"))).decode("ascii")


# --- challenge + signature ------------------------------------------------------


def test_notify_challenge_binds_all_fields():
    base = web_push.notify_challenge("dev-1", "attention", 1750000000)
    assert base == "notify.dev-1.attention.1750000000"
    assert base != web_push.notify_challenge("dev-2", "attention", 1750000000)
    assert base != web_push.notify_challenge("dev-1", "other", 1750000000)
    assert base != web_push.notify_challenge("dev-1", "attention", 1750000001)


def test_notify_signature_verifies_against_device_identity_key():
    private_key, public_b64 = _ed25519_pair()
    challenge = web_push.notify_challenge("dev-1", "attention", 1750000000)
    signature = _sign(private_key, challenge)
    assert pairing_crypto.verify_registration(public_b64, challenge, signature)


def test_notify_signature_rejects_tampered_fields():
    private_key, public_b64 = _ed25519_pair()
    signature = _sign(
        private_key, web_push.notify_challenge("dev-1", "attention", 1750000000)
    )
    forged = web_push.notify_challenge("dev-1", "attention", 1750009999)
    assert not pairing_crypto.verify_registration(public_b64, forged, signature)


# --- freshness ------------------------------------------------------------------


def test_timestamp_fresh_within_window():
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    assert web_push.notify_timestamp_fresh(int(now.timestamp()), now)
    assert web_push.notify_timestamp_fresh(
        int((now - timedelta(minutes=4)).timestamp()), now
    )
    assert web_push.notify_timestamp_fresh(
        int((now + timedelta(minutes=1)).timestamp()), now
    )


def test_timestamp_stale_or_far_future_rejected():
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    assert not web_push.notify_timestamp_fresh(
        int((now - timedelta(minutes=6)).timestamp()), now
    )
    assert not web_push.notify_timestamp_fresh(
        int((now + timedelta(minutes=6)).timestamp()), now
    )


# --- the content-free payload -----------------------------------------------------


def test_attention_payload_is_content_free():
    payload = json.loads(web_push.attention_payload())
    assert payload == {"kind": "attention", "url": "/app/"}
    # Nothing else may ever ride along: no goal, no task id, no device name.
    assert set(payload.keys()) == {"kind", "url"}


# --- delivery + pruning -----------------------------------------------------------


class _FakePushError(web_push.WebPushException):
    def __init__(self, status_code: int):
        response = type("R", (), {"status_code": status_code})()
        super().__init__(f"push failed ({status_code})", response=response)


def _subscription(endpoint: str) -> dict:
    return {"endpoint": endpoint, "keys": {"p256dh": "pk", "auth": "ak"}}


def test_send_to_subscriptions_counts_and_prunes_gone_endpoints():
    sent = []

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims):
        if subscription_info["endpoint"] == "https://push/gone":
            raise _FakePushError(410)
        sent.append((subscription_info["endpoint"], data))

    delivered, gone = web_push.send_to_subscriptions(
        [_subscription("https://push/a"), _subscription("https://push/gone")],
        payload="{}",
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    assert delivered == 1
    assert gone == ["https://push/gone"]
    assert sent == [("https://push/a", "{}")]
    del delivered, gone


def test_send_to_subscriptions_transient_error_is_not_pruned():
    def fake_send(subscription_info, data, vapid_private_key, vapid_claims):
        raise _FakePushError(500)

    delivered, gone = web_push.send_to_subscriptions(
        [_subscription("https://push/a")],
        payload="{}",
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    assert delivered == 0
    assert gone == []


def test_subscription_gone_statuses():
    assert web_push.subscription_gone(404)
    assert web_push.subscription_gone(410)
    assert not web_push.subscription_gone(201)
    assert not web_push.subscription_gone(500)
    assert not web_push.subscription_gone(None)
