"""The landing build's unlisted lab pages (#311): tuning previews the build emits under
``generated/lab/<name>/index.html``, served at ``/lab/<name>/`` and nowhere else
listed. They are documents for a person who was sent the link, so every answer
tells crawlers to keep out, and only names the build could have written are read."""

from __future__ import annotations

from pathlib import Path

import pytest
from litestar import Litestar
from litestar.testing import TestClient

from buildapp.root_controller import GENERATED_LAB_DIR, LANDING_DIR, RootController

LAB_DOCUMENT = "<!doctype html><html><head><title>Lab</title></head><body>field</body></html>\n"


@pytest.fixture
def lab_dir(monkeypatch, tmp_path) -> Path:
    """A built lab page on disk, standing in for `cd landing && npm run build`."""
    lab = tmp_path / "generated" / "lab"
    page = lab / "notifications-0a1b2c" / "index.html"
    page.parent.mkdir(parents=True)
    page.write_text(LAB_DOCUMENT)
    (tmp_path / "generated" / "index.html").write_text("<!doctype html>home")
    monkeypatch.setattr("buildapp.root_controller.GENERATED_LAB_DIR", lab)
    return lab


def test_lab_pages_are_read_from_the_astro_build_output():
    assert GENERATED_LAB_DIR == LANDING_DIR / "generated" / "lab"


@pytest.mark.parametrize("path", ["/lab/notifications-0a1b2c/", "/lab/notifications-0a1b2c"])
def test_a_built_lab_page_is_served_as_a_document_crawlers_are_told_to_skip(lab_dir, path):
    with TestClient(Litestar([RootController])) as client:
        response = client.get(path, params={"v": "a", "speed": "0.5"})
    assert response.status_code == 200
    assert response.headers["content-type"].startswith("text/html")
    assert response.headers["x-robots-tag"] == "noindex, nofollow"
    assert response.text == LAB_DOCUMENT


@pytest.mark.parametrize(
    "path",
    [
        "/lab/notifications-ffffff/",
        "/lab/../index.html",
        "/lab/%2e%2e/",
        "/lab/..%2fgenerated/",
        "/lab/Notifications-0a1b2c/",
        "/lab/notifications-0a1b2c.html",
        "/lab/",
    ],
)
def test_anything_but_a_built_lab_page_is_not_found(lab_dir, path):
    with TestClient(Litestar([RootController])) as client:
        response = client.get(path)
    assert response.status_code == 404
    assert "home" not in response.text


def test_the_image_ships_the_whole_build_output_lab_pages_included():
    containerfile = (Path(__file__).resolve().parents[1] / "Containerfile").read_text()
    assert (
        "COPY --from=landing /build/repo/skriftapp/buildapp/landing/generated "
        "./buildapp/landing/generated"
    ) in containerfile
