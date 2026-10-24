import json
from pathlib import Path
from unittest.mock import Mock

import pytest

from buildctl.cli import main
from buildctl.registry import load_registry
from buildctl.runtime import BuildctlError, redact

REGISTRY = Path(__file__).parents[1] / "environments.toml"


@pytest.mark.parametrize(
    "command",
    [
        ["bridge", "deploy"],
        ["bridge", "rollback"],
        ["bridge", "pair"],
        ["bridge", "prune"],
        ["app", "deploy", "HEAD"],
        ["app", "deploy", "HEAD", "--force"],
    ],
)
def test_prod_mutations_refused_before_any_execution(tmp_path, capsys, command):
    runner = Mock()
    code = main(
        ["--env", "prod", *command],
        runner=runner,
        home=tmp_path,
        registry_path=REGISTRY,
    )
    assert code == 1
    assert "Zech" in capsys.readouterr().err
    runner.run.assert_not_called()
    runner.pair.assert_not_called()
    assert list(tmp_path.iterdir()) == []


@pytest.mark.parametrize(
    "command",
    [
        ["bridge", "deploy"],
        ["bridge", "pair"],
        ["app", "deploy", "HEAD"],
    ],
)
def test_missing_environment_cannot_mutate(tmp_path, command):
    runner = Mock()
    with pytest.raises(SystemExit) as result:
        main(command, runner=runner, home=tmp_path, registry_path=REGISTRY)
    assert result.value.code == 2
    runner.run.assert_not_called()


def test_unknown_environment_refused(tmp_path, capsys):
    runner = Mock()
    assert (
        main(
            ["--env", "nightly", "status"],
            runner=runner,
            home=tmp_path,
            registry_path=REGISTRY,
        )
        == 1
    )
    assert "unknown environment" in capsys.readouterr().err
    runner.run.assert_not_called()


def test_registry_paths_and_services(tmp_path):
    envs = load_registry(REGISTRY, tmp_path)
    assert envs["dev"].binary_dir == tmp_path / ".local/state/build-dev/builds"
    assert envs["prod"].allow_mutation is False
    assert envs["dev"].service == "build-bridge-dev.service"


def test_cross_environment_and_symlink_paths_refused(tmp_path):
    env = load_registry(REGISTRY, tmp_path)["dev"]
    with pytest.raises(BuildctlError):
        env.path(tmp_path / ".build/identity.json")
    env.state_root.mkdir(parents=True)
    alias = env.state_root / "alias"
    alias.symlink_to(tmp_path / ".build", target_is_directory=True)
    with pytest.raises(BuildctlError):
        env.path(alias / "identity.json")
    with pytest.raises(BuildctlError):
        env.path(env.state_root / "../escape")


def test_registry_cannot_point_dev_root_into_prod(tmp_path):
    registry = tmp_path / "registry.toml"
    registry.write_text(
        REGISTRY.read_text().replace(
            'state_root = "~/.local/state/build-dev"', 'state_root = "~/.build/dev"'
        )
    )
    with pytest.raises(BuildctlError):
        load_registry(registry, tmp_path)


def test_registry_rejects_unknown_channels(tmp_path):
    registry = tmp_path / "registry.toml"
    registry.write_text(
        REGISTRY.read_text().replace('channel = "dev"', 'channel = "nightly"')
    )
    with pytest.raises(BuildctlError, match="channel"):
        load_registry(registry, tmp_path)


def test_prod_read_only_status_and_logs(tmp_path, capsys):
    env = load_registry(REGISTRY, tmp_path)["prod"]
    env.logs_dir.mkdir(parents=True)
    log = env.logs_dir / "bridge.log"
    log.write_text("relay: connected token=log-secret\n")
    before = sorted(path.relative_to(tmp_path) for path in tmp_path.rglob("*"))
    runner = Mock()
    runner.run.return_value.stdout = "inactive"
    runner.run.return_value.returncode = 3
    assert (
        main(
            ["--env", "prod", "bridge", "status"],
            runner=runner,
            home=tmp_path,
            registry_path=REGISTRY,
        )
        == 0
    )
    status = json.loads(capsys.readouterr().out)
    assert status["paired"] is False
    assert (
        main(
            ["--env", "prod", "bridge", "logs"],
            runner=runner,
            home=tmp_path,
            registry_path=REGISTRY,
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "connected" in output
    assert "log-secret" not in output
    assert before == sorted(path.relative_to(tmp_path) for path in tmp_path.rglob("*"))
    assert all(
        "is-active" in call.args[0] or "show" in call.args[0]
        for call in runner.run.call_args_list
    )


def test_redact_credentials(monkeypatch):
    monkeypatch.setenv("BRIDGE_UNKNOWN_SECRET", "inherited-secret")
    output = redact(
        "inherited-secret token=log-secret Bearer abc123 "
        '{"private_key":"identity-private-secret"}'
    )
    for secret in (
        "inherited-secret",
        "log-secret",
        "abc123",
        "identity-private-secret",
    ):
        assert secret not in output


def test_direct_prod_mutators_refuse(tmp_path):
    from buildctl.bridge import Bridge
    from buildctl.service import Systemd

    env = load_registry(REGISTRY, tmp_path)["prod"]
    runner = Mock()
    service = Systemd(env, runner)
    bridge = Bridge(env, runner, tmp_path, service=service)
    for mutate in (
        lambda: bridge.build("a" * 40),
        lambda: bridge.activate("a" * 40),
        service.reload,
        service.start,
        service.stop,
        service.prepare_install,
    ):
        with pytest.raises(BuildctlError, match="authorization"):
            mutate()
    runner.run.assert_not_called()
    assert list(tmp_path.iterdir()) == []


def test_deploy_prints_one_line_result(tmp_path, capsys, monkeypatch):
    bridge = Mock()
    bridge.deploy.return_value = {"current": "a" * 40, "relay": "authenticated"}
    monkeypatch.setattr("buildctl.cli.Bridge", Mock(return_value=bridge))
    assert (
        main(
            ["--env", "dev", "bridge", "deploy"],
            runner=Mock(),
            home=tmp_path,
            registry_path=REGISTRY,
        )
        == 0
    )
    lines = capsys.readouterr().out.splitlines()
    assert len(lines) == 1
    assert json.loads(lines[0])["relay"] == "authenticated"


@pytest.mark.parametrize(
    "branch,default_branch", [("main", "main"), ("trunk", "trunk")]
)
def test_mutable_registry_refuses_main_and_repository_default(
    tmp_path, branch, default_branch
):
    registry = tmp_path / "registry.toml"
    content = REGISTRY.read_text().replace('branch = "dev"', f'branch = "{branch}"')
    if default_branch != "main":
        content = content.replace(
            'default_branch = "main"', f'default_branch = "{default_branch}"', 1
        )
    registry.write_text(content)
    with pytest.raises(BuildctlError, match="protected deployment branch"):
        load_registry(registry, tmp_path)


def test_app_deploy_rechecks_protected_branch_before_any_command(tmp_path):
    from dataclasses import replace
    from buildctl.app import App

    env = replace(load_registry(REGISTRY, tmp_path)["dev"], branch="main")
    runner = Mock()
    with pytest.raises(BuildctlError, match="protected deployment branch"):
        App(env, runner, tmp_path).deploy("HEAD", force=True)
    runner.run.assert_not_called()
