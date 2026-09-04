"""The ICE servers a browser opens its WebRTC peer connection to its bridge with.

One route, session-authenticated by the reusable ``auth_guard``, answering
``{"iceServers": [...]}``. The browser fetches it once per peer connection and
forwards the list to the bridge inside the sealed E2EE session, so the bridge
needs no Cloudflare access and the relay never sees a credential.

``CF_TURN_KEY_ID`` / ``CF_TURN_KEY_API_TOKEN`` are read from the environment (the
``build-app`` Secret) and never leave this process; only the short-lived
credentials Cloudflare mints from them are returned. Sibling of
``PushController``.
"""

from __future__ import annotations

import asyncio
import os

from litestar import Controller, Request, post
from litestar.exceptions import HTTPException
from litestar.response import Response
from litestar.status_codes import HTTP_502_BAD_GATEWAY

from skrift.auth.guards import auth_guard

from buildapp import ice_servers
from buildapp.session_auth import require_user

ICE_SERVERS_ROUTE_PATH = "/api/rtc/ice-servers"


class RtcController(Controller):
    """WebRTC's one api surface: the ICE servers a peer connection needs."""

    path = ""

    @post(ICE_SERVERS_ROUTE_PATH, guards=[auth_guard])
    async def mint_ice_servers(self, request: Request) -> Response:
        """Freshly minted TURN credentials for this user, or the STUN-only list
        when no TURN key is configured. A Cloudflare failure is a 502: quietly
        answering STUN would look like a working peer connection until the first
        client that cannot hole-punch."""
        require_user(request)
        key_id = os.environ.get(ice_servers.CF_TURN_KEY_ID_ENV, "")
        api_token = os.environ.get(ice_servers.CF_TURN_KEY_API_TOKEN_ENV, "")
        try:
            # requests is synchronous; keep the event loop free.
            servers = await asyncio.to_thread(ice_servers.ice_servers, key_id, api_token)
        except ice_servers.IceServersUnavailable as exc:
            raise HTTPException(
                status_code=HTTP_502_BAD_GATEWAY, detail=str(exc)
            ) from exc
        return Response({"iceServers": servers})
