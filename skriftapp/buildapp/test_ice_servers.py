"""ICE-server minting: the Cloudflare TURN call, the no-key STUN fallback, and the
failures that must surface rather than degrade silently.

No test reaches the network — every call goes through the injected ``send``.
"""

from __future__ import annotations

import pytest

from buildapp.ice_servers import (
    CREDENTIALS_URL,
    STUN_ONLY,
    TTL_SECONDS,
    IceServersUnavailable,
    ice_servers,
)

KEY_ID = "key-abc"
API_TOKEN = "token-xyz"
CLOUDFLARE_SERVERS = [
    {
        "urls": [
            "stun:stun.cloudflare.com:3478",
            "turn:turn.cloudflare.com:3478?transport=udp",
        ],
        "username": "minted-username",
        "credential": "minted-credential",
    }
]


class FakeResponse:
    def __init__(self, status_code: int, body: dict):
        self.status_code = status_code
        self._body = body

    def json(self) -> dict:
        return self._body


class RecordingSend:
    """Stands in for ``requests.post``; records the one call it is given."""

    def __init__(self, response: FakeResponse):
        self._response = response
        self.calls: list[tuple[str, dict]] = []

    def __call__(self, url: str, **kwargs) -> FakeResponse:
        self.calls.append((url, kwargs))
        return self._response


def _created(body: dict) -> RecordingSend:
    return RecordingSend(FakeResponse(201, body))


def test_returns_cloudflares_array_verbatim():
    send = _created({"iceServers": CLOUDFLARE_SERVERS})
    assert ice_servers(KEY_ID, API_TOKEN, send=send) == CLOUDFLARE_SERVERS


def test_asks_the_key_scoped_credentials_url_for_a_ttl_bound_credential():
    send = _created({"iceServers": CLOUDFLARE_SERVERS})
    ice_servers(KEY_ID, API_TOKEN, send=send)
    url, kwargs = send.calls[0]
    assert url == CREDENTIALS_URL.format(key_id=KEY_ID)
    assert kwargs["json"] == {"ttl": TTL_SECONDS}
    assert kwargs["headers"]["Authorization"] == f"Bearer {API_TOKEN}"
    assert kwargs["timeout"] > 0


def test_a_caller_supplied_ttl_is_what_cloudflare_is_asked_for():
    send = _created({"iceServers": CLOUDFLARE_SERVERS})
    ice_servers(KEY_ID, API_TOKEN, ttl_seconds=600, send=send)
    assert send.calls[0][1]["json"] == {"ttl": 600}


@pytest.mark.parametrize(
    ("key_id", "api_token"),
    [("", ""), ("", API_TOKEN), (KEY_ID, "")],
)
def test_without_a_configured_key_the_list_is_stun_only(key_id, api_token):
    send = _created({"iceServers": CLOUDFLARE_SERVERS})
    assert ice_servers(key_id, api_token, send=send) == STUN_ONLY
    assert send.calls == []


def test_the_stun_fallback_cannot_be_mutated_through_a_caller():
    fallback = ice_servers("", "")
    fallback[0]["urls"].append("turn:attacker.example:3478")
    assert ice_servers("", "") == STUN_ONLY


def test_a_non_201_from_cloudflare_fails_fast_with_the_status():
    send = RecordingSend(FakeResponse(403, {"errors": ["bad token"]}))
    with pytest.raises(IceServersUnavailable) as raised:
        ice_servers(KEY_ID, API_TOKEN, send=send)
    assert "403" in str(raised.value)


def test_a_response_without_ice_servers_fails_fast():
    send = _created({"unexpected": True})
    with pytest.raises(IceServersUnavailable):
        ice_servers(KEY_ID, API_TOKEN, send=send)
