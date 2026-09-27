"""Web-push endpoints — content-free unread notifications (#191).

Storage rides the framework: subscriptions live in Skrift's ``push_subscriptions``
table (``skrift.db.models.push_subscription``, created by a framework migration)
via ``skrift.push.save_subscription``. VAPID keys come from the environment
(``VAPID_PRIVATE_KEY``/``VAPID_PUBLIC_KEY``/``VAPID_SUBJECT`` — a k8s Secret in
production) so the key the browser subscribes with is exactly the key notify
signs with.

Two audiences, mirroring ``devices_controller``:
- **authenticated** (browser session/desktop OAuth): subscribe/unsubscribe this
  browser's push subscription, and read the VAPID public key to subscribe with.
- **public** (bridge-facing): ``/api/push/notify`` — no session; authenticated by
  an Ed25519 signature over a timestamped challenge, verified against the
  device's pinned identity key (same proof-of-key scheme as registration).

E2EE invariant (#200): the api forwards opaque ciphertext and sees only sids.
The pushed payload is ``web_push.push_payload(task_id, kind, blob)`` —
``{"task_id", "kind", "url"}`` where ``task_id`` is opaque and ``kind`` a generic
label, plus ``"sealed"`` when the bridge sealed a blob to that subscription. The
api matches a blob to a subscription by ``web_push.subscription_id(endpoint)``,
validates only its shape and forwards it byte-identical; it never decodes,
stores or logs a blob or a payload. Content authenticity rests on the
notification public key never reaching the api: the key goes from the browser to
the bridge over the E2EE session only, and without it no one can seal content
the browser will open.
"""

from __future__ import annotations

import asyncio
import os
from dataclasses import dataclass
from uuid import UUID

from litestar import Controller, Request, get, post
from litestar.exceptions import (
    ClientException,
    NotAuthorizedException,
    ServiceUnavailableException,
)
from litestar.response import Response
from sqlalchemy import delete, select
from sqlalchemy.ext.asyncio import AsyncSession

from skrift.db.models.push_subscription import PushSubscription
from skrift.push import save_subscription

from buildapp import pairing_crypto, web_push
from buildapp.clock import utc_now
from buildapp.desktop_auth import build_auth_guard
from buildapp.models import Device
from buildapp.request_body import read_json_object
from buildapp.session_auth import require_user

NOTIFY_ROUTE_PATH = "/api/push/notify"

# The freshness window alone leaves a captured signed notify replayable for its
# whole span (notification-spam). Remember recently seen (device, timestamp,
# signature) tuples and refuse duplicates — same scheme as the relay's device
# auth. In-process: the api runs one replica. A deploy starts the new pod with
# an empty guard, and for the few seconds both pods serve, a notify one pod has
# seen can still reach the other once (deploy/k8s/app.yaml, #168).
_notify_replay_guard = web_push.NotifyReplayGuard()


def _vapid_public_key() -> str:
    public_key = os.environ.get(web_push.VAPID_PUBLIC_KEY_ENV, "")
    if not public_key:
        raise ServiceUnavailableException("web push is not configured")
    return public_key


def _vapid_private_key() -> str:
    private_key = os.environ.get(web_push.VAPID_PRIVATE_KEY_ENV, "")
    if not private_key:
        raise ServiceUnavailableException("web push is not configured")
    return private_key


def _vapid_subject() -> str:
    return os.environ.get(web_push.VAPID_SUBJECT_ENV, "mailto:ops@getbuild.ing")


class PushController(Controller):
    """Push subscription registry + the bridge's notify entry point. Per-route
    guards (no class-level guard) so the signature-authenticated notify route
    sits alongside the session-authenticated ones."""

    path = ""

    # ----- authenticated (browser/session) -----------------------------------

    @get("/api/push/vapid-public-key", guards=[build_auth_guard])
    async def vapid_public_key(self, request: Request) -> Response:
        """The application server key the browser subscribes with."""
        require_user(request)
        return Response({"public_key": _vapid_public_key()})

    @post("/api/push/subscribe", guards=[build_auth_guard])
    async def subscribe(self, request: Request, db_session: AsyncSession) -> Response:
        """Store (or take over) this browser's push subscription for the current
        user. The endpoint is unique per browser+origin, so an existing row for it
        is updated in place — including when a different account logs in. Only an
        endpoint on a known push service is stored: the server POSTs to it later."""
        user_id = require_user(request)
        body = await read_json_object(request)
        try:
            endpoint = str(body["endpoint"])
            keys = body["keys"]
            p256dh_key = str(keys["p256dh"])
            auth_key = str(keys["auth"])
        except (KeyError, TypeError):
            raise ClientException("malformed subscription")
        if not p256dh_key or not auth_key:
            raise ClientException("malformed subscription")
        if not web_push.push_endpoint_allowed(endpoint):
            raise ClientException("push endpoint is not a known push service")

        await save_subscription(db_session, str(user_id), endpoint, p256dh_key, auth_key)
        return Response({"ok": True}, status_code=201)

    @post("/api/push/unsubscribe", guards=[build_auth_guard])
    async def unsubscribe(self, request: Request, db_session: AsyncSession) -> Response:
        """Remove this browser's subscription — only if the current user owns it."""
        user_id = require_user(request)
        body = await read_json_object(request)
        endpoint = str(body.get("endpoint", ""))
        if not endpoint:
            raise ClientException("endpoint required")
        await db_session.execute(
            delete(PushSubscription).where(
                PushSubscription.endpoint == endpoint,
                PushSubscription.user_id == str(user_id),
            )
        )
        await db_session.commit()
        return Response({"ok": True})

    # ----- public (bridge-facing, Ed25519-signed) ------------------------------

    @post(NOTIFY_ROUTE_PATH)
    async def notify(self, request: Request, db_session: AsyncSession) -> Response:
        """A bridge reports that something added to the unread counter. Pushes
        ``{task_id, kind, url}`` (opaque id + generic kind, deep-linking into
        ``/app/``) to every subscription of the device's owner, with that
        subscription's sealed blob when the bridge sent one. Authenticated by the
        device's Ed25519 signature over a timestamped challenge that binds the
        task, the kind and the sealed content; a freshness window bounds replay.
        Replies with the sealed sids that match no live subscription."""
        notify = _parse_notify(await read_json_object(request))
        device = await _authorized_device(db_session, notify)
        subscriptions = (
            (
                await db_session.execute(
                    select(PushSubscription).where(
                        PushSubscription.user_id == str(device.owner_user_id)
                    )
                )
            )
            .scalars()
            .all()
        )
        endpoints = [s.endpoint for s in subscriptions]
        if not subscriptions:
            return _notify_reply(0, [], notify, endpoints)

        subscription_infos = [
            {
                "endpoint": s.endpoint,
                "keys": {"p256dh": s.key_p256dh, "auth": s.key_auth},
            }
            for s in subscriptions
        ]
        vapid_private_key = _vapid_private_key()
        vapid_subject = _vapid_subject()
        # pywebpush is synchronous (requests); keep the event loop free.
        delivered, gone_endpoints = await asyncio.to_thread(
            web_push.send_to_subscriptions,
            web_push.sealed_deliveries(
                subscription_infos, notify.sealed, notify.task_id, notify.kind
            ),
            vapid_private_key,
            vapid_subject,
        )
        if gone_endpoints:
            await db_session.execute(
                delete(PushSubscription).where(
                    PushSubscription.endpoint.in_(gone_endpoints)
                )
            )
            await db_session.commit()
        return _notify_reply(delivered, gone_endpoints, notify, endpoints)


@dataclass(frozen=True)
class _Notify:
    """A notify request, parsed. ``sealed`` holds opaque blobs by sid."""

    device_id: UUID
    task_id: str
    kind: str
    timestamp: int
    signature: str
    sealed: tuple[web_push.SealedEntry, ...]

    @property
    def challenge(self) -> str:
        return web_push.notify_challenge(
            str(self.device_id), self.task_id, self.kind, self.timestamp, self.sealed
        )


def _parse_notify(body: dict) -> _Notify:
    """The notify's fields, or a 400 for a malformed request, an unknown kind or
    a ``sealed`` of any shape but the one ``web_push.parse_sealed`` accepts."""
    try:
        notify = _Notify(
            device_id=UUID(str(body["device_id"])),
            task_id=str(body["task_id"]),
            kind=str(body["kind"]),
            timestamp=int(body["timestamp"]),
            signature=str(body["signature_b64"]),
            sealed=web_push.parse_sealed(body.get("sealed", [])),
        )
    except (KeyError, ValueError, TypeError):
        raise ClientException("malformed notify")
    if notify.kind not in web_push.ALLOWED_KINDS:
        raise ClientException("unknown notify kind")
    return notify


async def _authorized_device(db_session: AsyncSession, notify: _Notify) -> Device:
    """The approved, owned device whose identity key signed this exact notify
    (v1 without ``sealed``, v2 over its digest with it), fresh and not replayed."""
    device = await db_session.get(Device, notify.device_id)
    if device is None or not device.approved or device.owner_user_id is None:
        raise NotAuthorizedException("notify not authorized")
    if not pairing_crypto.verify_registration(
        device.identity_public_key_b64, notify.challenge, notify.signature
    ):
        raise NotAuthorizedException("notify signature invalid")
    if not web_push.notify_timestamp_fresh(notify.timestamp, utc_now()):
        raise NotAuthorizedException("notify timestamp out of window")
    if not _notify_replay_guard.check_and_record(
        str(notify.device_id), notify.timestamp, notify.signature, utc_now()
    ):
        raise NotAuthorizedException("notify replayed")
    return device


def _notify_reply(
    delivered: int, gone_endpoints: list[str], notify: _Notify, endpoints: list[str]
) -> Response:
    live_endpoints = set(endpoints) - set(gone_endpoints)
    return Response(
        {
            "delivered": delivered,
            "pruned": len(gone_endpoints),
            "unknown_subscriptions": web_push.unknown_subscription_ids(
                notify.sealed, live_endpoints
            ),
        }
    )
