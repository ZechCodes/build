"""Where the bridge comes from, and every string that says so.

One repository holds the code, the workflow and the releases: ``ZechCodes/build-web``.
One table says what is downloadable and which segment addresses it. The api serves
every download from its own origin behind the alpha gate, so the URLs here are the
api's, not GitHub's — the only public GitHub URLs left are the ones an asset source
falls back to once the repository is public.

The api knows no version numbers, because "latest" is the version. Templates, the SPA
and install.sh carry no URLs of their own; they render or mirror what this module says.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

from buildapp.landing_page import fill_slots

#: The one repository. Code, release workflow and release assets all live here.
GITHUB_OWNER = "ZechCodes"
GITHUB_REPO = "build-web"
REPOSITORY = f"{GITHUB_OWNER}/{GITHUB_REPO}"

#: (key, human label). Order is the order the SPA lists them in.
PLATFORMS: tuple[tuple[str, str], ...] = (
    ("macos-arm64", "macOS · Apple silicon"),
    ("macos-x86_64", "macOS · Intel"),
    ("linux-x86_64", "Linux · x86_64"),
    ("linux-aarch64", "Linux · arm64"),
)

ASSET_NAME_TEMPLATE = "build-bridge-{key}.tar.gz"
CHECKSUMS_ASSET = "SHA256SUMS"
SIGNATURE_ASSET = "SHA256SUMS.sigstore.json"

DOWNLOADS_PATH = "/app/downloads"
DOWNLOAD_TOKEN_PATH = f"{DOWNLOADS_PATH}/token"
INSTALL_SCRIPT_PATH = "/install.sh"
#: The query parameter a download token rides in, on /install.sh and every asset.
DOWNLOAD_TOKEN_PARAM = "t"

_RELEASES_LATEST = f"https://github.com/{REPOSITORY}/releases/latest"

#: ``?`` is a glob character in sh, so the URL is double-quoted.
INSTALL_COMMAND_TEMPLATE = 'curl -fsSL "{install_script_url}?t={token}" | sh'

#: The script the api serves at /install.sh: the repo's own copy in a source tree,
#: ``/scripts/install.sh`` in the image (Containerfile COPY).
INSTALL_SCRIPT_FILE = Path(__file__).resolve().parents[2] / "scripts" / "install.sh"
API_BASE_URL_SLOT = "api_base_url"
DOWNLOAD_TOKEN_SLOT = "download_token"


def asset_name(key: str) -> str:
    return ASSET_NAME_TEMPLATE.format(key=key)


@dataclass(frozen=True)
class Downloadable:
    """One thing the download route will serve: the release asset it names, and
    whether fetching it spends the caller's one-use token."""

    name: str
    spends_token: bool


#: Every downloadable, keyed by the URL segment that addresses it. A platform key
#: fetches that platform's tarball; the two support files are addressed by their asset
#: name. Anything else is a 404. Only a tarball spends the token — install.sh fetches
#: the checksums and the signature first and the tarball last.
DOWNLOADABLE: dict[str, Downloadable] = {
    **{key: Downloadable(asset_name(key), True) for key, _ in PLATFORMS},
    CHECKSUMS_ASSET: Downloadable(CHECKSUMS_ASSET, False),
    SIGNATURE_ASSET: Downloadable(SIGNATURE_ASSET, False),
}


def download_url(public_base_url: str, segment: str) -> str:
    """Where a browser or install.sh fetches one downloadable from — this deployment,
    never GitHub."""
    return f"{public_base_url.rstrip('/')}{DOWNLOADS_PATH}/{segment}"


def latest_release_url() -> str:
    return _RELEASES_LATEST


def latest_asset_url(name: str) -> str:
    """The anonymous GitHub download URL. Only reachable while the repository is
    public — the public asset source is the one caller."""
    return f"{_RELEASES_LATEST}/download/{name}"


def install_script_url(public_base_url: str) -> str:
    return f"{public_base_url.rstrip('/')}{INSTALL_SCRIPT_PATH}"


def install_command(public_base_url: str, token: str) -> str:
    """The one-liner a human copies. Derived from this deployment's own origin, so a
    dev stack shows its own host rather than production's, and carrying the token
    because the install runs with no browser session."""
    return INSTALL_COMMAND_TEMPLATE.format(
        install_script_url=install_script_url(public_base_url), token=token
    )


def render_install_script(script_text: str, public_base_url: str, token: str) -> str:
    """The served install script: the file on disk with its two slots filled. Both are
    always filled — an unfilled placeholder would reach a shell."""
    return fill_slots(
        script_text,
        {API_BASE_URL_SLOT: public_base_url, DOWNLOAD_TOKEN_SLOT: token},
    )


def downloads_payload(
    public_base_url: str, token: str, releases_url: str | None
) -> dict:
    """What ``GET /app/downloads`` answers. Every download URL is this api's own; the
    releases link is whatever the asset source says it is, including nothing."""
    return {
        "install_command": install_command(public_base_url, token),
        "install_script_url": install_script_url(public_base_url),
        "releases_url": releases_url,
        "checksums_url": download_url(public_base_url, CHECKSUMS_ASSET),
        "platforms": [
            {"key": key, "label": label, "url": download_url(public_base_url, key)}
            for key, label in PLATFORMS
        ],
    }
