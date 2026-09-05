"""The download routes over a real ASGI stack: what an alpha member is handed, what
the install one-liner may fetch with it, and what each refusal looks like.

Nothing here reaches GitHub — the asset source is injected, and the public one never
makes a request at all."""

from __future__ import annotations

import contextlib
import re
from collections.abc import Iterator
from datetime import timedelta

import pytest
from litestar.di import Provide
from litestar.exceptions import HTTPException, NotFoundException
from litestar.response import Stream
from litestar.status_codes import (
    HTTP_200_OK,
    HTTP_201_CREATED,
    HTTP_302_FOUND,
    HTTP_401_UNAUTHORIZED,
    HTTP_403_FORBIDDEN,
    HTTP_404_NOT_FOUND,
    HTTP_502_BAD_GATEWAY,
)
from litestar.testing import TestClient
from sqlalchemy import select

from buildapp import download_tokens, email_message, releases
from buildapp.alpha_membership import INVITE_ONLY_DETAIL
from buildapp.controllers import BuildController
from buildapp.db_test_support import (
    asgi_app,
    in_memory_session_maker,
    revoke_every_invite,
    session_backend_config,
    sign_in,
    sign_in_member,
    sign_out,
)
from buildapp.email_test_support import PUBLIC_BASE_URL, email_settings
from buildapp.models import EphemeralToken
from buildapp.release_assets import NO_RELEASE_DETAIL, PublicAssets
from buildapp.releases import CHECKSUMS_ASSET, SIGNATURE_ASSET, asset_name
from buildapp.token_hash import token_hash

DOWNLOADS = "/app/downloads"
TOKEN_ROUTE = "/app/downloads/token"
PLATFORM = "macos-arm64"
TARBALL_ROUTE = f"{DOWNLOADS}/{PLATFORM}"
MEMBER = "member@example.com"
STREAMED = b"bridge-bytes"
TOKEN_IN_TEXT = re.compile(r"dl_[A-Za-z0-9_-]{32}")


class StreamingAssets:
    """Stands in for the private source: records what it was asked for and streams a
    little something back."""

    releases_url = None

    def __init__(self):
        self.delivered: list[str] = []

    async def deliver(self, name: str) -> Stream:
        self.delivered.append(name)
        return Stream(
            iter([STREAMED]),
            media_type="application/octet-stream",
            headers={
                "Content-Disposition": f'attachment; filename="{name}"',
                "Content-Length": str(len(STREAMED)),
            },
        )


@contextlib.contextmanager
def _client(monkeypatch, source) -> Iterator[TestClient]:
    monkeypatch.setattr(email_message, "get_settings", email_settings)
    monkeypatch.setitem(
        BuildController.dependencies,
        "asset_source",
        Provide(lambda: source, sync_to_thread=False),
    )
    session_config = session_backend_config()
    app = asgi_app(
        [BuildController],
        session_maker=in_memory_session_maker(),
        session_config=session_config,
    )
    with TestClient(app=app, session_config=session_config) as test_client:
        yield test_client


@pytest.fixture()
def assets() -> StreamingAssets:
    return StreamingAssets()


@pytest.fixture()
def client(monkeypatch, assets) -> Iterator[TestClient]:
    """A deployment whose repository is still private: the api streams the bytes."""
    with _client(monkeypatch, assets) as test_client:
        yield test_client


@pytest.fixture()
def public_client(monkeypatch) -> Iterator[TestClient]:
    """A deployment whose repository has gone public: the api points at GitHub."""
    with _client(monkeypatch, PublicAssets()) as test_client:
        yield test_client


def _one_liner_token(client: TestClient) -> str:
    payload = client.get(DOWNLOADS).json()
    return TOKEN_IN_TEXT.search(payload["install_command"]).group(0)


def _stored_token_hashes(client: TestClient) -> list[str]:
    async def read() -> list[str]:
        async with client.app.state.make_session() as session:
            rows = (await session.execute(select(EphemeralToken))).scalars().all()
            return [row.token_hash for row in rows]

    with client.portal() as portal:
        return portal.call(read)


def _age_every_token(client: TestClient, by: timedelta) -> None:
    async def age() -> None:
        async with client.app.state.make_session() as session:
            for row in (await session.execute(select(EphemeralToken))).scalars().all():
                row.expires_at = row.expires_at - by
            await session.commit()

    with client.portal() as portal:
        portal.call(age)


# ----- the page a member is handed ------------------------------------------


def test_the_downloads_payload_keeps_its_shape_and_points_at_this_api(client):
    sign_in_member(client)
    payload = client.get(DOWNLOADS).json()
    assert list(payload) == [
        "install_command",
        "install_script_url",
        "releases_url",
        "checksums_url",
        "platforms",
    ]
    assert payload["install_script_url"] == f"{PUBLIC_BASE_URL}/install.sh"
    assert payload["checksums_url"] == f"{PUBLIC_BASE_URL}{DOWNLOADS}/SHA256SUMS"
    assert [p["url"] for p in payload["platforms"]] == [
        f"{PUBLIC_BASE_URL}{DOWNLOADS}/{key}" for key, _ in releases.PLATFORMS
    ]


def test_a_private_deployment_offers_no_releases_page_and_a_public_one_does(
    client, public_client
):
    sign_in_member(client)
    sign_in_member(public_client)
    assert client.get(DOWNLOADS).json()["releases_url"] is None
    assert public_client.get(DOWNLOADS).json()["releases_url"] == (
        "https://github.com/ZechCodes/build-web/releases/latest"
    )


def test_rendering_the_page_mints_a_token_that_is_stored_only_as_a_hash(client):
    sign_in_member(client)
    payload = client.get(DOWNLOADS).json()
    token = TOKEN_IN_TEXT.search(payload["install_command"]).group(0)
    assert payload["install_command"] == (
        f'curl -fsSL "{PUBLIC_BASE_URL}/install.sh?t={token}" | sh'
    )
    assert _stored_token_hashes(client) == [token_hash(token)]


def test_every_render_hands_out_a_fresh_token(client):
    sign_in_member(client)
    assert _one_liner_token(client) != _one_liner_token(client)


def test_an_anonymous_visitor_is_refused_the_downloads_page(client):
    assert client.get(DOWNLOADS).status_code == HTTP_401_UNAUTHORIZED


def test_a_signed_in_non_member_is_refused_the_downloads_page(client):
    sign_in(client, "stranger@example.com")
    response = client.get(DOWNLOADS)
    assert response.status_code == HTTP_403_FORBIDDEN
    assert response.json()["detail"] == INVITE_ONLY_DETAIL


# ----- minting a fresh line --------------------------------------------------


def test_a_member_can_mint_a_fresh_install_line(client):
    sign_in_member(client)
    response = client.post(TOKEN_ROUTE)
    assert response.status_code == HTTP_201_CREATED
    body = response.json()
    assert list(body) == ["token", "install_command", "expires_in_s"]
    assert download_tokens.is_well_formed(body["token"])
    assert body["expires_in_s"] == 600
    assert body["install_command"] == (
        f'curl -fsSL "{PUBLIC_BASE_URL}/install.sh?t={body["token"]}" | sh'
    )
    assert _stored_token_hashes(client) == [token_hash(body["token"])]


def test_only_a_member_may_mint_an_install_line(client):
    assert client.post(TOKEN_ROUTE).status_code == HTTP_401_UNAUTHORIZED
    sign_in(client, "stranger@example.com")
    refusal = client.post(TOKEN_ROUTE)
    assert refusal.status_code == HTTP_403_FORBIDDEN
    assert refusal.json()["detail"] == INVITE_ONLY_DETAIL


# ----- fetching an asset -----------------------------------------------------


def test_a_member_session_downloads_a_platform_without_any_token(client, assets):
    sign_in_member(client)
    response = client.get(TARBALL_ROUTE)
    assert response.status_code == HTTP_200_OK
    assert response.content == STREAMED
    assert response.headers["content-disposition"] == (
        f'attachment; filename="{asset_name(PLATFORM)}"'
    )
    assert assets.delivered == [asset_name(PLATFORM)]


def test_a_member_session_downloads_the_checksums_and_the_signature(client, assets):
    sign_in_member(client)
    for name in (CHECKSUMS_ASSET, SIGNATURE_ASSET):
        assert client.get(f"{DOWNLOADS}/{name}").status_code == HTTP_200_OK
    assert assets.delivered == [CHECKSUMS_ASSET, SIGNATURE_ASSET]


def test_a_public_deployment_redirects_the_same_route_to_github(public_client):
    sign_in_member(public_client)
    response = public_client.get(TARBALL_ROUTE, follow_redirects=False)
    assert response.status_code == HTTP_302_FOUND
    assert response.headers["location"] == (
        "https://github.com/ZechCodes/build-web/releases/latest/download/"
        f"{asset_name(PLATFORM)}"
    )


def test_the_install_lines_token_downloads_with_no_session_at_all(client, assets):
    sign_in_member(client)
    token = _one_liner_token(client)
    sign_out(client)

    response = client.get(TARBALL_ROUTE, params={"t": token})

    assert response.status_code == HTTP_200_OK
    assert response.content == STREAMED
    assert assets.delivered == [asset_name(PLATFORM)]


def test_the_tarball_spends_the_token_and_the_support_files_do_not(client):
    """install.sh fetches SHA256SUMS, then the signature, then the tarball — so the
    line survives the first two and dies on the third."""
    sign_in_member(client)
    token = _one_liner_token(client)
    sign_out(client)
    fetch = {"params": {"t": token}}

    assert client.get(f"{DOWNLOADS}/{CHECKSUMS_ASSET}", **fetch).status_code == 200
    assert client.get(f"{DOWNLOADS}/{SIGNATURE_ASSET}", **fetch).status_code == 200
    assert client.get(TARBALL_ROUTE, **fetch).status_code == HTTP_200_OK

    assert _stored_token_hashes(client) == []
    assert client.get(TARBALL_ROUTE, **fetch).status_code == HTTP_401_UNAUTHORIZED


def test_an_expired_token_downloads_nothing(client, assets):
    sign_in_member(client)
    token = _one_liner_token(client)
    _age_every_token(client, download_tokens.TTL + timedelta(seconds=1))
    sign_out(client)

    response = client.get(TARBALL_ROUTE, params={"t": token})

    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert assets.delivered == []


def test_a_malformed_token_downloads_nothing(client, assets):
    sign_in_member(client)
    sign_out(client)
    response = client.get(TARBALL_ROUTE, params={"t": "../../etc/passwd"})
    assert response.status_code == HTTP_401_UNAUTHORIZED
    assert assets.delivered == []


def test_a_revoked_members_live_token_downloads_nothing(client, assets):
    sign_in_member(client)
    token = _one_liner_token(client)
    revoke_every_invite(client)
    sign_out(client)

    response = client.get(TARBALL_ROUTE, params={"t": token})

    assert response.status_code == HTTP_403_FORBIDDEN
    assert response.json()["detail"] == INVITE_ONLY_DETAIL
    assert assets.delivered == []


def test_an_unknown_segment_is_a_404_that_fetches_nothing_and_spends_nothing(
    client, assets
):
    sign_in_member(client)
    token = _one_liner_token(client)
    sign_out(client)

    response = client.get(f"{DOWNLOADS}/install.sh", params={"t": token})

    assert response.status_code == HTTP_404_NOT_FOUND
    assert response.json()["detail"] == "no such download"
    assert assets.delivered == []
    assert _stored_token_hashes(client) == [token_hash(token)]


def test_a_session_download_never_spends_a_token_it_does_not_hold(client):
    """The browsing member is identified by their session, so someone else's install
    line survives the click."""
    sign_in_member(client, "holder@example.com")
    token = _one_liner_token(client)
    sign_in_member(client, MEMBER)

    assert client.get(TARBALL_ROUTE, params={"t": token}).status_code == HTTP_200_OK

    assert token_hash(token) in _stored_token_hashes(client)


# ----- what upstream's failures look like from here --------------------------


class RefusingAssets:
    """A source that cannot deliver: whatever it raises is what the caller sees."""

    releases_url = None

    def __init__(self, refusal: Exception):
        self._refusal = refusal

    async def deliver(self, name: str) -> Stream:
        raise self._refusal


def _refused_download(monkeypatch, refusal: Exception):
    """Drive one tarball request through a source that refuses, and answer with the
    response and the token hashes that survived it."""
    with _client(monkeypatch, RefusingAssets(refusal)) as client:
        sign_in_member(client)
        token = _one_liner_token(client)
        sign_out(client)
        response = client.get(TARBALL_ROUTE, params={"t": token})
        return response, _stored_token_hashes(client), token_hash(token)


def test_a_repository_with_no_release_yet_answers_404_and_spends_nothing(monkeypatch):
    response, stored, spent_hash = _refused_download(
        monkeypatch, NotFoundException(NO_RELEASE_DETAIL)
    )
    assert response.status_code == HTTP_404_NOT_FOUND
    assert response.json()["detail"] == NO_RELEASE_DETAIL
    assert stored == [spent_hash]


def test_an_unreachable_github_answers_502_and_spends_nothing(monkeypatch):
    """A download that failed must not cost the member their install line."""
    response, stored, spent_hash = _refused_download(
        monkeypatch,
        HTTPException(status_code=HTTP_502_BAD_GATEWAY, detail="release lookup failed"),
    )
    assert response.status_code == HTTP_502_BAD_GATEWAY
    assert stored == [spent_hash]
