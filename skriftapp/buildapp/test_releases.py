"""One module owns every fact about where the bridge comes from: the ONE repository
that holds it, the six release assets, the api's own download paths, and the install
one-liner. Nothing here knows a version number — ``releases/latest`` IS the version —
and nothing outside this module derives a path, an asset name or a URL.

Pins the shared contract install.sh's uname table, the SPA's platform table and the
release workflow's asset list all mirror."""

from __future__ import annotations

from buildapp import releases
from buildapp.releases import (
    CHECKSUMS_ASSET,
    DOWNLOAD_TOKEN_PARAM,
    DOWNLOAD_TOKEN_PATH,
    DOWNLOADABLE,
    DOWNLOADS_PATH,
    GITHUB_OWNER,
    GITHUB_REPO,
    INSTALL_SCRIPT_FILE,
    INSTALL_SCRIPT_PATH,
    PLATFORMS,
    REPOSITORY,
    SIGNATURE_ASSET,
    asset_name,
    download_url,
    downloads_payload,
    install_command,
    install_script_url,
    latest_asset_url,
    latest_release_url,
    render_install_script,
)

PUBLIC_BASE_URL = "https://getbuild.ing"
LATEST = "https://github.com/ZechCodes/build-web/releases/latest"
#: A well-formed token that is obviously not one.
TOKEN = "dl_" + "x" * 32


def test_there_is_one_repository_and_it_is_this_one():
    assert (GITHUB_OWNER, GITHUB_REPO) == ("ZechCodes", "build-web")
    assert REPOSITORY == "ZechCodes/build-web"


def test_the_two_repo_design_is_gone_from_this_module():
    """No RELEASES_REPO env, no default repo, no per-request repo resolution: a repo
    argument is a second repository waiting to happen."""
    for retired in (
        "RELEASES_REPO_ENV",
        "DEFAULT_RELEASES_REPO",
        "releases_repo",
        "provide_releases_repo",
        "INSTALL_SCRIPT_ASSET",
    ):
        assert not hasattr(releases, retired), retired


def test_the_platform_keys_are_the_four_the_whole_pipeline_speaks():
    assert [key for key, _ in PLATFORMS] == [
        "macos-arm64",
        "macos-x86_64",
        "linux-x86_64",
        "linux-aarch64",
    ]
    assert [label for _, label in PLATFORMS] == [
        "macOS · Apple silicon",
        "macOS · Intel",
        "Linux · x86_64",
        "Linux · arm64",
    ]


def test_an_asset_name_is_the_platform_key_with_no_version_in_it():
    assert asset_name("macos-arm64") == "build-bridge-macos-arm64.tar.gz"
    assert all(version not in asset_name("linux-x86_64") for version in ("0.2", "v"))


def test_a_release_carries_exactly_six_assets_and_install_sh_is_not_one_of_them():
    """install.sh is served by the api from the image, so it is neither published as
    an asset nor listed in SHA256SUMS."""
    assert CHECKSUMS_ASSET == "SHA256SUMS"
    assert SIGNATURE_ASSET == "SHA256SUMS.sigstore.json"
    names = {downloadable.name for downloadable in DOWNLOADABLE.values()}
    assert names == {
        "build-bridge-macos-arm64.tar.gz",
        "build-bridge-macos-x86_64.tar.gz",
        "build-bridge-linux-x86_64.tar.gz",
        "build-bridge-linux-aarch64.tar.gz",
        "SHA256SUMS",
        "SHA256SUMS.sigstore.json",
    }
    assert "install.sh" not in names


def test_the_url_segment_is_the_table_key_and_only_a_tarball_spends_a_token():
    """A platform key fetches that platform's tarball; the two support files are
    addressed by their asset name. install.sh needs three fetches per install, so
    only the last of them — the tarball — spends the one-use token."""
    assert list(DOWNLOADABLE) == [
        "macos-arm64",
        "macos-x86_64",
        "linux-x86_64",
        "linux-aarch64",
        "SHA256SUMS",
        "SHA256SUMS.sigstore.json",
    ]
    for key, _ in PLATFORMS:
        assert DOWNLOADABLE[key].name == asset_name(key)
        assert DOWNLOADABLE[key].spends_token is True
    for support in (CHECKSUMS_ASSET, SIGNATURE_ASSET):
        assert DOWNLOADABLE[support].name == support
        assert DOWNLOADABLE[support].spends_token is False


def test_a_download_url_hangs_off_the_apis_own_downloads_path():
    assert DOWNLOADS_PATH == "/app/downloads"
    assert DOWNLOAD_TOKEN_PATH == "/app/downloads/token"
    assert DOWNLOAD_TOKEN_PARAM == "t"
    assert download_url(PUBLIC_BASE_URL, "macos-arm64") == (
        "https://getbuild.ing/app/downloads/macos-arm64"
    )
    assert download_url("http://localhost:8090", CHECKSUMS_ASSET) == (
        "http://localhost:8090/app/downloads/SHA256SUMS"
    )


def test_the_public_github_urls_name_this_repository_and_take_no_repo_argument():
    assert latest_release_url() == LATEST
    assert latest_asset_url(CHECKSUMS_ASSET) == f"{LATEST}/download/SHA256SUMS"
    assert latest_asset_url(asset_name("macos-arm64")) == (
        f"{LATEST}/download/build-bridge-macos-arm64.tar.gz"
    )


def test_the_install_command_quotes_its_url_and_carries_the_token():
    """``?`` is a glob character in sh, so the URL is double-quoted; without the
    token the one-liner installs nothing."""
    assert install_script_url(PUBLIC_BASE_URL) == "https://getbuild.ing/install.sh"
    assert INSTALL_SCRIPT_PATH == "/install.sh"
    assert install_command(PUBLIC_BASE_URL, TOKEN) == (
        f'curl -fsSL "https://getbuild.ing/install.sh?t={TOKEN}" | sh'
    )
    assert install_command("http://localhost:8090", TOKEN).startswith(
        'curl -fsSL "http://localhost:8090/install.sh?t=dl_'
    )


def test_the_payload_carries_exactly_the_keys_the_spa_renders():
    payload = downloads_payload(PUBLIC_BASE_URL, TOKEN, latest_release_url())
    assert list(payload) == [
        "install_command",
        "install_script_url",
        "releases_url",
        "checksums_url",
        "platforms",
    ]
    assert payload["install_command"] == install_command(PUBLIC_BASE_URL, TOKEN)
    assert payload["install_script_url"] == "https://getbuild.ing/install.sh"
    assert payload["releases_url"] == LATEST


def test_every_download_url_in_the_payload_is_the_apis_own_and_carries_no_token():
    """The alpha gate is the api's, so the browser downloads from the api. A member
    session is enough — same-origin cookies ride the click — so no URL here leaks a
    token into a page, a history entry or a referrer."""
    payload = downloads_payload(PUBLIC_BASE_URL, TOKEN, latest_release_url())
    urls = [payload["checksums_url"], *(p["url"] for p in payload["platforms"])]
    for url in urls:
        assert url.startswith("https://getbuild.ing/app/downloads/")
        assert TOKEN not in url
    assert payload["checksums_url"] == download_url(PUBLIC_BASE_URL, CHECKSUMS_ASSET)


def test_the_payload_lists_the_four_platforms_in_order_with_their_own_routes():
    platforms = downloads_payload(PUBLIC_BASE_URL, TOKEN, None)["platforms"]
    assert [p["key"] for p in platforms] == [key for key, _ in PLATFORMS]
    assert [p["label"] for p in platforms] == [label for _, label in PLATFORMS]
    for platform in platforms:
        assert list(platform) == ["key", "label", "url"]
        assert platform["url"] == download_url(PUBLIC_BASE_URL, platform["key"])


def test_the_releases_url_is_whatever_the_asset_source_says_including_nothing():
    """While the repository is private GitHub 404s a non-collaborator, so the page
    offers no "all releases" link at all. The payload does not decide that — it
    carries the value it is handed."""
    assert downloads_payload(PUBLIC_BASE_URL, TOKEN, None)["releases_url"] is None


def test_the_install_script_ships_in_the_image_beside_the_repos_own_copy():
    assert INSTALL_SCRIPT_FILE.name == "install.sh"
    assert INSTALL_SCRIPT_FILE.parent.name == "scripts"
    assert INSTALL_SCRIPT_FILE.is_file()


def test_rendering_the_install_script_fills_both_slots_and_leaves_no_placeholder():
    script = (
        "#!/bin/sh\n"
        "API_BASE_URL='{{api_base_url}}'\n"
        "DOWNLOAD_TOKEN='{{download_token}}'\n"
    )
    rendered = render_install_script(script, PUBLIC_BASE_URL, TOKEN)
    assert f"API_BASE_URL='{PUBLIC_BASE_URL}'" in rendered
    assert f"DOWNLOAD_TOKEN='{TOKEN}'" in rendered
    assert "{{" not in rendered


def test_a_script_rendered_without_a_token_carries_an_empty_token_line():
    """The route serves the script to anyone; only the download routes judge a
    token. An empty one renders an empty line, never the placeholder."""
    script = "DOWNLOAD_TOKEN='{{download_token}}'\nAPI_BASE_URL='{{api_base_url}}'\n"
    rendered = render_install_script(script, PUBLIC_BASE_URL, "")
    assert "DOWNLOAD_TOKEN=''" in rendered
    assert "{{" not in rendered


def test_the_repos_own_install_script_renders_with_nothing_left_to_fill():
    rendered = render_install_script(
        INSTALL_SCRIPT_FILE.read_text(), PUBLIC_BASE_URL, TOKEN
    )
    assert "{{" not in rendered
