"""Where a release asset actually comes from — the one place that knows whether this
repository is private.

Two implementations of one seam, chosen once by whether a token is configured: with a
token the api streams the asset out of the GitHub API (the repository is private, so
nobody else can fetch it); without one it redirects to the public asset and never
touches the network. Every other module asks ``AssetSource`` and learns nothing.

No test reaches GitHub: the fetch is injected, exactly as the ICE-server tests inject
``requests.post``."""

from __future__ import annotations

import asyncio

import pytest
import requests
from litestar.exceptions import HTTPException, NotFoundException
from litestar.response import Redirect, Stream

from buildapp import release_assets
from buildapp.release_assets import (
    GITHUB_RELEASES_TOKEN_ENV,
    NO_RELEASE_DETAIL,
    PrivateAssets,
    PublicAssets,
    asset_source,
)
from buildapp.releases import CHECKSUMS_ASSET, asset_name, latest_release_url

TOKEN = "a-fake-releases-token"
TARBALL = asset_name("macos-arm64")
LATEST_RELEASE_API = (
    "https://api.github.com/repos/ZechCodes/build-web/releases/latest"
)
ASSET_API_URL = "https://api.github.com/repos/ZechCodes/build-web/releases/assets/42"
ASSET_SIZE = 1024
CHUNKS = [b"tar", b"ball"]


class FakeResponse:
    def __init__(self, status_code: int, body: dict | None = None, chunks=()):
        self.status_code = status_code
        self._body = body or {}
        self._chunks = list(chunks)

    def json(self) -> dict:
        return self._body

    def iter_content(self, chunk_size: int):
        assert chunk_size > 0
        return iter(self._chunks)


class RecordingFetch:
    """Stands in for ``requests.get``; answers the queued responses in order and
    records every call."""

    def __init__(self, *responses):
        self._responses = list(responses)
        self.calls: list[tuple[str, dict]] = []

    def __call__(self, url: str, **kwargs) -> FakeResponse:
        self.calls.append((url, kwargs))
        response = self._responses.pop(0)
        if isinstance(response, Exception):
            raise response
        return response


def _release(*assets: dict) -> FakeResponse:
    return FakeResponse(200, {"assets": list(assets)})


def _asset(name: str = TARBALL) -> dict:
    return {"name": name, "url": ASSET_API_URL, "size": ASSET_SIZE}


def _delivered(source, name: str = TARBALL):
    return asyncio.run(source.deliver(name))


def test_no_token_means_the_public_source_and_a_token_means_the_private_one():
    """The one decision. Nothing else in the app reads this variable or branches on
    it."""
    assert GITHUB_RELEASES_TOKEN_ENV == "GITHUB_RELEASES_TOKEN"
    assert isinstance(asset_source({}), PublicAssets)
    assert isinstance(asset_source({GITHUB_RELEASES_TOKEN_ENV: "   "}), PublicAssets)
    private = asset_source({GITHUB_RELEASES_TOKEN_ENV: f"  {TOKEN} "})
    assert isinstance(private, PrivateAssets)
    assert private.token == TOKEN


def test_the_provider_makes_that_decision_from_the_process_environment(monkeypatch):
    monkeypatch.delenv(GITHUB_RELEASES_TOKEN_ENV, raising=False)
    assert isinstance(release_assets.provide_asset_source(), PublicAssets)
    monkeypatch.setenv(GITHUB_RELEASES_TOKEN_ENV, TOKEN)
    assert isinstance(release_assets.provide_asset_source(), PrivateAssets)


def test_a_public_asset_is_a_redirect_to_github_and_costs_no_request():
    source = PublicAssets()
    response = _delivered(source)
    assert isinstance(response, Redirect)
    assert response.url == (
        "https://github.com/ZechCodes/build-web/releases/latest/download/"
        f"{TARBALL}"
    )
    assert _delivered(source, CHECKSUMS_ASSET).url.endswith("/download/SHA256SUMS")


def test_a_public_source_offers_the_releases_page():
    assert PublicAssets().releases_url == latest_release_url()


def test_a_private_source_offers_no_releases_page():
    """A private repository 404s a non-collaborator, so the page must not link to
    one."""
    assert PrivateAssets(TOKEN).releases_url is None


def test_the_private_source_asks_github_for_the_latest_release_as_itself():
    fetch = RecordingFetch(_release(_asset()), FakeResponse(200, chunks=CHUNKS))
    _delivered(PrivateAssets(TOKEN, fetch=fetch))
    url, kwargs = fetch.calls[0]
    assert url == LATEST_RELEASE_API
    assert kwargs["headers"] == {
        "Authorization": f"Bearer {TOKEN}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }
    assert kwargs["timeout"] > 0


def test_the_private_source_then_asks_for_the_named_assets_own_bytes():
    """The asset's API url with ``Accept: application/octet-stream`` answers a 302 to
    a signed objects.githubusercontent.com URL; requests follows it and drops the
    Authorization header across hosts."""
    fetch = RecordingFetch(
        _release(_asset("SHA256SUMS"), _asset()), FakeResponse(200, chunks=CHUNKS)
    )
    _delivered(PrivateAssets(TOKEN, fetch=fetch))
    url, kwargs = fetch.calls[1]
    assert url == ASSET_API_URL
    assert kwargs["headers"] == {
        "Authorization": f"Bearer {TOKEN}",
        "Accept": "application/octet-stream",
    }
    assert kwargs["stream"] is True
    assert kwargs["timeout"] > 0


def test_the_private_source_streams_the_bytes_as_a_named_attachment():
    fetch = RecordingFetch(_release(_asset()), FakeResponse(200, chunks=CHUNKS))
    response = _delivered(PrivateAssets(TOKEN, fetch=fetch))
    assert isinstance(response, Stream)
    assert response.media_type == "application/octet-stream"
    assert response.headers["Content-Disposition"] == (
        f'attachment; filename="{TARBALL}"'
    )
    assert response.headers["Content-Length"] == str(ASSET_SIZE)
    assert list(response.iterator) == CHUNKS


def test_a_repository_with_no_release_yet_is_a_404_that_says_so():
    fetch = RecordingFetch(FakeResponse(404))
    with pytest.raises(NotFoundException) as refusal:
        _delivered(PrivateAssets(TOKEN, fetch=fetch))
    assert refusal.value.detail == NO_RELEASE_DETAIL == (
        "no bridge release has been published yet"
    )


def test_an_unreachable_github_is_a_502_not_a_500():
    fetch = RecordingFetch(requests.exceptions.ConnectTimeout("no route"))
    with pytest.raises(HTTPException) as refusal:
        _delivered(PrivateAssets(TOKEN, fetch=fetch))
    assert refusal.value.status_code == 502


def test_a_github_that_answers_anything_else_is_a_502():
    fetch = RecordingFetch(FakeResponse(500))
    with pytest.raises(HTTPException) as refusal:
        _delivered(PrivateAssets(TOKEN, fetch=fetch))
    assert refusal.value.status_code == 502


def test_a_release_that_does_not_carry_the_asset_is_a_502():
    """The route already refused unknown names, so a missing asset here means the
    release is malformed — upstream's problem, not the caller's."""
    fetch = RecordingFetch(_release(_asset("SHA256SUMS")))
    with pytest.raises(HTTPException) as refusal:
        _delivered(PrivateAssets(TOKEN, fetch=fetch))
    assert refusal.value.status_code == 502
    assert TARBALL in refusal.value.detail


def test_an_asset_download_that_fails_is_a_502():
    fetch = RecordingFetch(_release(_asset()), FakeResponse(403))
    with pytest.raises(HTTPException) as refusal:
        _delivered(PrivateAssets(TOKEN, fetch=fetch))
    assert refusal.value.status_code == 502


def test_no_refusal_ever_carries_the_token():
    for fetch in (
        RecordingFetch(FakeResponse(500)),
        RecordingFetch(_release(_asset("SHA256SUMS"))),
        RecordingFetch(requests.exceptions.ConnectTimeout(f"used {TOKEN}")),
    ):
        with pytest.raises(HTTPException) as refusal:
            _delivered(PrivateAssets(TOKEN, fetch=fetch))
        assert TOKEN not in str(refusal.value.detail)
