"""One module owns every fact in the downloads payload: the four platform keys, the
asset name each maps to, and the two URLs that never carry a version number because
``releases/latest`` IS the version. Pins the shared contract the SPA renders and
install.sh's uname table names."""

from __future__ import annotations

from buildapp import releases
from buildapp.releases import (
    CHECKSUMS_ASSET,
    DEFAULT_RELEASES_REPO,
    INSTALL_SCRIPT_ASSET,
    INSTALL_SCRIPT_PATH,
    PLATFORMS,
    RELEASES_REPO_ENV,
    asset_name,
    downloads_payload,
    install_command,
    latest_asset_url,
    releases_repo,
)

PUBLIC_BASE_URL = "https://getbuild.ing"
REPO = "ZechCodes/build-releases"
LATEST_DOWNLOAD = f"https://github.com/{REPO}/releases/latest/download"


def test_the_platform_keys_are_the_four_the_whole_pipeline_speaks():
    assert [key for key, _ in PLATFORMS] == [
        "macos-arm64",
        "macos-x86_64",
        "linux-x86_64",
        "linux-aarch64",
    ]


def test_an_asset_name_is_the_platform_key_with_no_version_in_it():
    assert asset_name("macos-arm64") == "build-bridge-macos-arm64.tar.gz"
    assert all(version not in asset_name("linux-x86_64") for version in ("0.2", "v"))


def test_a_latest_asset_url_hangs_off_releases_latest_download():
    assert latest_asset_url(REPO, CHECKSUMS_ASSET) == f"{LATEST_DOWNLOAD}/SHA256SUMS"
    assert latest_asset_url(REPO, INSTALL_SCRIPT_ASSET) == f"{LATEST_DOWNLOAD}/install.sh"


def test_the_install_command_is_the_one_liner_over_this_deployments_own_host():
    assert install_command(PUBLIC_BASE_URL) == (
        "curl -fsSL https://getbuild.ing/install.sh | sh"
    )
    assert install_command("http://localhost:8090") == (
        "curl -fsSL http://localhost:8090/install.sh | sh"
    )
    assert INSTALL_SCRIPT_PATH == "/install.sh"


def test_the_payload_carries_exactly_the_keys_the_spa_renders():
    payload = downloads_payload(REPO, PUBLIC_BASE_URL)
    assert list(payload) == [
        "install_command",
        "install_script_url",
        "releases_url",
        "checksums_url",
        "platforms",
    ]
    assert payload["install_script_url"] == f"{PUBLIC_BASE_URL}/install.sh"
    assert payload["releases_url"] == f"https://github.com/{REPO}/releases/latest"
    assert payload["checksums_url"] == f"{LATEST_DOWNLOAD}/SHA256SUMS"


def test_the_payload_lists_the_four_platforms_in_order_with_their_tarballs():
    platforms = downloads_payload(REPO, PUBLIC_BASE_URL)["platforms"]
    assert [p["key"] for p in platforms] == [key for key, _ in PLATFORMS]
    assert [p["label"] for p in platforms] == [
        "macOS · Apple silicon",
        "macOS · Intel",
        "Linux · x86_64",
        "Linux · arm64",
    ]
    for platform in platforms:
        assert platform["url"] == f"{LATEST_DOWNLOAD}/{asset_name(platform['key'])}"
        assert list(platform) == ["key", "label", "url"]


def test_the_repo_comes_from_the_environment_and_falls_back_to_the_default():
    assert RELEASES_REPO_ENV == "RELEASES_REPO"
    assert DEFAULT_RELEASES_REPO == REPO
    assert releases_repo({}) == DEFAULT_RELEASES_REPO
    assert releases_repo({RELEASES_REPO_ENV: "  "}) == DEFAULT_RELEASES_REPO
    assert releases_repo({RELEASES_REPO_ENV: " someone/forks "}) == "someone/forks"


def test_the_provider_reads_the_repo_from_the_process_environment(monkeypatch):
    """One resolution of RELEASES_REPO, so no handler reaches into os.environ."""
    monkeypatch.setenv(releases.RELEASES_REPO_ENV, "someone/forks")
    assert releases.provide_releases_repo() == "someone/forks"
    monkeypatch.delenv(releases.RELEASES_REPO_ENV)
    assert releases.provide_releases_repo() == releases.DEFAULT_RELEASES_REPO
