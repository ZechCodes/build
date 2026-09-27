"""Web-push unit tests: the notify challenge/signature contract with the bridge,
timestamp freshness, the content-free payload rule, and gone-subscription pruning.

E2EE invariant under test: nothing task-shaped may appear in a push payload —
the server only ever says "something needs your attention".
"""

from __future__ import annotations

import base64
import json
from datetime import datetime, timedelta, timezone

import requests
from cryptography.hazmat.primitives.asymmetric import ec
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
    base = web_push.notify_challenge("dev-1", "task-1", "attention", 1750000000)
    assert base == "notify.dev-1.task-1.attention.1750000000"
    assert base != web_push.notify_challenge("dev-2", "task-1", "attention", 1750000000)
    assert base != web_push.notify_challenge("dev-1", "task-2", "attention", 1750000000)
    assert base != web_push.notify_challenge("dev-1", "task-1", "other", 1750000000)
    assert base != web_push.notify_challenge("dev-1", "task-1", "attention", 1750000001)


def test_notify_signature_verifies_against_device_identity_key():
    private_key, public_b64 = _ed25519_pair()
    challenge = web_push.notify_challenge("dev-1", "task-1", "attention", 1750000000)
    signature = _sign(private_key, challenge)
    assert pairing_crypto.verify_registration(public_b64, challenge, signature)


def test_notify_signature_rejects_tampered_fields():
    private_key, public_b64 = _ed25519_pair()
    signature = _sign(
        private_key,
        web_push.notify_challenge("dev-1", "task-1", "attention", 1750000000),
    )
    forged = web_push.notify_challenge("dev-1", "task-1", "attention", 1750009999)
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


def test_push_payload_carries_only_id_kind_and_deep_link():
    payload = json.loads(web_push.push_payload("run-7", "agent"))
    assert payload == {
        "task_id": "run-7",
        "kind": "agent",
        "url": "/app/#/task/run-7",
    }
    # Exactly these three keys — no message, no title, no device name.
    assert set(payload.keys()) == {"task_id", "kind", "url"}


def test_push_kinds_are_the_two_the_unread_counter_has():
    # #191: a push fires only for what adds to the unread counter — an agent's
    # conversation and a watched task (a "task" to the user, #190). The old
    # plan/run state labels are refused.
    assert web_push.ALLOWED_KINDS == {"agent", "task"}


def test_each_kind_deep_links_to_what_it_names():
    # An agent's id is its conversation owner, which the SPA resolves by run id;
    # a task's is the tracker task's, which it resolves by task id.
    agent = json.loads(web_push.push_payload("run-1", "agent"))
    task = json.loads(web_push.push_payload("task-1", "task"))
    assert agent["url"] == "/app/#/task/run-1"
    assert task["url"] == "/app/#/tasks/task-1"


# --- delivery + pruning -----------------------------------------------------------


class _FakePushError(web_push.WebPushException):
    def __init__(self, status_code: int):
        response = type("R", (), {"status_code": status_code})()
        super().__init__(f"push failed ({status_code})", response=response)


def _subscription(endpoint: str) -> dict:
    return {"endpoint": endpoint, "keys": {"p256dh": "pk", "auth": "ak"}}


def test_send_to_subscriptions_counts_and_prunes_gone_endpoints():
    sent = []

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout):
        if subscription_info["endpoint"] == "https://fcm.googleapis.com/fcm/send/gone":
            raise _FakePushError(410)
        sent.append((subscription_info["endpoint"], data))

    delivered, gone = web_push.send_to_subscriptions(
        [
            _subscription("https://fcm.googleapis.com/fcm/send/a"),
            _subscription("https://fcm.googleapis.com/fcm/send/gone"),
        ],
        payload="{}",
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    assert delivered == 1
    assert gone == ["https://fcm.googleapis.com/fcm/send/gone"]
    assert sent == [("https://fcm.googleapis.com/fcm/send/a", "{}")]
    del delivered, gone


def test_send_to_subscriptions_transient_error_is_not_pruned():
    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout):
        raise _FakePushError(500)

    delivered, gone = web_push.send_to_subscriptions(
        [_subscription("https://fcm.googleapis.com/fcm/send/a")],
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


def test_send_passes_an_explicit_bounded_timeout():
    # pywebpush's default timeout is 10000 handed to requests as SECONDS —
    # a hanging push service would block the worker thread for hours.
    seen_timeouts = []

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout):
        seen_timeouts.append(timeout)

    web_push.send_to_subscriptions(
        [_subscription("https://fcm.googleapis.com/fcm/send/a")],
        payload="{}",
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    assert seen_timeouts == [web_push.PUSH_SEND_TIMEOUT_SECONDS]
    assert web_push.PUSH_SEND_TIMEOUT_SECONDS <= 30


def test_transport_errors_do_not_abort_the_remaining_subscriptions():
    # pywebpush does not wrap transport failures: a ConnectionError from one dead
    # endpoint must not 500 the whole notify or skip the other browsers.
    sent = []

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout):
        if subscription_info["endpoint"] == "https://fcm.googleapis.com/fcm/send/dead":
            raise requests.exceptions.ConnectionError("no route to push service")
        sent.append(subscription_info["endpoint"])

    delivered, gone = web_push.send_to_subscriptions(
        [
            _subscription("https://fcm.googleapis.com/fcm/send/dead"),
            _subscription("https://fcm.googleapis.com/fcm/send/b"),
        ],
        payload="{}",
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    assert delivered == 1
    assert gone == [], "a transport failure is transient, never a prune"
    assert sent == ["https://fcm.googleapis.com/fcm/send/b"]


# --- push service allowlist -----------------------------------------------------


ACCEPTED_ENDPOINTS = [
    "https://fcm.googleapis.com/fcm/send/abc:APA91b",
    "https://fcm.googleapis.com:443/fcm/send/abc",
    "https://jmt17.google.com/fcm/send/ctXj7rHd-Iw:APA91b",  # a real Chromium subscription
    "https://updates.push.services.mozilla.com/wpush/v2/gAAAA",
    "https://push.services.mozilla.com.example.push.services.mozilla.com/wpush/v2/x",
    "https://web.push.apple.com/QGu3",
    "https://wns2-par02p.notify.windows.com/w/?token=BQYAAA",
]

REJECTED_ENDPOINTS = [
    "",
    "http://fcm.googleapis.com/fcm/send/abc",  # not https
    "https://127.0.0.1/fcm/send/abc",
    "https://[::1]/fcm/send/abc",
    "https://169.254.169.254/latest/meta-data/",
    "https://10.0.0.1/",
    "https://2130706433/",  # an integer IPv4 spelling, and on no push service
    "https://fcm.googleapis.com.evil.example/fcm/send/abc",  # suffix is the attacker's
    "https://evil.example/fcm.googleapis.com",
    "https://evilfcm.googleapis.com/",  # an exact host, not a suffix
    "https://googleapis.com/",
    "https://google.com/",
    "https://jmt17.google.com.evil.example/",
    "https://evil.google.com/",  # only the one Google host, not the domain
    "https://notpush.apple.com/",  # no label boundary
    "https://push.apple.com/",  # the bare suffix is not a push host
    "https://apple.com.push.apple.com.evil.example/",
    "https://user:pass@fcm.googleapis.com/fcm/send/abc",
    "https://evil.example@fcm.googleapis.com/fcm/send/abc",
    "https://fcm.googleapis.com@evil.example/",
    "https://fcm.googleapis.com:8443/fcm/send/abc",
    "https://fcm.googleapis.com:80/",
    "https://fcm.googleapis.com./fcm/send/abc",  # trailing dot
    "https://fcm.googleapis.com\\@evil.example/",
    "https://fcm.googleapis.com /x",
    " https://fcm.googleapis.com/x",
    "https://fcm.googleapis.com\n.evil.example/",
    "https://fcm.googleapis.com:notaport/",
    "https://xn--fcm-googleapis-.com/",
    "https://evil.example%2f.push.apple.com/",  # not a hostname a push service has
    "https://evil.example\uff0f.push.apple.com/",
    "https://evil_example.push.apple.com/",
    "ftp://fcm.googleapis.com/",
    "//fcm.googleapis.com/fcm/send/abc",
]


def test_known_push_services_are_allowed():
    for endpoint in ACCEPTED_ENDPOINTS:
        assert web_push.push_endpoint_allowed(endpoint), endpoint


def test_anything_off_the_push_services_is_refused():
    for endpoint in REJECTED_ENDPOINTS:
        assert not web_push.push_endpoint_allowed(endpoint), endpoint


def test_nothing_is_sent_to_an_endpoint_off_the_push_services():
    # A row stored before the allowlist, or written around the api, is still
    # never a destination: the check runs again at send time.
    sent = []

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout):
        sent.append(subscription_info["endpoint"])

    delivered, gone = web_push.send_to_subscriptions(
        [
            _subscription("https://169.254.169.254/latest/meta-data/"),
            _subscription("https://fcm.googleapis.com.evil.example/x"),
            _subscription("https://fcm.googleapis.com/fcm/send/a"),
        ],
        payload="{}",
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    assert sent == ["https://fcm.googleapis.com/fcm/send/a"]
    assert delivered == 1
    assert gone == []


def test_a_delivery_follows_no_redirect(monkeypatch):
    # A push service answering 307 to an internal host must not carry the POST
    # there: the real sender's session is asked, and hands back the 3xx itself.
    asked = []

    class RedirectingAdapter(requests.adapters.BaseAdapter):
        def send(self, request, **kwargs):
            asked.append(request.url)
            response = requests.Response()
            response.status_code = 307
            response.headers["Location"] = "http://127.0.0.1:8080/internal"
            response.url = request.url
            response.request = request
            response._content = b""
            return response

        def close(self):
            pass

    real_request = requests.Session.request

    def mounted(self, *args, **kwargs):
        self.mount("https://", RedirectingAdapter())
        self.mount("http://", RedirectingAdapter())
        return real_request(self, *args, **kwargs)

    monkeypatch.setattr(requests.Session, "request", mounted)
    receiver = ec.generate_private_key(ec.SECP256R1())
    p256dh = base64.urlsafe_b64encode(
        receiver.public_key().public_bytes(
            Encoding.X962, PublicFormat.UncompressedPoint
        )
    ).rstrip(b"=").decode()
    auth = base64.urlsafe_b64encode(b"0123456789abcdef").rstrip(b"=").decode()
    vapid = ec.generate_private_key(ec.SECP256R1())
    vapid_private = base64.urlsafe_b64encode(
        vapid.private_numbers().private_value.to_bytes(32, "big")
    ).rstrip(b"=").decode()

    delivered, gone = web_push.send_to_subscriptions(
        [
            {
                "endpoint": "https://fcm.googleapis.com/fcm/send/a",
                "keys": {"p256dh": p256dh, "auth": auth},
            }
        ],
        payload="{}",
        vapid_private_key=vapid_private,
        vapid_subject="mailto:ops@getbuild.ing",
    )
    assert asked == ["https://fcm.googleapis.com/fcm/send/a"]
    assert (delivered, gone) == (0, [])


# --- notify replay guard -----------------------------------------------------------


def test_replay_guard_rejects_a_duplicate_notify_within_ttl():
    guard = web_push.NotifyReplayGuard()
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    assert guard.check_and_record("dev-1", 1750000000, "sigA", now)
    assert not guard.check_and_record("dev-1", 1750000000, "sigA", now)
    assert not guard.check_and_record(
        "dev-1", 1750000000, "sigA", now + timedelta(minutes=4)
    ), "still rejected while the signature could remain fresh"


def test_replay_guard_ttl_covers_the_full_freshness_window():
    # A notify stamped up to NOTIFY_FRESHNESS_WINDOW in the future stays fresh for
    # another full window: the guard must remember at least twice the window.
    guard = web_push.NotifyReplayGuard()
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    assert guard.check_and_record("dev-1", 1750000000, "sigA", now)
    within_validity = now + 2 * web_push.NOTIFY_FRESHNESS_WINDOW
    assert not guard.check_and_record("dev-1", 1750000000, "sigA", within_validity)
    past_validity = now + 2 * web_push.NOTIFY_FRESHNESS_WINDOW + timedelta(seconds=1)
    assert guard.check_and_record("dev-1", 1750000000, "sigA", past_validity)


def test_replay_guard_distinguishes_devices_timestamps_and_signatures():
    guard = web_push.NotifyReplayGuard()
    now = datetime(2026, 7, 1, 12, 0, tzinfo=timezone.utc)
    assert guard.check_and_record("dev-1", 1750000000, "sigA", now)
    assert guard.check_and_record("dev-2", 1750000000, "sigA", now)
    assert guard.check_and_record("dev-1", 1750000001, "sigA", now)
    assert guard.check_and_record("dev-1", 1750000000, "sigB", now)
