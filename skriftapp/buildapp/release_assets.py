"""Public release redirects; legacy GitHub credentials never affect downloads."""

from __future__ import annotations

import os
from collections.abc import Mapping
from typing import Protocol, runtime_checkable

from litestar.response import Redirect, Response

from buildapp.releases import latest_asset_url, latest_release_url, releases_repo


@runtime_checkable
class AssetSource(Protocol):
    releases_url: str | None

    async def deliver(self, name: str) -> Response: ...


class PublicAssets:
    def __init__(self, repo: str | None = None):
        self.repo = repo or releases_repo()
        self.releases_url = latest_release_url(self.repo)

    async def deliver(self, name: str) -> Response:
        return Redirect(latest_asset_url(name, self.repo))


def asset_source(environment: Mapping[str, str]) -> AssetSource:
    return PublicAssets(releases_repo(environment))


def provide_asset_source() -> AssetSource:
    return asset_source(os.environ)
