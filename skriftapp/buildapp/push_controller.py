"""Web-push endpoints — content-free attention notifications.

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

E2EE invariant: the pushed payload is always ``web_push.push_payload(task_id,
kind)`` — ``{"task_id", "kind", "url"}`` where ``task_id`` is opaque and ``kind``
a generic status label — never task content (goals/plan text).
"""

from __future__ import annotations

import asyncio
import os
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


# The freshness window alone leaves a captured signed notify replayable for its
# whole span (notification-spam). Remember recently seen (device, timestamp,
# signature) tuples and refuse duplicates — same scheme as the relay's device
# auth. In-process: the api runs single-replica (Recreate strategy).
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
        is updated in place — including when a different account logs in."""
        user_id = require_user(request)
        body = await read_json_object(request)
        try:
            endpoint = str(body["endpoint"])
            keys = body["keys"]
            p256dh_key = str(keys["p256dh"])
            auth_key = str(keys["auth"])
        except (KeyError, TypeError):
            raise ClientException("malformed subscription")
        if not endpoint.startswith("https://") or not p256dh_key or not auth_key:
            raise ClientException("malformed subscription")

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

    @post("/api/push/notify")
    async def notify(self, request: Request, db_session: AsyncSession) -> Response:
        """A bridge reports that a task needs its human. Pushes a content-free
        ``{task_id, kind, url}`` payload (opaque id + generic kind, deep-linking
        into ``/app/``) to every subscription of the device's owner. Authenticated
        by the device's Ed25519 signature over a timestamped challenge that binds
        the task and kind; a freshness window bounds replay."""
        body = await read_json_object(request)
        try:
            device_id = UUID(str(body["device_id"]))
            task_id = str(body["task_id"])
            kind = str(body["kind"])
            timestamp = int(body["timestamp"])
            signature = str(body["signature_b64"])
        except (KeyError, ValueError, TypeError):
            raise ClientException("malformed notify")
        if kind not in web_push.ALLOWED_KINDS:
            raise ClientException("unknown notify kind")

        device = await db_session.get(Device, device_id)
        if device is None or not device.approved or device.owner_user_id is None:
            raise NotAuthorizedException("notify not authorized")
        challenge = web_push.notify_challenge(str(device_id), task_id, kind, timestamp)
        if not pairing_crypto.verify_registration(
            device.identity_public_key_b64, challenge, signature
        ):
            raise NotAuthorizedException("notify signature invalid")
        if not web_push.notify_timestamp_fresh(timestamp, utc_now()):
            raise NotAuthorizedException("notify timestamp out of window")
        if not _notify_replay_guard.check_and_record(
            str(device_id), timestamp, signature, utc_now()
        ):
            raise NotAuthorizedException("notify replayed")

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
        if not subscriptions:
            return Response({"delivered": 0, "pruned": 0})

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
            subscription_infos,
            web_push.push_payload(task_id, kind),
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
        return Response({"delivered": delivered, "pruned": len(gone_endpoints)})
