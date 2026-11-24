"""Fixed hosted routes; no URL fetch/proxy and no raw-token path parameters."""

import json
from contextlib import suppress
from uuid import UUID

from litestar import Controller, Request, delete, get, post
from litestar.di import Provide
from litestar.exceptions import ClientException, HTTPException
from litestar.response import Response
from sqlalchemy.ext.asyncio import AsyncSession

from buildapp import bridge_http_auth as auth
from buildapp import hosted_tunnel_claim
from buildapp.clock import utc_now
from buildapp.desktop_auth import bridge_http_client_binding, build_auth_guard
from buildapp.email_message import provide_public_base_url
from buildapp.request_body import require_json_object
from buildapp.session_auth import require_user


async def fields(request, allowed, maximum=18000):
    # A JSON-only cookie API cannot be invoked by cross-origin HTML forms.
    if request.content_type[0] != "application/json":
        raise ClientException("JSON required")
    raw = bytearray()
    async for chunk in request.stream():
        if len(raw) + len(chunk) > maximum:
            raise ClientException("Request too large", status_code=413)
        raw.extend(chunk)
    try:
        body = require_json_object(json.loads(raw))
    except (ValueError, UnicodeError, RecursionError):
        raise ClientException("Invalid JSON") from None
    if set(body) != set(allowed):
        raise ClientException("Invalid fields")
    return body


def response(payload, status_code=201):
    return Response(
        payload,
        status_code=status_code,
        headers={"Cache-Control": "no-store", "Referrer-Policy": "no-referrer"},
    )


HOSTED_CLAIM_ERRORS = {
    400: "invalid_request",
    401: "device_proof_refused",
    413: "request_too_large",
    429: "challenge_budget_exhausted",
}


async def hosted_claim_request(operation, device_id, request, db, issuer, success_status):
    status = 503
    try:
        body = await fields(request, ("proof",))
        payload = await operation(db, device_id, body["proof"], issuer, utc_now())
        await db.commit()
        return response(payload, success_status)
    except HTTPException as error:
        if error.status_code in HOSTED_CLAIM_ERRORS:
            status = error.status_code
    except Exception:
        # Materialization and database failures must never send or log their
        # exception details.
        pass
    with suppress(Exception):
        await db.rollback()
    return response({"error": HOSTED_CLAIM_ERRORS.get(status, "claim_unavailable")}, status)


async def discovery_challenge(request, db_session, public_base_url, device_id=None):
    body = await fields(request, ("origin",), 2200)
    try:
        payload = await auth.create_discovery(
            db_session,
            require_user(request),
            bridge_http_client_binding(request),
            body["origin"],
            public_base_url,
            utc_now(),
            device_id=device_id,
        )
    except ValueError:
        raise ClientException("Invalid HTTPS origin") from None
    await db_session.commit()
    return response(payload)


class BridgeHttpController(Controller):
    path = ""
    dependencies = {
        "public_base_url": Provide(provide_public_base_url, sync_to_thread=False)
    }

    @post("/api/devices/{device_id:uuid}/hosted-tunnel/challenge")
    async def hosted_challenge(
        self, device_id: UUID, request: Request, db_session: AsyncSession, public_base_url: str,
    ) -> Response:
        return await hosted_claim_request(
            hosted_tunnel_claim.issue_challenge, device_id, request, db_session, public_base_url, 201,
        )

    @post("/api/devices/{device_id:uuid}/hosted-tunnel", status_code=200)
    async def hosted_claim(
        self, device_id: UUID, request: Request, db_session: AsyncSession, public_base_url: str,
    ) -> Response:
        return await hosted_claim_request(
            hosted_tunnel_claim.claim_assignment, device_id, request, db_session, public_base_url, 200,
        )

    @post("/api/devices/{device_id:uuid}/http-endpoint/challenge")
    async def registration_challenge(
        self, device_id: UUID, request: Request, db_session: AsyncSession, public_base_url: str
    ) -> Response:
        body = await fields(request, ("proof",), 18000)
        payload = await auth.registration_challenge(
            db_session, device_id, body["proof"], public_base_url, utc_now()
        )
        await db_session.commit()
        return response(payload)

    @post("/api/devices/{device_id:uuid}/http-endpoint")
    async def register(
        self,
        device_id: UUID,
        request: Request,
        db_session: AsyncSession,
        public_base_url: str,
    ) -> Response:
        body = await fields(request, ("proof",))
        payload = await auth.register_endpoint(
            db_session, device_id, body["proof"], public_base_url, utc_now()
        )
        await db_session.commit()
        return response(payload)

    @post("/api/bridge-discovery/challenges", guards=[build_auth_guard])
    async def challenge(
        self, request: Request, db_session: AsyncSession, public_base_url: str
    ) -> Response:
        return await discovery_challenge(request, db_session, public_base_url)

    @post("/api/devices/{device_id:uuid}/http-discovery", guards=[build_auth_guard])
    async def saved_challenge(
        self, device_id: UUID, request: Request, db_session: AsyncSession, public_base_url: str
    ) -> Response:
        return await discovery_challenge(request, db_session, public_base_url, device_id)

    @get(
        "/api/bridge-discovery/challenges/{challenge_id:uuid}",
        guards=[build_auth_guard],
    )
    async def pending(
        self,
        challenge_id: UUID,
        request: Request,
        db_session: AsyncSession,
        public_base_url: str,
    ) -> Response:
        payload = await auth.pending_discovery(
            db_session,
            require_user(request),
            bridge_http_client_binding(request),
            challenge_id,
            public_base_url,
            utc_now(),
        )
        return response(payload, 200)

    @post("/api/bridge-discovery/complete", guards=[build_auth_guard])
    async def complete(
        self, request: Request, db_session: AsyncSession, public_base_url: str
    ) -> Response:
        body = await fields(request, ("challenge_id", "proof"))
        payload = await auth.complete_discovery(
            db_session,
            require_user(request),
            bridge_http_client_binding(request),
            body["challenge_id"],
            body["proof"],
            public_base_url,
            utc_now(),
        )
        await db_session.commit()
        return response(payload)

    @post("/api/devices/{device_id:uuid}/http-credentials", guards=[build_auth_guard])
    async def credentials(
        self,
        device_id: UUID,
        request: Request,
        db_session: AsyncSession,
        public_base_url: str,
    ) -> Response:
        body = await fields(request, ("family_id", "endpoint_epoch"), 2048)
        payload = await auth.renew(
            db_session,
            require_user(request),
            bridge_http_client_binding(request),
            device_id,
            body["family_id"],
            body["endpoint_epoch"],
            public_base_url,
            utc_now(),
        )
        await db_session.commit()
        return response(payload)

    @delete(
        "/api/devices/{device_id:uuid}/http-credentials/{family_id:uuid}",
        guards=[build_auth_guard],
        status_code=204,
    )
    async def revoke(
        self,
        device_id: UUID,
        family_id: UUID,
        request: Request,
        db_session: AsyncSession,
    ) -> None:
        await auth.revoke_family(
            db_session,
            require_user(request),
            bridge_http_client_binding(request),
            device_id,
            family_id,
            utc_now(),
        )
        await db_session.commit()

    @post("/api/bridge-http/logout", guards=[build_auth_guard], status_code=204)
    async def logout(self, request: Request, db_session: AsyncSession) -> None:
        await fields(request, (), 2048)
        await auth.revoke_client(
            db_session,
            require_user(request),
            bridge_http_client_binding(request),
            utc_now(),
        )
        await db_session.commit()

    @post("/api/bridge-http/introspect", status_code=200)
    async def introspect(
        self, request: Request, db_session: AsyncSession, public_base_url: str
    ) -> Response:
        body = await fields(request, ("token", "client_assertion"))
        payload = await auth.introspect(
            db_session,
            body["token"],
            body["client_assertion"],
            public_base_url,
            utc_now(),
        )
        await db_session.commit()
        return response(payload, 200)
