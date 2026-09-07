"""``GET /install.sh`` — the target of the one-liner a member copies.

The api serves the script that ships in its own image, filled in for this deployment:
its own origin, and the caller's download token. It never redirects to GitHub (the
repository may be private) and it never asks the database whether the token is any good
— the download routes are the one judge of that. It does refuse a token that is not
even shaped like one, because the value is substituted into a shell script."""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest
from litestar import Litestar
from litestar.status_codes import HTTP_200_OK, HTTP_400_BAD_REQUEST
from litestar.testing import TestClient

from buildapp import email_message, releases
from buildapp.db_test_support import in_memory_session_maker, asgi_app
from buildapp.email_message import provide_public_base_url
from buildapp.email_test_support import PUBLIC_BASE_URL, email_settings
from buildapp.root_controller import RootController

INSTALL_SCRIPT_PATH = "/install.sh"
#: A well-formed token that is obviously not one.
TOKEN = "dl_" + "x" * 32
CONTAINERFILE = Path(__file__).resolve().parents[1] / "Containerfile"
IMAGE_COPY_LINE = "COPY scripts/install.sh /scripts/install.sh"
SLOTTED_SCRIPT = (
    "#!/bin/sh\n"
    "API_BASE_URL='{{api_base_url}}'\n"
    "DOWNLOAD_TOKEN='{{download_token}}'\n"
    "echo done\n"
)


@pytest.fixture()
def client(monkeypatch) -> Iterator[TestClient]:
    monkeypatch.setattr(email_message, "get_settings", email_settings)
    app = asgi_app([RootController], session_maker=in_memory_session_maker())
    with TestClient(app=app) as test_client:
        yield test_client


@pytest.fixture()
def slotted_script(monkeypatch, tmp_path) -> Path:
    """Stands in for the shipped script, carrying the two slots the api fills. The
    script itself is the install stream's file; what is pinned here is what this route
    does to it."""
    script = tmp_path / "install.sh"
    script.write_text(SLOTTED_SCRIPT)
    monkeypatch.setattr(releases, "INSTALL_SCRIPT_FILE", script)
    return script


def test_the_script_is_served_from_the_image_not_redirected_to_github(client):
    response = client.get(INSTALL_SCRIPT_PATH, follow_redirects=False)
    assert response.status_code == HTTP_200_OK
    assert "github.com" not in response.headers.get("location", "")
    assert response.text == releases.render_install_script(
        releases.INSTALL_SCRIPT_FILE.read_text(), PUBLIC_BASE_URL, ""
    )


def test_the_served_script_is_plain_text_that_is_never_cached(client):
    """The body carries a secret, so no cache may hold a copy of it."""
    response = client.get(INSTALL_SCRIPT_PATH)
    assert response.headers["content-type"] == "text/plain; charset=utf-8"
    assert response.headers["cache-control"] == "no-store"


def test_this_deployments_own_origin_is_filled_in(client, slotted_script):
    body = client.get(INSTALL_SCRIPT_PATH).text
    assert f"API_BASE_URL='{PUBLIC_BASE_URL}'" in body


def test_the_token_in_the_url_lands_in_the_scripts_token_line(client, slotted_script):
    body = client.get(INSTALL_SCRIPT_PATH, params={"t": TOKEN}).text
    assert f"DOWNLOAD_TOKEN='{TOKEN}'" in body


def test_a_script_asked_for_without_a_token_carries_an_empty_one(
    client, slotted_script
):
    """Someone reading the script before copying the line gets a runnable, tokenless
    copy — install.sh is what tells them the line is missing its token."""
    body = client.get(INSTALL_SCRIPT_PATH).text
    assert "DOWNLOAD_TOKEN=''" in body


def test_no_placeholder_ever_survives_into_a_served_script(client, slotted_script):
    assert "{{" not in client.get(INSTALL_SCRIPT_PATH).text
    assert "{{" not in client.get(INSTALL_SCRIPT_PATH, params={"t": TOKEN}).text


def test_the_shipped_script_leaves_no_placeholder_either(client):
    assert "{{" not in client.get(INSTALL_SCRIPT_PATH).text


@pytest.mark.parametrize(
    "token",
    ["'; rm -rf /", "dl_short", "not-a-token", "dl_" + "x" * 31 + "'"],
    ids=["shell escape", "too short", "wrong prefix", "quote"],
)
def test_a_token_that_is_not_even_shaped_like_one_is_refused_and_serves_nothing(
    client, token
):
    response = client.get(INSTALL_SCRIPT_PATH, params={"t": token})
    assert response.status_code == HTTP_400_BAD_REQUEST
    assert response.json()["detail"] == "malformed download token"
    assert "#!/bin/sh" not in response.text


def test_the_route_takes_only_this_deployments_origin(client):
    """No repository, no database: the download routes judge the token, this one
    renders."""
    app = Litestar(route_handlers=[RootController], openapi_config=None)
    handler = next(iter(app.route_handler_method_map[INSTALL_SCRIPT_PATH].values()))
    assert sorted(handler.resolve_dependencies()) == ["public_base_url"]
    assert (
        RootController.dependencies["public_base_url"].dependency
        is provide_public_base_url
    )
    assert "releases_repo" not in RootController.dependencies


def test_the_image_ships_the_script_the_route_serves():
    """``INSTALL_SCRIPT_FILE`` resolves to /scripts/install.sh in the image, so the
    Containerfile has to put it there."""
    assert IMAGE_COPY_LINE in CONTAINERFILE.read_text()
    assert releases.INSTALL_SCRIPT_FILE.is_file()
