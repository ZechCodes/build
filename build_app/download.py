"""Download page for the Build desktop app.

Queries the latest GitHub release for ZechCodes/build-app and renders a
platform selector. Asset URLs are cached in-process for five minutes so we
don't hammer the GitHub API.
"""

from __future__ import annotations

import logging
import time
from dataclasses import asdict, dataclass, field
from typing import Any

import httpx
from litestar import Request, get
from litestar.response import Template

logger = logging.getLogger(__name__)

GITHUB_API_URL = "https://api.github.com/repos/ZechCodes/build-app/releases/latest"
CACHE_TTL_SECONDS = 300


@dataclass(frozen=True)
class PlatformAssets:
    macos_dmg: str | None = None
    windows_exe: str | None = None
    linux_appimage: str | None = None
    linux_deb: str | None = None
    version: str | None = None


@dataclass
class _Cache:
    fetched_at: float = 0.0
    assets: PlatformAssets = field(default_factory=PlatformAssets)


_cache = _Cache()


def _platform_from_user_agent(ua: str) -> str:
    ua_l = ua.lower()
    if "android" in ua_l:
        return "linux"
    if "mac os x" in ua_l or "macintosh" in ua_l:
        return "macos"
    if "windows" in ua_l:
        return "windows"
    if "linux" in ua_l:
        return "linux"
    return "macos"


def _assets_from_release(data: dict[str, Any]) -> PlatformAssets:
    assets = {a["name"]: a["browser_download_url"] for a in data.get("assets", [])}

    def match(predicate) -> str | None:
        for name, url in assets.items():
            if predicate(name):
                return url
        return None

    return PlatformAssets(
        macos_dmg=match(lambda n: n.endswith(".dmg")),
        windows_exe=match(lambda n: n.endswith(".exe") and "Setup" in n),
        linux_appimage=match(lambda n: n.endswith(".AppImage")),
        linux_deb=match(lambda n: n.endswith(".deb")),
        version=data.get("tag_name"),
    )


async def _fetch_assets() -> PlatformAssets:
    now = time.monotonic()
    if now - _cache.fetched_at < CACHE_TTL_SECONDS and _cache.assets.version:
        return _cache.assets
    try:
        async with httpx.AsyncClient(timeout=5) as client:
            resp = await client.get(
                GITHUB_API_URL,
                headers={"Accept": "application/vnd.github+json"},
            )
            resp.raise_for_status()
            assets = _assets_from_release(resp.json())
    except Exception:
        logger.exception("Failed to fetch latest build-app release")
        return _cache.assets
    _cache.fetched_at = now
    _cache.assets = assets
    return assets


@get("/download", exclude_from_auth=True)
async def download_page(request: Request) -> Template:
    assets = await _fetch_assets()
    default_platform = _platform_from_user_agent(
        request.headers.get("user-agent", "")
    )
    return Template(
        "page-download.html",
        context={
            "assets": asdict(assets),
            "default_platform": default_platform,
        },
    )
