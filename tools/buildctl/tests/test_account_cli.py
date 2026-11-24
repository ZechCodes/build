"""Use a fake runner to verify the dev-only account operator entry point."""

import json
import subprocess
from unittest.mock import Mock
from uuid import UUID

import pytest

from buildctl.cli import main


USER = UUID("81dc4b8d-7551-44e0-a39b-117c5241d412")
ARGS = ["account", "set", "--user-id", str(USER)]
KUBE = ["kubectl", "--context", "dev-operator", "--namespace", "build-dev"]
WHOAMI = [*KUBE, "auth", "whoami", "-o", "json"]


def test_account_set_uses_explicit_dev_context_and_authenticated_actor(tmp_path, capsys):
    runner = Mock()
    payload = {"changed": True, "dry_run": True}
    exec_args = [
        *KUBE, "exec", "deployment/build-app", "--container", "app", "--",
        "python", "-m", "buildapp.account_policy", "set", "--env", "dev",
        "--user-id", str(USER), "--actor", "kubernetes:someone@example.com",
        "--tunnel-access", "on", "--dry-run",
    ]
    runner.run.side_effect = [
        subprocess.CompletedProcess(
            WHOAMI, 0, '{"status":{"userInfo":{"username":"someone@example.com"}}}', "",
        ),
        subprocess.CompletedProcess(exec_args, 0, json.dumps(payload), ""),
    ]
    code = main(
        ["--env", "dev", *ARGS, "--context", "dev-operator", "--tunnel-access", "on", "--dry-run"],
        home=tmp_path, runner=runner,
    )
    assert code == 0
    assert [call.args[0] for call in runner.run.call_args_list] == [WHOAMI, exec_args]
    assert json.loads(capsys.readouterr().out) == payload


def test_account_set_allows_type_only_and_uses_dev_context_environment(tmp_path, monkeypatch):
    monkeypatch.setenv("DEV_CONTEXT", "dev-operator")
    runner = Mock()
    runner.run.side_effect = [
        Mock(stdout='{"status":{"userInfo":{"username":"someone@example.com"}}}'),
        Mock(stdout='{"changed": true}'),
    ]
    assert main(["--env", "dev", *ARGS, "--type", "admin"], home=tmp_path, runner=runner) == 0
    assert runner.run.call_args_list[0].args[0] == WHOAMI
    assert runner.run.call_args_list[1].args[0][-2:] == ["--type", "admin"]


@pytest.mark.parametrize("options", [[], ["--dry-run"]])
def test_account_set_refuses_prod_before_running_any_command(tmp_path, capsys, options):
    runner = Mock()
    assert main(
        ["--env", "prod", *ARGS, "--type", "admin", *options], home=tmp_path, runner=runner,
    ) == 1
    assert "refused" in capsys.readouterr().err
    runner.run.assert_not_called()


def test_account_set_requires_context_and_valid_identity_before_exec(tmp_path, monkeypatch, capsys):
    monkeypatch.delenv("DEV_CONTEXT", raising=False)
    runner = Mock()
    args = ["--env", "dev", *ARGS, "--type", "admin"]
    assert main(args, home=tmp_path, runner=runner) == 1
    assert "context" in capsys.readouterr().err
    runner.run.assert_not_called()

    runner.run.return_value = Mock(stdout='{"status":{"userInfo":{}}}')
    assert main([*args, "--context", "dev-operator"], home=tmp_path, runner=runner) == 1
    assert "identity" in capsys.readouterr().err
    runner.run.assert_called_once_with(WHOAMI)


@pytest.mark.parametrize("options", [[], ["--tunnel-access", "maybe"], ["--user-id", "broken"]])
def test_account_set_rejects_invalid_arguments(tmp_path, options):
    runner = Mock()
    with pytest.raises(SystemExit) as exit_info:
        main(["--env", "dev", *ARGS, *options], home=tmp_path, runner=runner)
    assert exit_info.value.code == 2
    runner.run.assert_not_called()
