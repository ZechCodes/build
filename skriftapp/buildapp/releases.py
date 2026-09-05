"""Where the bridge binaries live, and the one payload that says so.

Every fact in the downloads response is here: the four platform keys the whole pipeline
speaks (release matrix, asset names, install.sh's uname table, the SPA's platform
table), the version-free asset names, and the ``releases/latest`` URLs — the api knows
no version numbers, because "latest" is the version. Templates and the SPA carry no
URLs of their own; they render what this module returns.
"""

from __future__ import annotations

import os
from collections.abc import Mapping

RELEASES_REPO_ENV = "RELEASES_REPO"
DEFAULT_RELEASES_REPO = "ZechCodes/build-releases"

#: (key, human label). Order is the order the SPA lists them in.
PLATFORMS: tuple[tuple[str, str], ...] = (
    ("macos-arm64", "macOS · Apple silicon"),
    ("macos-x86_64", "macOS · Intel"),
    ("linux-x86_64", "Linux · x86_64"),
    ("linux-aarch64", "Linux · arm64"),
)

ASSET_NAME_TEMPLATE = "build-bridge-{key}.tar.gz"
CHECKSUMS_ASSET = "SHA256SUMS"
INSTALL_SCRIPT_ASSET = "install.sh"
INSTALL_SCRIPT_PATH = "/install.sh"
INSTALL_COMMAND_TEMPLATE = "curl -fsSL {install_script_url} | sh"

_RELEASES_LATEST = "https://github.com/{repo}/releases/latest"
_LATEST_DOWNLOAD = f"{_RELEASES_LATEST}/download/{{name}}"


def releases_repo(environment: Mapping[str, str]) -> str:
    """The GitHub ``owner/name`` holding the release assets."""
    return environment.get(RELEASES_REPO_ENV, "").strip() or DEFAULT_RELEASES_REPO


def provide_releases_repo() -> str:
    """The dependency every route serving a download URL takes, so no handler reads
    the environment itself."""
    return releases_repo(os.environ)


def asset_name(key: str) -> str:
    return ASSET_NAME_TEMPLATE.format(key=key)


def latest_release_url(repo: str) -> str:
    return _RELEASES_LATEST.format(repo=repo)


def latest_asset_url(repo: str, name: str) -> str:
    return _LATEST_DOWNLOAD.format(repo=repo, name=name)


def install_script_url(public_base_url: str) -> str:
    return f"{public_base_url.rstrip('/')}{INSTALL_SCRIPT_PATH}"


def install_command(public_base_url: str) -> str:
    """The one-liner a human copies. Derived from this deployment's own origin, so a
    dev stack shows its own host rather than production's."""
    return INSTALL_COMMAND_TEMPLATE.format(
        install_script_url=install_script_url(public_base_url)
    )


def downloads_payload(repo: str, public_base_url: str) -> dict:
    return {
        "install_command": install_command(public_base_url),
        "install_script_url": install_script_url(public_base_url),
        "releases_url": latest_release_url(repo),
        "checksums_url": latest_asset_url(repo, CHECKSUMS_ASSET),
        "platforms": [
            {"key": key, "label": label, "url": latest_asset_url(repo, asset_name(key))}
            for key, label in PLATFORMS
        ],
    }
