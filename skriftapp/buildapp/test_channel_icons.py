"""Image assembly picks the public favicons once, alongside the SPA channel."""

from pathlib import Path
import struct

import pytest

from buildapp.channel_icons import install_landing_icons
from buildapp.landing_page import render_shell
from buildapp.root_controller import RootController
from buildapp.test_passkey_signin_page import render

ROOT = Path(__file__).resolve().parents[2]
DEV_ICONS = ROOT / "spa" / "channel-icons"


@pytest.fixture
def app_dir(tmp_path):
    root = tmp_path / "buildapp"
    landing = root / "landing"
    landing.mkdir(parents=True)
    for extension in ("png", "svg"):
        source = ROOT / "skriftapp" / "buildapp" / "landing" / f"favicon.{extension}"
        (landing / f"favicon.{extension}").write_bytes(source.read_bytes())
    static = root / "static"
    static.mkdir()
    for extension in ("png", "svg"):
        (static / f"favicon-dev.{extension}").write_bytes(
            (DEV_ICONS / f"favicon-dev.{extension}").read_bytes()
        )
    return root


@pytest.mark.parametrize("app_config", ["app.yaml", "app.dev.yaml", ""])
def test_non_channel_images_keep_existing_favicons_byte_for_byte(app_dir, app_config):
    before = {path: path.read_bytes() for path in app_dir.rglob("*") if path.is_file()}
    install_landing_icons(app_config, app_dir)
    assert {path: path.read_bytes() for path in app_dir.rglob("*") if path.is_file()} == before


def test_dev_image_serves_the_spa_dev_icons_for_sign_in_and_public_shell(app_dir, monkeypatch):
    install_landing_icons("app.channel-dev.yaml", app_dir)
    monkeypatch.setattr("buildapp.root_controller.LANDING_DIR", app_dir / "landing")
    html_pages = [render(), render_shell(title="Docs", description="", body="", scripts="")]
    for html in html_pages:
        for extension in ("png", "svg"):
            assert f'href="/landing/favicon.{extension}"' in html
            response = RootController.landing_asset.fn(None, f"favicon.{extension}")
            assert response.content == (app_dir / "static" / f"favicon-dev.{extension}").read_bytes()


def test_dev_favicon_preserves_svg_geometry_and_has_a_32_pixel_png():
    prod = (ROOT / "spa" / "public" / "favicon.svg").read_text()
    inverted = prod.replace("#5EF5B5", "MINT").replace("#000000", "#5EF5B5").replace("MINT", "#000000")
    assert (DEV_ICONS / "favicon-dev.svg").read_text() == inverted
    png = (DEV_ICONS / "favicon-dev.png").read_bytes()
    assert png[:8] == b"\x89PNG\r\n\x1a\n"
    assert struct.unpack(">II", png[16:24]) == (32, 32)


def test_image_installs_channel_icons_after_copying_both_static_and_landing_assets():
    source = (Path(__file__).resolve().parents[1] / "Containerfile").read_text()
    assembly = source.index('python -m buildapp.channel_icons "$APP_CONFIG"')
    assert source.index("COPY skriftapp/buildapp ./buildapp") < assembly
    assert source.index("COPY --from=spa /build/repo/skriftapp/buildapp/static") < assembly
    assert source.index("ARG APP_CONFIG=app.yaml") < assembly
