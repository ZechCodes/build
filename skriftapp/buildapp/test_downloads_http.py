"""Public download routes work without a session, database, or GitHub credentials."""

import pytest
from litestar import Litestar
from litestar.di import Provide
from litestar.testing import TestClient

from buildapp.controllers import BuildController
from buildapp import releases


@pytest.fixture()
def client(monkeypatch):
    monkeypatch.setenv("RELEASES_REPO", "example/artifacts")
    monkeypatch.setenv("GITHUB_RELEASES_TOKEN", "legacy-secret")
    monkeypatch.setitem(BuildController.dependencies, "public_base_url", Provide(
        lambda: "https://example.test", sync_to_thread=False,
    ))
    with TestClient(Litestar([BuildController])) as client:
        yield client


def test_anonymous_metadata_needs_no_database_or_membership(client):
    response = client.get("/app/downloads")
    assert response.status_code == 200
    payload = response.json()
    assert payload["install_command"] == 'curl -fsSL "https://example.test/install.sh" | sh'
    assert payload["desktop_install_command"] == 'curl -fsSL "https://example.test/install-desktop.sh" | sh'
    assert payload["desktop_releases_url"] == 'https://github.com/example/artifacts/releases'


@pytest.mark.parametrize("segment,name", list(releases.DOWNLOADABLE.items()))
def test_every_asset_is_public_even_with_invalid_legacy_token(client, segment, name):
    response = client.get(f"/app/downloads/{segment}?t=invalid", follow_redirects=False)
    assert response.status_code == 302
    assert response.headers["location"] == f"https://github.com/example/artifacts/releases/latest/download/{name}"


def test_unknown_asset_remains_a_404(client):
    assert client.get("/app/downloads/unknown").status_code == 404


def test_legacy_refresh_returns_nonexpiring_public_command_without_minting(client):
    response = client.post("/app/downloads/token")
    assert response.status_code == 201
    assert response.json() == {
        "token": "", "expires_in_s": None,
        "install_command": 'curl -fsSL "https://example.test/install.sh" | sh',
    }
