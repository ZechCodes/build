from unittest.mock import Mock

import pytest

from buildctl.cli import main


def test_secrets_execs_launch_binary_without_loading_registry(monkeypatch):
    execute = Mock()
    monkeypatch.setenv("BRIDGE_SECRET_BRIDGE", "/launch/bin/build-bridge")
    monkeypatch.setattr("os.execv", execute)
    assert main(["--env", "prod", "secrets", "get", "--id", "sec_fixture"]) == 0
    execute.assert_called_once_with(
        "/launch/bin/build-bridge",
        ["/launch/bin/build-bridge", "secrets", "get", "--env", "prod", "--id", "sec_fixture"],
    )


def test_secrets_missing_launch_fails_closed(monkeypatch, capsys):
    monkeypatch.delenv("BRIDGE_SECRET_BRIDGE", raising=False)
    assert main(["--env", "dev", "secrets", "get", "--id", "sec_fixture"]) == 1
    assert capsys.readouterr().err == "unauthorized\n"


def test_secrets_malformed_arguments_use_fixed_diagnostic(capsys):
    with pytest.raises(SystemExit) as result:
        main(["--env", "dev", "secrets", "get", "--unexpected", "private-request"])
    assert result.value.code == 2
    assert capsys.readouterr().err == "invalid_arguments\n"
