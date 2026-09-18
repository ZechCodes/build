"""Public release contract, including safe deployment overrides."""

import pytest

from buildapp import releases


def test_default_repository_and_bridge_assets(monkeypatch):
    monkeypatch.delenv("RELEASES_REPO", raising=False)
    assert releases.releases_repo() == "ZechCodes/build-releases"
    assert releases.latest_asset_url("SHA256SUMS") == (
        "https://github.com/ZechCodes/build-releases/releases/latest/download/SHA256SUMS"
    )
    assert set(releases.DOWNLOADABLE) == {
        "macos-arm64", "macos-x86_64", "linux-x86_64", "linux-aarch64",
        "SHA256SUMS", "SHA256SUMS.sigstore.json",
    }


def test_metadata_and_both_scripts_share_the_configured_repo(monkeypatch):
    monkeypatch.setenv("RELEASES_REPO", "example/releases")
    payload = releases.downloads_payload("https://example.test", releases.latest_release_url())
    assert payload["releases_url"] == "https://github.com/example/releases/releases/latest"
    assert payload["desktop_releases_url"] == "https://github.com/example/releases/releases"
    assert payload["install_command"] == 'curl -fsSL "https://example.test/install.sh" | sh'
    assert payload["desktop_install_command"] == 'curl -fsSL "https://example.test/install-desktop.sh" | sh'
    for path in (releases.INSTALL_SCRIPT_FILE, releases.DESKTOP_INSTALL_SCRIPT_FILE):
        script = releases.render_install_script(path.read_text())
        assert 'DEFAULT_REPO="example/releases"' in script
        assert "{{" not in script
        assert "BUILD_RELEASES_REPO" in script


@pytest.mark.parametrize("repo", ["", "bad", "a/b/c", "a/b'", 'a/$(id)', "a/b\ncommand", "../repo"])
def test_repository_configuration_cannot_inject_shell(repo):
    with pytest.raises(ValueError, match="owner/repository"):
        releases.releases_repo({"RELEASES_REPO": repo})
