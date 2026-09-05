"""Where a release asset comes from — the one place that knows whether the repository
is private.

The alpha gate is the api's, so a download is always a request to this app. What
happens next is the only thing that differs: while ``ZechCodes/build-web`` is private
its release assets are not anonymously fetchable, so the api streams them out of the
GitHub REST API with a read-only token; once the repository is public there is nothing
to hide and the same route redirects to the public asset URL.

Two implementations of one seam, chosen once by ``asset_source`` from whether the token
is configured. Nothing else in the app reads that variable or branches on it — a
handler asks its ``AssetSource`` and learns nothing about which one it holds.
"""

from __future__ import annotations

import asyncio
import os
from collections.abc import Iterator, Mapping
from typing import Protocol, runtime_checkable

import requests
from litestar.exceptions import HTTPException, NotFoundException
from litestar.response import Redirect, Response, Stream
from litestar.status_codes import HTTP_404_NOT_FOUND, HTTP_502_BAD_GATEWAY

from buildapp.releases import REPOSITORY, latest_asset_url, latest_release_url

GITHUB_RELEASES_TOKEN_ENV = "GITHUB_RELEASES_TOKEN"

LATEST_RELEASE_URL = f"https://api.github.com/repos/{REPOSITORY}/releases/latest"
GITHUB_API_VERSION = "2022-11-28"
GITHUB_JSON = "application/vnd.github+json"
OCTET_STREAM = "application/octet-stream"

#: One hung GitHub request must not hold a worker thread open indefinitely.
REQUEST_TIMEOUT_SECONDS = 10
CHUNK_BYTES = 65536

NO_RELEASE_DETAIL = "no bridge release has been published yet"
LOOKUP_FAILED_DETAIL = "release lookup failed"


@runtime_checkable
class AssetSource(Protocol):
    """How the api answers "give me this release asset". Runtime-checkable because a
    handler declares it as a dependency type and Litestar validates what it is handed."""

    #: The human-facing releases page, when there is one a visitor could open.
    releases_url: str | None

    async def deliver(self, name: str) -> Response:
        """The response that hands the caller the named asset."""
        ...


class PublicAssets:
    """The repository is public: GitHub serves the asset itself, so the api points at
    it and stays out of the way. No token, no request, no bytes through this pod."""

    releases_url: str | None = latest_release_url()

    async def deliver(self, name: str) -> Response:
        return Redirect(latest_asset_url(name))


class PrivateAssets:
    """The repository is private: only this app's token can fetch an asset, so the api
    fetches it and streams the bytes on. There is no releases page a member could
    open — GitHub 404s a non-collaborator — so it offers none."""

    releases_url: str | None = None

    def __init__(self, token: str, fetch=requests.get):
        self.token = token
        self._fetch = fetch
        self._release_headers = self._bearer() | {
            "Accept": GITHUB_JSON,
            "X-GitHub-Api-Version": GITHUB_API_VERSION,
        }
        self._asset_headers = self._bearer() | {"Accept": OCTET_STREAM}

    async def deliver(self, name: str) -> Response:
        asset = self._named(await self._latest_release(), name)
        response = await asyncio.to_thread(
            self._get,
            asset["url"],
            headers=self._asset_headers,
            stream=True,
        )
        if response.status_code != 200:
            raise _bad_gateway(LOOKUP_FAILED_DETAIL)
        return Stream(
            _chunks(response),
            media_type=OCTET_STREAM,
            headers={
                "Content-Disposition": f'attachment; filename="{name}"',
                "Content-Length": str(asset["size"]),
            },
        )

    async def _latest_release(self) -> dict:
        response = await asyncio.to_thread(
            self._get, LATEST_RELEASE_URL, headers=self._release_headers
        )
        if response.status_code == HTTP_404_NOT_FOUND:
            raise NotFoundException(NO_RELEASE_DETAIL)
        if response.status_code != 200:
            raise _bad_gateway(LOOKUP_FAILED_DETAIL)
        return response.json()

    @staticmethod
    def _named(release: dict, name: str) -> dict:
        for asset in release.get("assets", []):
            if asset.get("name") == name:
                return asset
        raise _bad_gateway(f"the latest release has no {name}")

    def _get(self, url: str, **kwargs):
        """One synchronous request, run in a worker thread by every caller. A
        transport failure is upstream's, never a 500 here — and its message never
        carries the token."""
        try:
            return self._fetch(url, timeout=REQUEST_TIMEOUT_SECONDS, **kwargs)
        except requests.exceptions.RequestException as exc:
            raise _bad_gateway(LOOKUP_FAILED_DETAIL) from exc

    def _bearer(self) -> dict[str, str]:
        """Who the api is to GitHub. Safe on the asset url too: GitHub answers that
        with a 302 to a signed objects.githubusercontent.com URL and requests strips
        Authorization across hosts, so the token never leaves api.github.com."""
        return {"Authorization": f"Bearer {self.token}"}


def _chunks(response) -> Iterator[bytes]:
    """The asset's bytes, with the upstream response's life tied to them. A caller
    that walks away mid-tarball closes this generator, which returns the connection
    to the pool there and then rather than whenever the collector gets to it."""
    with response:
        yield from response.iter_content(CHUNK_BYTES)


def _bad_gateway(detail: str) -> HTTPException:
    return HTTPException(status_code=HTTP_502_BAD_GATEWAY, detail=detail)


def asset_source(environment: Mapping[str, str]) -> AssetSource:
    """The one decision: a configured token means the repository is private and the
    api streams; no token means it is public and the api redirects."""
    token = environment.get(GITHUB_RELEASES_TOKEN_ENV, "").strip()
    return PrivateAssets(token) if token else PublicAssets()


def provide_asset_source() -> AssetSource:
    """The dependency every download route takes, so no handler reads the
    environment itself."""
    return asset_source(os.environ)
