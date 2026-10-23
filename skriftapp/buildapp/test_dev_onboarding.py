"""Dev onboarding never offers or serves production installers or artifacts."""
import pytest
from litestar import Litestar
from litestar.di import Provide
from litestar.testing import TestClient
from buildapp import releases
from buildapp.controllers import BuildController
from buildapp.invite_mail import build_invite_email
from buildapp.root_controller import RootController

DEV_URL = "https://dev.getbuild.ing"

@pytest.fixture()
def dev_client(monkeypatch):
    for controller in (BuildController, RootController):
        monkeypatch.setitem(controller.dependencies, "public_base_url", Provide(
            lambda: DEV_URL, sync_to_thread=False))
    with TestClient(Litestar([BuildController, RootController])) as client:
        yield client


def test_actual_dev_metadata_withholds_all_installation_paths(dev_client):
    response = dev_client.get("/app/downloads")
    assert response.status_code == 200
    payload = response.json()
    for key in ("install_command", "install_script_url", "desktop_install_command",
                "desktop_install_script_url", "releases_url", "desktop_releases_url", "checksums_url"):
        assert payload[key] is None
    assert payload["platforms"] == []
    assert "source-built" in payload["installation_notice"]
    assert payload["docs_url"] == "/docs#setup"
    assert releases.install_command(DEV_URL) is None
    assert releases.install_command(DEV_URL, desktop=True) is None


@pytest.mark.parametrize("path", ["/install.sh", "/install-desktop.sh",
                         *[f"/app/downloads/{key}" for key in releases.DOWNLOADABLE]])
def test_dev_routes_never_deliver_prod_installer_or_release(dev_client, monkeypatch, path):
    original_read = type(releases.INSTALL_SCRIPT_FILE).read_text
    def unexpected_read(path, *args, **kwargs):
        if path in (releases.INSTALL_SCRIPT_FILE, releases.DESKTOP_INSTALL_SCRIPT_FILE):
            pytest.fail("dev must refuse before reading installer scripts")
        return original_read(path, *args, **kwargs)
    monkeypatch.setattr(type(releases.INSTALL_SCRIPT_FILE), "read_text", unexpected_read)
    response = dev_client.get(path, follow_redirects=False)
    assert response.status_code == 404
    assert "location" not in response.headers


def test_legacy_dev_token_caller_gets_no_install_command(dev_client):
    response = dev_client.post("/app/downloads/token")
    assert response.status_code == 201
    assert response.json()["install_command"] is None


def test_dev_invite_withholds_prod_install_and_pairing_instructions():
    message = build_invite_email(to="dev@example.test", invite_url=f"{DEV_URL}/invite/test",
                                 public_base_url=DEV_URL)
    for body in (message.text_body, message.html_body):
        assert "source-built" in body
        assert "separate" in body
        assert "curl" not in body
        assert "install.sh" not in body
        assert "https://getbuild.ing" not in body
        assert "enter the pairing code" not in body


def test_dev_docs_withhold_production_installers(dev_client, monkeypatch):
    monkeypatch.setattr("buildapp.landing_content.load_content", lambda: {"harnesses": []})
    response = dev_client.get("/docs")
    assert response.status_code == 200
    assert "source-built" in response.text
    assert "separate" in response.text
    assert "curl" not in response.text
    assert "install.sh" not in response.text
