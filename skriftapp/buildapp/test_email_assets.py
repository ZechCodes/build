"""The images every email links to: served by this deployment at stable URLs under
/landing/email/ with a long cache, sized as the template declares them, small enough to
mail, and shipped in the image."""

from __future__ import annotations

import struct
from pathlib import Path

import pytest
from litestar import Litestar
from litestar.testing import TestClient

from buildapp.email_template import (
    EMAIL_ASSET_DIRECTORY,
    EMAIL_ASSET_PATH,
    LAPTOP_IMAGE,
    MARK_IMAGE,
    EmailImage,
)
from buildapp.landing_page import LANDING_DIR
from buildapp.root_controller import EMAIL_ASSET_CACHE_CONTROL, RootController

REPO_ROOT = Path(__file__).resolve().parents[2]
EMAIL_ASSETS = LANDING_DIR / EMAIL_ASSET_DIRECTORY
#: The task's budget for the laptop image; a mail client downloads it on every open.
IMAGE_SIZE_LIMIT_BYTES = 250 * 1024
HOSTED_FILES = (MARK_IMAGE.file, MARK_IMAGE.file_1x, LAPTOP_IMAGE.file)


def png_size(path: Path) -> tuple[int, int]:
    """Width and height from the IHDR chunk, which every PNG starts with."""
    header = path.read_bytes()[:24]
    assert header[:8] == b"\x89PNG\r\n\x1a\n", path
    return struct.unpack(">II", header[16:24])


@pytest.mark.parametrize("name", HOSTED_FILES)
def test_each_email_image_is_served_as_a_png_with_a_long_cache(name: str):
    with TestClient(Litestar([RootController])) as client:
        response = client.get(f"{EMAIL_ASSET_PATH}/{name}")
    assert response.status_code == 200
    assert response.headers["content-type"] == "image/png"
    assert response.headers["cache-control"] == EMAIL_ASSET_CACHE_CONTROL
    assert "max-age=31536000" in EMAIL_ASSET_CACHE_CONTROL
    assert response.content == (EMAIL_ASSETS / name).read_bytes()


def test_other_landing_assets_keep_their_uncached_answer():
    with TestClient(Litestar([RootController])) as client:
        response = client.get("/landing/brand-mark.svg")
    assert response.status_code == 200
    assert "cache-control" not in response.headers


@pytest.mark.parametrize("image", (MARK_IMAGE, LAPTOP_IMAGE))
def test_each_image_file_is_at_least_twice_its_display_size_at_its_aspect(
    image: EmailImage,
):
    width, height = png_size(EMAIL_ASSETS / image.file)
    assert width >= 2 * image.width_px
    assert height >= 2 * image.height_px
    assert width / height == pytest.approx(image.width_px / image.height_px, rel=0.01)


def test_the_one_x_mark_is_its_display_size():
    assert png_size(EMAIL_ASSETS / MARK_IMAGE.file_1x) == (
        MARK_IMAGE.width_px,
        MARK_IMAGE.height_px,
    )


@pytest.mark.parametrize("name", HOSTED_FILES)
def test_each_image_is_small_enough_to_mail(name: str):
    assert (EMAIL_ASSETS / name).stat().st_size < IMAGE_SIZE_LIMIT_BYTES


def test_the_app_image_ships_the_email_images():
    containerfile = (REPO_ROOT / "skriftapp" / "Containerfile").read_text()
    assert "COPY skriftapp/buildapp ./buildapp" in containerfile
    shipped = f"skriftapp/buildapp/landing/{EMAIL_ASSET_DIRECTORY}/"
    ignored = (REPO_ROOT / ".containerignore").read_text().splitlines()
    for rule in ignored:
        rule = rule.strip()
        if rule and not rule.startswith("#"):
            assert not shipped.startswith(rule.rstrip("/") + "/"), rule
