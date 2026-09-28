"""Web-push unit tests: the notify challenge/signature contract with the bridge,
timestamp freshness, the payload rule, gone-subscription pruning, and the shape
checks on sealed content (#200).

E2EE invariant under test: no cleartext content appears in a push payload or a
log. The api says "something needs your attention" and forwards, byte-identical
and never decoded, whatever ciphertext the bridge sealed to a subscription.
"""

from __future__ import annotations

import base64
import json
import logging
import re
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest
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


def _delivery(endpoint: str, payload: str = "{}") -> tuple[dict, str]:
    return (_subscription(endpoint), payload)


def test_send_to_subscriptions_counts_and_prunes_gone_endpoints():
    sent = []

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout, ttl):
        if subscription_info["endpoint"] == "https://fcm.googleapis.com/fcm/send/gone":
            raise _FakePushError(410)
        sent.append((subscription_info["endpoint"], data))

    delivered, gone = web_push.send_to_subscriptions(
        [
            _delivery("https://fcm.googleapis.com/fcm/send/a"),
            _delivery("https://fcm.googleapis.com/fcm/send/gone"),
        ],
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    assert delivered == 1
    assert gone == ["https://fcm.googleapis.com/fcm/send/gone"]
    assert sent == [("https://fcm.googleapis.com/fcm/send/a", "{}")]
    del delivered, gone


def test_send_to_subscriptions_transient_error_is_not_pruned():
    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout, ttl):
        raise _FakePushError(500)

    delivered, gone = web_push.send_to_subscriptions(
        [_delivery("https://fcm.googleapis.com/fcm/send/a")],
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

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout, ttl):
        seen_timeouts.append(timeout)

    web_push.send_to_subscriptions(
        [_delivery("https://fcm.googleapis.com/fcm/send/a")],
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

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout, ttl):
        if subscription_info["endpoint"] == "https://fcm.googleapis.com/fcm/send/dead":
            raise requests.exceptions.ConnectionError("no route to push service")
        sent.append(subscription_info["endpoint"])

    delivered, gone = web_push.send_to_subscriptions(
        [
            _delivery("https://fcm.googleapis.com/fcm/send/dead"),
            _delivery("https://fcm.googleapis.com/fcm/send/b"),
        ],
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

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout, ttl):
        sent.append(subscription_info["endpoint"])

    delivered, gone = web_push.send_to_subscriptions(
        [
            _delivery("https://169.254.169.254/latest/meta-data/"),
            _delivery("https://fcm.googleapis.com.evil.example/x"),
            _delivery("https://fcm.googleapis.com/fcm/send/a"),
        ],
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    assert sent == ["https://fcm.googleapis.com/fcm/send/a"]
    assert delivered == 1
    assert gone == []


def _real_subscription_and_vapid(endpoint: str) -> tuple[dict, str]:
    """A subscription with real keys and a VAPID key, so the real sender
    (pywebpush over requests) runs end to end up to the transport adapter."""
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
    return {"endpoint": endpoint, "keys": {"p256dh": p256dh, "auth": auth}}, vapid_private


def _mount_adapter(monkeypatch, adapter: requests.adapters.BaseAdapter) -> None:
    real_request = requests.Session.request

    def mounted(self, *args, **kwargs):
        self.mount("https://", adapter)
        self.mount("http://", adapter)
        return real_request(self, *args, **kwargs)

    monkeypatch.setattr(requests.Session, "request", mounted)


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

    _mount_adapter(monkeypatch, RedirectingAdapter())
    subscription_info, vapid_private = _real_subscription_and_vapid(
        "https://fcm.googleapis.com/fcm/send/a"
    )

    delivered, gone = web_push.send_to_subscriptions(
        [(subscription_info, "{}")],
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


# --- sealed content (#200): the api forwards ciphertext it never opens ---------

FIXTURES = Path(__file__).resolve().parents[2] / "fixtures" / "push"
SID = "fktz0HaQPrTBONQORgaYdrkQLm41LU5PVktHaqho6To"
OTHER_SID = "x" * 43


def _fixture(name: str) -> dict:
    return json.loads((FIXTURES / name).read_text())


def test_notify_challenge_v2_equals_the_fixture():
    vector = _fixture("notify-challenge-v2.json")
    sealed = web_push.parse_sealed(vector["sealed"])
    challenge = web_push.notify_challenge(
        vector["device_id"],
        vector["entity_id"],
        vector["kind"],
        vector["timestamp"],
        sealed,
    )
    assert challenge == vector["challenge"]


def test_notify_challenge_without_sealed_is_the_191_challenge():
    assert web_push.notify_challenge("dev-1", "task-1", "task", 1790000000, ()) == (
        "notify.dev-1.task-1.task.1790000000"
    )


def test_notify_challenge_v2_binds_order_sids_and_blobs():
    first = web_push.SealedEntry(SID, "AQID")
    second = web_push.SealedEntry(OTHER_SID, "BAUG")
    base = web_push.notify_challenge("d", "t", "task", 1, (first, second))
    assert base != web_push.notify_challenge("d", "t", "task", 1, (second, first))
    assert base != web_push.notify_challenge(
        "d", "t", "task", 1, (first, web_push.SealedEntry(OTHER_SID, "BAUH"))
    )
    assert base != web_push.notify_challenge("d", "t", "task", 1, (first,))


def test_subscription_id_matches_the_sealed_fixture_pair():
    vector = _fixture("sealed-v1.json")
    assert web_push.subscription_id(vector["endpoint"]) == vector["subscription_id"]


def test_push_payload_carries_a_sealed_blob_verbatim():
    blob = _fixture("sealed-v1.json")["blob"]
    payload = json.loads(web_push.push_payload("run-7", "agent", blob))
    assert payload == {
        "task_id": "run-7",
        "kind": "agent",
        "url": "/app/#/task/run-7",
        "sealed": blob,
    }


def test_parse_sealed_accepts_the_fixture_entries_in_order():
    vector = _fixture("notify-challenge-v2.json")
    assert web_push.parse_sealed(vector["sealed"]) == (
        web_push.SealedEntry(SID, "AQID"),
        web_push.SealedEntry(OTHER_SID, "BAUG"),
    )
    assert web_push.parse_sealed([]) == ()


def test_parse_sealed_accepts_the_limits_exactly():
    at_limits = [
        {"subscription_id": f"{index:043d}", "blob": "A" * web_push.MAX_SEALED_BLOB_CHARS}
        for index in range(web_push.MAX_SEALED_ENTRIES)
    ]
    assert len(web_push.parse_sealed(at_limits)) == web_push.MAX_SEALED_ENTRIES
    assert web_push.parse_sealed([{"subscription_id": SID, "blob": "A"}])


def _entry(**overrides) -> dict:
    return {"subscription_id": SID, "blob": "AQID", **overrides}


BAD_SEALED_SHAPES = {
    "not a list": {"subscription_id": SID, "blob": "AQID"},
    "a string": "AQID",
    "null": None,
    "too many entries": [
        _entry(subscription_id=f"{index:043d}")
        for index in range(web_push.MAX_SEALED_ENTRIES + 1)
    ],
    "an entry that is not an object": ["AQID"],
    "an extra key": [_entry(kind="task")],
    "a missing blob": [{"subscription_id": SID}],
    "a missing sid": [{"blob": "AQID"}],
    "an empty blob": [_entry(blob="")],
    "a long blob": [_entry(blob="A" * (web_push.MAX_SEALED_BLOB_CHARS + 1))],
    "padding in the blob": [_entry(blob="AQI=")],
    "standard base64 in the blob": [_entry(blob="AQ+/")],
    "whitespace in the blob": [_entry(blob="AQ ID")],
    "non-ascii in the blob": [_entry(blob="AQIé")],
    "a blob that is not a string": [_entry(blob=12)],
    "a short sid": [_entry(subscription_id=SID[:-1])],
    "a long sid": [_entry(subscription_id=SID + "A")],
    "a bad sid charset": [_entry(subscription_id=SID[:-1] + "=")],
    "a sid that is not a string": [_entry(subscription_id=None)],
    "a repeated sid": [_entry(), _entry(blob="BAUG")],
}


@pytest.mark.parametrize("shape", BAD_SEALED_SHAPES.values(), ids=BAD_SEALED_SHAPES.keys())
def test_parse_sealed_refuses_every_bad_shape(shape):
    with pytest.raises(ValueError):
        web_push.parse_sealed(shape)


def test_push_ttl_is_a_plain_integer_line_the_service_worker_can_mirror():
    source = Path(web_push.__file__).read_text()
    match = re.search(r"^PUSH_TTL_SECONDS = (\d+)$", source, re.MULTILINE)
    assert match is not None
    assert int(match.group(1)) == web_push.PUSH_TTL_SECONDS == 0


def test_send_passes_the_push_ttl_on_every_delivery():
    seen_ttls = []

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout, ttl):
        seen_ttls.append(ttl)

    web_push.send_to_subscriptions(
        [
            _delivery("https://fcm.googleapis.com/fcm/send/a"),
            _delivery("https://fcm.googleapis.com/fcm/send/b"),
        ],
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    assert seen_ttls == [web_push.PUSH_TTL_SECONDS] * 2


def test_each_subscription_is_sent_its_own_payload():
    sent = []

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout, ttl):
        sent.append((subscription_info["endpoint"], data))

    web_push.send_to_subscriptions(
        [
            _delivery("https://fcm.googleapis.com/fcm/send/a", "payload-a"),
            _delivery("https://fcm.googleapis.com/fcm/send/b", "payload-b"),
        ],
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    assert sent == [
        ("https://fcm.googleapis.com/fcm/send/a", "payload-a"),
        ("https://fcm.googleapis.com/fcm/send/b", "payload-b"),
    ]


def test_sealed_deliveries_match_blobs_by_subscription_id():
    matched = "https://fcm.googleapis.com/fcm/send/fixture-endpoint"
    unmatched = "https://fcm.googleapis.com/fcm/send/other"
    blob = _fixture("sealed-v1.json")["blob"]
    deliveries = web_push.sealed_deliveries(
        [_subscription(matched), _subscription(unmatched)],
        (web_push.SealedEntry(SID, blob),),
        "run-7",
        "agent",
    )
    assert deliveries == [
        (_subscription(matched), web_push.push_payload("run-7", "agent", blob)),
        (_subscription(unmatched), web_push.push_payload("run-7", "agent")),
    ]


def test_unknown_subscription_ids_are_the_sealed_sids_no_live_endpoint_has():
    live = ["https://fcm.googleapis.com/fcm/send/fixture-endpoint"]
    sealed = (
        web_push.SealedEntry(OTHER_SID, "BAUG"),
        web_push.SealedEntry(SID, "AQID"),
    )
    assert web_push.unknown_subscription_ids(sealed, live) == [OTHER_SID]
    assert web_push.unknown_subscription_ids(sealed, []) == [OTHER_SID, SID]
    assert web_push.unknown_subscription_ids((), live) == []


# --- nothing a push carries reaches a log --------------------------------------

BLOB_MARKER = "SEALEDBLOBMARKER" + "A" * 40
PAYLOAD_MARKER = web_push.push_payload("run-7", "agent", BLOB_MARKER)


def _assert_logs_carry_no_payload(caplog) -> None:
    logged = "\n".join(
        f"{record.getMessage()} {record.exc_text or ''}" for record in caplog.records
    )
    assert BLOB_MARKER not in logged
    assert "run-7" not in logged


@pytest.mark.parametrize(
    "failure",
    [
        requests.exceptions.ConnectionError(f"could not send {PAYLOAD_MARKER}"),
        web_push.WebPushException(f"Push failed: 500\nResponse body:{PAYLOAD_MARKER}"),
        _FakePushError(410),
    ],
    ids=["transport error", "push service error", "gone"],
)
def test_a_delivery_failure_never_logs_the_blob_or_payload(caplog, failure):
    caplog.set_level(logging.DEBUG)

    def fake_send(subscription_info, data, vapid_private_key, vapid_claims, timeout, ttl):
        raise failure

    web_push.send_to_subscriptions(
        [_delivery("https://fcm.googleapis.com/fcm/send/a", PAYLOAD_MARKER)],
        vapid_private_key="priv",
        vapid_subject="mailto:ops@getbuild.ing",
        send=fake_send,
    )
    _assert_logs_carry_no_payload(caplog)


class _EchoingPushService(requests.adapters.BaseAdapter):
    """A push service that fails the delivery, either by transport error or by a
    500 whose body echoes the request (so a logged response would leak it)."""

    def __init__(self, transport_error: bool):
        super().__init__()
        self.transport_error = transport_error

    def send(self, request, **kwargs):
        if self.transport_error:
            raise requests.exceptions.ConnectionError(f"{request.url} {request.body!r}")
        response = requests.Response()
        response.status_code = 500
        response.url = request.url
        response.request = request
        response._content = request.body
        return response

    def close(self):
        pass


@pytest.mark.parametrize("transport_error", [True, False], ids=["transport", "500"])
def test_the_real_sender_never_logs_the_blob_or_payload(caplog, monkeypatch, transport_error):
    caplog.set_level(logging.DEBUG)
    _mount_adapter(monkeypatch, _EchoingPushService(transport_error))
    subscription_info, vapid_private = _real_subscription_and_vapid(
        "https://fcm.googleapis.com/fcm/send/a"
    )

    delivered, gone = web_push.send_to_subscriptions(
        [(subscription_info, PAYLOAD_MARKER)],
        vapid_private_key=vapid_private,
        vapid_subject="mailto:ops@getbuild.ing",
    )
    assert (delivered, gone) == (0, [])
    _assert_logs_carry_no_payload(caplog)
