"""Download redirects always use the public artifact repository."""

import asyncio

from buildapp.release_assets import PublicAssets, asset_source, provide_asset_source


def test_legacy_credentials_do_not_switch_to_private_source():
    source = asset_source({"GITHUB_RELEASES_TOKEN": "legacy-secret"})
    assert isinstance(source, PublicAssets)
    response = asyncio.run(source.deliver("SHA256SUMS"))
    assert response.url == "https://github.com/ZechCodes/build-releases/releases/latest/download/SHA256SUMS"


def test_provider_respects_repository_override(monkeypatch):
    monkeypatch.setenv("RELEASES_REPO", "example/artifacts")
    source = provide_asset_source()
    assert source.releases_url == "https://github.com/example/artifacts/releases/latest"
    assert asyncio.run(source.deliver("SHA256SUMS")).url == (
        "https://github.com/example/artifacts/releases/latest/download/SHA256SUMS"
    )
