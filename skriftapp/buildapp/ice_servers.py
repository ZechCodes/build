"""ICE servers for the browser's WebRTC peer connection to its bridge.

The Cloudflare TURN key is a long-lived secret held only by the api (the
``build-app`` Secret). It mints short-lived credentials scoped to one user's own
device pair; the browser forwards the resulting list to the bridge inside the
sealed E2EE session, so the key never reaches a browser or a bridge and the relay
never sees a credential.

With no key configured — local dev — the list is STUN-only and direct host
candidates carry localhost sessions.

Pure logic plus one injectable sender, mirroring ``web_push``: no framework and no
DB imports, so a test needs no network.
"""

from __future__ import annotations

from copy import deepcopy

import requests

__all__ = [
    "CF_TURN_KEY_API_TOKEN_ENV",
    "CF_TURN_KEY_ID_ENV",
    "STUN_ONLY",
    "TTL_SECONDS",
    "IceServersUnavailable",
    "ice_servers",
]

CF_TURN_KEY_ID_ENV = "CF_TURN_KEY_ID"
CF_TURN_KEY_API_TOKEN_ENV = "CF_TURN_KEY_API_TOKEN"

# How long a minted credential lives. The one home of this number: a session that
# outlives it does an ICE restart with a freshly fetched list.
TTL_SECONDS = 86400

CREDENTIALS_URL = (
    "https://rtc.live.cloudflare.com/v1/turn/keys/{key_id}"
    "/credentials/generate-ice-servers"
)

# One hung Cloudflare request must not hold a worker thread open indefinitely.
REQUEST_TIMEOUT_SECONDS = 10

STUN_ONLY = [{"urls": ["stun:stun.cloudflare.com:3478"]}]

CREDENTIALS_CREATED = 201


class IceServersUnavailable(RuntimeError):
    """Cloudflare could not mint credentials. Never swallowed: a silent fallback
    to STUN would look like a working peer connection until the first client
    that cannot hole-punch."""


def ice_servers(
    key_id: str,
    api_token: str,
    ttl_seconds: int = TTL_SECONDS,
    send=requests.post,
) -> list[dict]:
    """Cloudflare's ``iceServers`` array verbatim, or the STUN-only list when no
    TURN key is configured."""
    if not key_id or not api_token:
        return deepcopy(STUN_ONLY)

    try:
        response = send(
            CREDENTIALS_URL.format(key_id=key_id),
            json={"ttl": ttl_seconds},
            headers={"Authorization": f"Bearer {api_token}"},
            timeout=REQUEST_TIMEOUT_SECONDS,
        )
    except requests.exceptions.RequestException as exc:
        raise IceServersUnavailable("cloudflare is unreachable") from exc
    if response.status_code != CREDENTIALS_CREATED:
        raise IceServersUnavailable(
            f"cloudflare answered {response.status_code} for TURN credentials"
        )
    try:
        return response.json()["iceServers"]
    except (KeyError, TypeError, ValueError) as exc:
        raise IceServersUnavailable(
            "cloudflare returned no iceServers array"
        ) from exc
