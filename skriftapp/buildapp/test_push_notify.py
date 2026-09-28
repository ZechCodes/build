"""The bridge's notify route over HTTP (#191, #200): a signed notify pushes to
every subscription of the device's owner, and a sealed one forwards each
subscription's ciphertext byte-identical without ever opening it.

The api sees only sids and opaque blobs: it matches a blob to a subscription by
``subscription_id(endpoint)``, validates the blob's shape and nothing else, and
reports the sealed sids that match no live subscription so the bridge can drop
their keys.
"""

from __future__ import annotations

import base64
import json
import logging
from collections.abc import Iterator
from datetime import datetime, timezone
from pathlib import Path
from uuid import UUID, uuid4

import pytest
import requests
from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat
from litestar.status_codes import (
    HTTP_200_OK,
    HTTP_201_CREATED,
    HTTP_400_BAD_REQUEST,
    HTTP_401_UNAUTHORIZED,
)
from litestar.testing import TestClient
from skrift.db.models.push_subscription import PushSubscription
from sqlalchemy import select
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import web_push
from buildapp.db_test_support import add_member, asgi_app, in_memory_session_maker
from buildapp.models import Device
from buildapp.push_controller import NOTIFY_ROUTE_PATH, PushController
from buildapp.test_web_push import BAD_SEALED_SHAPES, _FakePushError

USER = uuid4()
FIXTURE = json.loads(
    (Path(__file__).resolve().parents[2] / "fixtures" / "push" / "sealed-v1.json").read_text()
)
MATCHED_ENDPOINT = FIXTURE["endpoint"]
MATCHED_SID = FIXTURE["subscription_id"]
UNMATCHED_ENDPOINT = "https://fcm.googleapis.com/fcm/send/no-key-registered"
GONE_ENDPOINT = "https://fcm.googleapis.com/fcm/send/gone"


class PushService:
    """Stands in for the sender: records what each endpoint was sent, and fails
    the endpoints it is told to."""

    def __init__(self):
        self.sent: dict[str, str] = {}
        self.failures: dict[str, Exception] = {}

    def send(self, subscription_info, data, vapid_private_key, vapid_claims, timeout, ttl):
        endpoint = subscription_info["endpoint"]
        if endpoint in self.failures:
            raise self.failures[endpoint]
        self.sent[endpoint] = data

    def payload(self, endpoint: str) -> dict:
        return json.loads(self.sent[endpoint])


async def _seed_membership(session: AsyncSession) -> None:
    await add_member(session, USER)


@pytest.fixture()
def push_service(monkeypatch) -> PushService:
    service = PushService()
    real_send_to_subscriptions = web_push.send_to_subscriptions

    def send_through_the_fake(deliveries, vapid_private_key, vapid_subject):
        return real_send_to_subscriptions(
            deliveries, vapid_private_key, vapid_subject, send=service.send
        )

    monkeypatch.setattr(web_push, "send_to_subscriptions", send_through_the_fake)
    monkeypatch.setenv(web_push.VAPID_PRIVATE_KEY_ENV, "vapid-private")
    return service


@pytest.fixture()
def client(push_service) -> Iterator[TestClient]:
    app = asgi_app(
        [PushController],
        session_maker=in_memory_session_maker(),
        seed=_seed_membership,
    )
    with TestClient(app=app) as test_client:
        yield test_client


class Bridge:
    """A paired device and the key it signs notifies with."""

    def __init__(self, device_id: UUID, private_key: Ed25519PrivateKey):
        self.device_id = device_id
        self.private_key = private_key

    def notify(self, sealed: list[dict] | None = None, *, signed_sealed=None) -> dict:
        """A notify for ``task-1``; ``signed_sealed`` signs different entries than
        the ones sent, as a tampering relay would."""
        timestamp = int(datetime.now(timezone.utc).timestamp())
        signed = sealed if signed_sealed is None else signed_sealed
        challenge = web_push.notify_challenge(
            str(self.device_id),
            "task-1",
            "task",
            timestamp,
            web_push.parse_sealed(signed or []),
        )
        body = {
            "device_id": str(self.device_id),
            "task_id": "task-1",
            "kind": "task",
            "timestamp": timestamp,
            "signature_b64": base64.b64encode(
                self.private_key.sign(challenge.encode("utf-8"))
            ).decode("ascii"),
        }
        if sealed is not None:
            body["sealed"] = sealed
        return body


def _run(client: TestClient, work) -> object:
    with client.portal() as portal:
        return portal.call(work)


def paired_bridge(client: TestClient) -> Bridge:
    private_key = Ed25519PrivateKey.generate()
    device = Device(
        id=uuid4(),
        name="laptop",
        owner_user_id=USER,
        identity_public_key_b64=base64.b64encode(
            private_key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
        ).decode("ascii"),
        transport_public_key_b64="x25519-public",
        approved=True,
    )

    async def store() -> None:
        async with client.app.state.make_session() as session:
            session.add(device)
            await session.commit()

    _run(client, store)
    return Bridge(device.id, private_key)


def subscribe(client: TestClient, *endpoints: str) -> None:
    async def store() -> None:
        async with client.app.state.make_session() as session:
            for endpoint in endpoints:
                session.add(
                    PushSubscription(
                        user_id=str(USER), endpoint=endpoint, key_p256dh="pk", key_auth="ak"
                    )
                )
            await session.commit()

    _run(client, store)


def stored_endpoints(client: TestClient) -> list[str]:
    async def read() -> list[str]:
        async with client.app.state.make_session() as session:
            rows = (await session.execute(select(PushSubscription))).scalars().all()
            return sorted(row.endpoint for row in rows)

    return _run(client, read)


def _column_values(client: TestClient) -> str:
    """Every value in every subscription row, to prove no blob was stored."""

    async def read() -> str:
        async with client.app.state.make_session() as session:
            rows = (await session.execute(select(PushSubscription))).scalars().all()
            return " ".join(str(vars(row)) for row in rows)

    return _run(client, read)


def test_a_sealed_notify_forwards_the_blob_byte_identical_to_its_subscription(
    client, push_service
):
    bridge = paired_bridge(client)
    subscribe(client, MATCHED_ENDPOINT, UNMATCHED_ENDPOINT)

    response = client.post(
        NOTIFY_ROUTE_PATH,
        json=bridge.notify([{"subscription_id": MATCHED_SID, "blob": FIXTURE["blob"]}]),
    )

    assert response.status_code == HTTP_201_CREATED
    assert response.json() == {"delivered": 2, "pruned": 0, "unknown_subscriptions": []}
    assert push_service.payload(MATCHED_ENDPOINT) == {
        "task_id": "task-1",
        "kind": "task",
        "url": "/app/#/tasks/task-1",
        "sealed": FIXTURE["blob"],
    }
    assert push_service.sent[MATCHED_ENDPOINT] == web_push.push_payload(
        "task-1", "task", FIXTURE["blob"]
    )
    assert push_service.payload(UNMATCHED_ENDPOINT) == {
        "task_id": "task-1",
        "kind": "task",
        "url": "/app/#/tasks/task-1",
    }
    assert FIXTURE["blob"] not in _column_values(client)


def test_a_blob_that_is_not_ciphertext_is_forwarded_unchanged(client, push_service):
    # Valid base64url, but not a version-1 blob: no version byte, no point, no
    # tag. Only something that never decodes it can forward it.
    garbage = base64.urlsafe_b64encode(b"\xff" * 30 + b"not a sealed blob").rstrip(b"=")
    garbage_blob = garbage.decode("ascii")
    bridge = paired_bridge(client)
    subscribe(client, MATCHED_ENDPOINT)

    response = client.post(
        NOTIFY_ROUTE_PATH,
        json=bridge.notify([{"subscription_id": MATCHED_SID, "blob": garbage_blob}]),
    )

    assert response.status_code == HTTP_201_CREATED
    assert push_service.payload(MATCHED_ENDPOINT)["sealed"] == garbage_blob


def test_sealed_sids_matching_no_live_subscription_are_reported_unknown(
    client, push_service
):
    gone_sid = web_push.subscription_id(GONE_ENDPOINT)
    stranger_sid = web_push.subscription_id("https://fcm.googleapis.com/fcm/send/elsewhere")
    push_service.failures[GONE_ENDPOINT] = _FakePushError(410)
    bridge = paired_bridge(client)
    subscribe(client, MATCHED_ENDPOINT, GONE_ENDPOINT)

    response = client.post(
        NOTIFY_ROUTE_PATH,
        json=bridge.notify(
            [
                {"subscription_id": stranger_sid, "blob": "AQID"},
                {"subscription_id": MATCHED_SID, "blob": "BAUG"},
                {"subscription_id": gone_sid, "blob": "BwgJ"},
            ]
        ),
    )

    assert response.json() == {
        "delivered": 1,
        "pruned": 1,
        "unknown_subscriptions": [stranger_sid, gone_sid],
    }
    assert stored_endpoints(client) == [MATCHED_ENDPOINT]


def test_with_no_subscriptions_every_sealed_sid_is_unknown(client, push_service):
    bridge = paired_bridge(client)
    response = client.post(
        NOTIFY_ROUTE_PATH,
        json=bridge.notify([{"subscription_id": MATCHED_SID, "blob": "AQID"}]),
    )
    assert response.json() == {
        "delivered": 0,
        "pruned": 0,
        "unknown_subscriptions": [MATCHED_SID],
    }


@pytest.mark.parametrize("shape", BAD_SEALED_SHAPES.values(), ids=BAD_SEALED_SHAPES.keys())
def test_a_badly_shaped_sealed_is_refused(client, push_service, shape):
    bridge = paired_bridge(client)
    subscribe(client, MATCHED_ENDPOINT)
    body = bridge.notify()
    body["sealed"] = shape

    response = client.post(NOTIFY_ROUTE_PATH, json=body)

    assert response.status_code == HTTP_400_BAD_REQUEST
    assert push_service.sent == {}


def test_a_v2_signature_over_a_different_sealed_is_refused(client, push_service):
    bridge = paired_bridge(client)
    subscribe(client, MATCHED_ENDPOINT)
    signed = [{"subscription_id": MATCHED_SID, "blob": "AQID"}]
    tampered = [{"subscription_id": MATCHED_SID, "blob": "AQIE"}]

    response = client.post(NOTIFY_ROUTE_PATH, json=bridge.notify(tampered, signed_sealed=signed))

    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert push_service.sent == {}


def test_a_v1_signature_cannot_carry_sealed_content(client, push_service):
    bridge = paired_bridge(client)
    subscribe(client, MATCHED_ENDPOINT)
    added = [{"subscription_id": MATCHED_SID, "blob": "AQID"}]

    response = client.post(NOTIFY_ROUTE_PATH, json=bridge.notify(added, signed_sealed=[]))

    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert push_service.sent == {}


def test_a_v1_notify_without_sealed_still_pushes_the_191_payload(client, push_service):
    bridge = paired_bridge(client)
    subscribe(client, MATCHED_ENDPOINT)

    response = client.post(NOTIFY_ROUTE_PATH, json=bridge.notify())

    assert response.status_code == HTTP_201_CREATED
    assert response.json() == {"delivered": 1, "pruned": 0, "unknown_subscriptions": []}
    assert push_service.sent[MATCHED_ENDPOINT] == web_push.push_payload("task-1", "task")


def test_a_failed_sealed_delivery_logs_neither_blob_nor_payload(
    client, push_service, caplog
):
    caplog.set_level(logging.DEBUG)
    blob = FIXTURE["blob"]
    push_service.failures[MATCHED_ENDPOINT] = requests.exceptions.ConnectionError(
        f"lost {web_push.push_payload('task-1', 'task', blob)}"
    )
    push_service.failures[UNMATCHED_ENDPOINT] = web_push.WebPushException(
        f"Push failed: 500\nResponse body:{blob}"
    )
    bridge = paired_bridge(client)
    subscribe(client, MATCHED_ENDPOINT, UNMATCHED_ENDPOINT)

    response = client.post(
        NOTIFY_ROUTE_PATH,
        json=bridge.notify([{"subscription_id": MATCHED_SID, "blob": blob}]),
    )

    assert response.status_code == HTTP_201_CREATED
    logged = "\n".join(
        f"{record.getMessage()} {record.exc_text or ''}" for record in caplog.records
    )
    assert blob not in logged
    assert '"sealed"' not in logged


def test_the_notify_reply_is_ok_even_for_zero_deliveries(client, push_service):
    # Unchanged from #191: a notify with no subscriptions is a success, not an error.
    bridge = paired_bridge(client)
    response = client.post(NOTIFY_ROUTE_PATH, json=bridge.notify())
    assert response.status_code in (HTTP_200_OK, HTTP_201_CREATED)
    assert response.json()["delivered"] == 0


def test_a_replayed_sealed_notify_is_refused(client, push_service):
    bridge = paired_bridge(client)
    subscribe(client, MATCHED_ENDPOINT)
    body = bridge.notify([{"subscription_id": MATCHED_SID, "blob": "AQID"}])

    assert client.post(NOTIFY_ROUTE_PATH, json=body).status_code == HTTP_201_CREATED
    assert client.post(NOTIFY_ROUTE_PATH, json=body).status_code == HTTP_401_UNAUTHORIZED
