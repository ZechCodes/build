"""Both website installers are public shell scripts without token interpolation."""

from pathlib import Path

import pytest
from litestar import Litestar
from litestar.testing import TestClient

from buildapp.root_controller import RootController


@pytest.mark.parametrize("path", ["/install.sh", "/install-desktop.sh"])
def test_anonymous_installers_ignore_legacy_tokens_and_query_overrides(monkeypatch, path):
    monkeypatch.setenv("RELEASES_REPO", "example/artifacts")
    with TestClient(Litestar([RootController])) as client:
        response = client.get(path, params={"t": "'; touch /tmp/pwned", "repo": "evil/repo"})
    assert response.status_code == 200
    assert response.headers["content-type"] == "text/plain; charset=utf-8"
    assert response.text.startswith("#!/bin/sh")
    assert 'DEFAULT_REPO="example/artifacts"' in response.text
    assert "pwned" not in response.text
    assert "evil/repo" not in response.text
    assert "{{" not in response.text


def test_image_carries_both_installers():
    containerfile = (Path(__file__).resolve().parents[1] / "Containerfile").read_text()
    for name in ("install.sh", "install-desktop.sh"):
        assert f"COPY scripts/{name} /scripts/{name}" in containerfile
