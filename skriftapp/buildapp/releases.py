"""Public release locations shared by download metadata and website installers."""

from __future__ import annotations

import os
import re
from collections.abc import Mapping
from pathlib import Path

DEFAULT_RELEASES_REPO = "ZechCodes/build-releases"
RELEASES_REPO_ENV = "RELEASES_REPO"
PLATFORMS: tuple[tuple[str, str], ...] = (
    ("macos-arm64", "macOS · Apple silicon"),
    ("macos-x86_64", "macOS · Intel"),
    ("linux-x86_64", "Linux · x86_64"),
    ("linux-aarch64", "Linux · arm64"),
)
CHECKSUMS_ASSET = "SHA256SUMS"
SIGNATURE_ASSET = "SHA256SUMS.sigstore.json"
DOWNLOADS_PATH = "/app/downloads"
DOWNLOAD_TOKEN_PATH = f"{DOWNLOADS_PATH}/token"
# Retained for previously issued download tokens and their authentication helper.
DOWNLOAD_TOKEN_PARAM = "t"
INSTALL_SCRIPT_PATH = "/install.sh"
DESKTOP_INSTALL_SCRIPT_PATH = "/install-desktop.sh"
SCRIPTS_DIR = Path(__file__).resolve().parents[2] / "scripts"
INSTALL_SCRIPT_FILE = SCRIPTS_DIR / "install.sh"
DESKTOP_INSTALL_SCRIPT_FILE = SCRIPTS_DIR / "install-desktop.sh"


def releases_repo(environment: Mapping[str, str] | None = None) -> str:
    environment = os.environ if environment is None else environment
    repo = environment.get(RELEASES_REPO_ENV, DEFAULT_RELEASES_REPO).strip()
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*", repo):
        raise ValueError("RELEASES_REPO must be a GitHub owner/repository")
    return repo


def asset_name(key: str) -> str:
    return f"build-bridge-{key}.tar.gz"


DOWNLOADABLE: dict[str, str] = {
    **{key: asset_name(key) for key, _ in PLATFORMS},
    CHECKSUMS_ASSET: CHECKSUMS_ASSET,
    SIGNATURE_ASSET: SIGNATURE_ASSET,
}


def download_url(public_base_url: str, segment: str) -> str:
    return f"{public_base_url.rstrip('/')}{DOWNLOADS_PATH}/{segment}"


def latest_release_url(repo: str | None = None) -> str:
    return f"https://github.com/{repo or releases_repo()}/releases/latest"


def desktop_release_url() -> str:
    return f"https://github.com/{releases_repo()}/releases"


def latest_asset_url(name: str, repo: str | None = None) -> str:
    return f"{latest_release_url(repo)}/download/{name}"


def install_script_url(public_base_url: str, *, desktop: bool = False) -> str:
    path = DESKTOP_INSTALL_SCRIPT_PATH if desktop else INSTALL_SCRIPT_PATH
    return f"{public_base_url.rstrip('/')}{path}"


def install_command(public_base_url: str, *, desktop: bool = False) -> str:
    return f'curl -fsSL "{install_script_url(public_base_url, desktop=desktop)}" | sh'


def render_install_script(script_text: str) -> str:
    """Override only the checked-in default; retain the installer's runtime override.

    Repository validation prevents configuration from injecting shell syntax.
    """
    return script_text.replace(
        f'DEFAULT_REPO="{DEFAULT_RELEASES_REPO}"',
        f'DEFAULT_REPO="{releases_repo()}"',
    )


def downloads_payload(public_base_url: str, releases_url: str | None) -> dict:
    return {
        "install_command": install_command(public_base_url),
        "install_script_url": install_script_url(public_base_url),
        "releases_url": releases_url,
        "desktop_install_command": install_command(public_base_url, desktop=True),
        "desktop_install_script_url": install_script_url(public_base_url, desktop=True),
        "desktop_releases_url": desktop_release_url(),
        "checksums_url": download_url(public_base_url, CHECKSUMS_ASSET),
        "platforms": [
            {"key": key, "label": label, "url": download_url(public_base_url, key)}
            for key, label in PLATFORMS
        ],
    }
