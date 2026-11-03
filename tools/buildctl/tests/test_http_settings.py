"""HTTP configuration uses temporary homes and never loads TLS key contents."""

import json
import os
from pathlib import Path
from unittest.mock import Mock

import pytest

from buildctl.bridge import Bridge
from buildctl.cli import main
from buildctl.registry import load_registry
from buildctl.runtime import BuildctlError

REGISTRY = Path(__file__).parents[1] / "environments.toml"
SHA = "a" * 40


@pytest.fixture
def configured(tmp_path):
    env = load_registry(REGISTRY, tmp_path)["dev"]
    env.state_root.mkdir(parents=True)
    cert, key = env.state_root / "cert.pem", env.state_root / "key.pem"
    cert.touch(mode=0o600)
    key.touch(mode=0o600)
    values = {
        "bind": "127.0.0.1:9443",
        "origin": "https://bridge.example:9443",
        "tls_cert": str(cert),
        "tls_key": str(key),
        "allowed_origins": ["https://dev.getbuild.ing", "https://localhost:8443"],
    }
    write_config(env, values)
    return env, values


def write_config(env, values):
    path = env.state_root / "http.toml"
    path.write_text(
        "\n".join(f"{key} = {json.dumps(value)}" for key, value in values.items())
    )
    path.chmod(0o600)
    return path


def test_absent_config_strips_inherited_http_without_creating_state(tmp_path):
    env = load_registry(REGISTRY, tmp_path)["dev"]
    inherited = {"BRIDGE_HTTP_ENABLED": "true", "BRIDGE_HTTP_TLS_KEY": "ignored"}
    child = env.bridge_env(inherited)
    assert not any(key.startswith("BRIDGE_HTTP_") for key in child)
    assert inherited["BRIDGE_HTTP_ENABLED"] == "true"
    assert not env.state_root.exists()


def test_configured_environment_and_install(configured, monkeypatch):
    env, values = configured
    original_open = Path.open

    def refuse_key_read(path, *args, **kwargs):
        assert path != Path(values["tls_key"]), "buildctl must never open the TLS key"
        return original_open(path, *args, **kwargs)

    monkeypatch.setattr(Path, "open", refuse_key_read)
    expected = {
        "BRIDGE_HTTP_ENABLED": "true",
        "BRIDGE_HTTP_BIND": values["bind"],
        "BRIDGE_HTTP_ORIGIN": values["origin"],
        "BRIDGE_HTTP_TLS_CERT": values["tls_cert"],
        "BRIDGE_HTTP_TLS_KEY": values["tls_key"],
        "BRIDGE_HTTP_ALLOWED_ORIGINS": ",".join(values["allowed_origins"]),
    }
    child = env.bridge_env({"BRIDGE_HTTP_BIND": "ignored", "KEEP": "yes"})
    assert {
        key: value for key, value in child.items() if key.startswith("BRIDGE_HTTP_")
    } == expected
    assert child["KEEP"] == "yes"
    binary = env.binary_dir / SHA / "build-bridge"
    binary.parent.mkdir(parents=True)
    binary.write_text("fake executable; never run")
    binary.chmod(0o700)
    runner, service = Mock(), Mock()
    runner.run.return_value.returncode = 0
    bridge = Bridge(env, runner, env.home, service=service)
    bridge.verify = Mock()
    bridge.activate(SHA)
    install = runner.run.call_args
    assert install.args[0] == [str(binary), "install-service"]
    assert all(install.kwargs["env"][key] == value for key, value in expected.items())


def test_optional_allowlist_and_ipv6(configured):
    env, values = configured
    values.pop("allowed_origins")
    values["bind"] = "[::1]:9443"
    values["origin"] = "https://[::1]:9443"
    write_config(env, values)
    child = env.bridge_env({})
    assert child["BRIDGE_HTTP_BIND"] == "[::1]:9443"
    assert "BRIDGE_HTTP_ALLOWED_ORIGINS" not in child


INVALID_VALUES = [
    ("origin", "http://bridge.example"),
    ("origin", "https://bridge.example/path"),
    ("origin", "https://bridge.example/"),
    ("origin", "https://*.example"),
    ("origin", "https://bridge.example:0"),
    ("origin", "https://user@bridge.example"),
    ("origin", "https://bridge.example?query"),
    ("origin", "https://bridge.example#fragment"),
    ("origin", "https://bridge.example:\\bad"),
    ("origin", 42),
    ("bind", "*:9443"),
    ("bind", "127.0.0.1:0"),
    ("bind", "localhost:9443"),
    ("bind", "::1:9443"),
    ("bind", "127.0.0.1:65536"),
    ("bind", 42),
    ("tls_cert", "/outside/cert.pem"),
    ("tls_key", "/outside/key.pem"),
    ("tls_cert", "cert.pem"),
    ("tls_key", "key.pem"),
    ("tls_key", 42),
    ("unknown", "value"),
    ("allowed_origins", ["*"]),
    ("allowed_origins", ["http://app.example"]),
    ("allowed_origins", ["https://app.example/path"]),
    ("allowed_origins", "https://app.example"),
    ("allowed_origins", []),
    ("allowed_origins", ["https://app.example"] * 17),
    ("allowed_origins", [42]),
]


@pytest.mark.parametrize("field,value", INVALID_VALUES)
def test_invalid_settings_refused_before_mutation(configured, field, value):
    env, values = configured
    values[field] = value
    write_config(env, values)
    assert_refused_without_mutation(env)


def assert_refused_without_mutation(env):
    before = sorted(env.home.rglob("*"))
    runner, service = Mock(), Mock()
    bridge = Bridge(env, runner, env.home, service=service)
    for operation in (
        bridge.deploy,
        bridge.rollback,
        bridge.pair,
        lambda: bridge.activate(SHA),
    ):
        with pytest.raises(BuildctlError):
            operation()
    runner.run.assert_not_called()
    runner.pair.assert_not_called()
    assert service.mock_calls == []
    assert before == sorted(env.home.rglob("*"))


@pytest.mark.parametrize("field", ["bind", "origin", "tls_cert", "tls_key"])
def test_missing_required_setting(configured, field):
    env, values = configured
    values.pop(field)
    write_config(env, values)
    assert_refused_without_mutation(env)


@pytest.mark.parametrize("field", ["tls_cert", "tls_key", "http.toml"])
def test_symlink_refused(configured, field):
    env, values = configured
    path = env.state_root / "http.toml" if field == "http.toml" else Path(values[field])
    target = env.state_root / "original"
    path.rename(target)
    path.symlink_to(target)
    assert_refused_without_mutation(env)


@pytest.mark.parametrize("name", ["http.toml", "key.pem"])
@pytest.mark.parametrize("mode", [0o644, 0o400, 0o700])
def test_permissions_must_be_0600(configured, name, mode):
    env, _ = configured
    (env.state_root / name).chmod(mode)
    assert_refused_without_mutation(env)


def test_config_owner_must_match_current_user(configured, monkeypatch):
    env, _ = configured
    monkeypatch.setattr(
        os, "getuid", lambda: (env.state_root / "http.toml").stat().st_uid + 1
    )
    assert_refused_without_mutation(env)


@pytest.mark.parametrize("name", ["http.toml", "cert.pem", "key.pem"])
def test_nonregular_files_refused(configured, name):
    env, _ = configured
    path = env.state_root / name
    path.unlink()
    path.mkdir(mode=0o600)
    assert_refused_without_mutation(env)


def test_malformed_toml_is_plain_buildctl_error(configured):
    env, _ = configured
    (env.state_root / "http.toml").write_text("bind = [")
    assert_refused_without_mutation(env)


def test_status_exposes_only_public_http_configuration(configured, capsys):
    env, values = configured
    runner = Mock()
    runner.run.return_value.stdout = "inactive"
    runner.run.return_value.returncode = 3
    assert (
        main(
            ["--env", "dev", "bridge", "status"],
            runner=runner,
            home=env.home,
            registry_path=REGISTRY,
        )
        == 0
    )
    output = capsys.readouterr().out
    assert json.loads(output)["http"] == {
        "configured": True,
        "origin": values["origin"],
        "bind": values["bind"],
    }
    assert "pem" not in output
    (env.state_root / "http.toml").unlink()
    assert (
        main(
            ["--env", "dev", "bridge", "status"],
            runner=runner,
            home=env.home,
            registry_path=REGISTRY,
        )
        == 0
    )
    assert json.loads(capsys.readouterr().out)["http"] == {"configured": False}


def test_prod_still_refused(tmp_path):
    env = load_registry(REGISTRY, tmp_path)["prod"]
    with pytest.raises(BuildctlError, match="authorization"):
        env.bridge_env({})
    assert list(tmp_path.iterdir()) == []
