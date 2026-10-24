"""CLI cutover routing and persistent shared roots without service mutations."""

import json
from pathlib import Path
from unittest.mock import Mock

import pytest

from buildctl.cli import main
from buildctl.registry import load_registry
from buildctl.runtime import BuildctlError

REGISTRY = Path(__file__).parents[1] / "environments.toml"


@pytest.mark.parametrize("flags", [[], ["--dry-run"], ["--force", "--prod-stopped"]])
def test_migrate_routes_without_environment(tmp_path, monkeypatch, capsys, flags):
    migrator = Mock()
    migrator.run.return_value = {"tables": {"tasks": 4}, "backup": "/backup"}
    factory = Mock(return_value=migrator)
    monkeypatch.setattr("buildctl.cli.Migrator", factory, raising=False)
    runner = Mock()
    runner.run.return_value.stdout = "inactive"
    assert (
        main(
            ["migrate", "--from", "prod", "--to", "dev", *flags],
            home=tmp_path,
            runner=runner,
            registry_path=REGISTRY,
        )
        == 0
    )
    args, kwargs = factory.call_args
    assert args == (tmp_path / ".build", tmp_path / ".local/state/build-dev")
    assert kwargs["source_prefix"] == tmp_path / ".build"
    assert kwargs["projects_dir"] == tmp_path / "Projects"
    assert kwargs["worktrees"] == tmp_path / "Projects/.build-worktrees"
    assert kwargs["states"]() == {"prod": "inactive", "dev": "inactive"}
    assert all("is-active" in call.args[0] for call in runner.run.call_args_list)
    migrator.run.assert_called_once_with(
        dry_run="--dry-run" in flags,
        force="--force" in flags,
        prod_stopped="--prod-stopped" in flags,
    )
    assert json.loads(capsys.readouterr().out)["tables"]["tasks"] == 4


def test_migrate_routes_rollback(tmp_path, monkeypatch):
    migrator = Mock()
    migrator.rollback.return_value = {"restored": True}
    monkeypatch.setattr(
        "buildctl.cli.Migrator", Mock(return_value=migrator), raising=False
    )
    assert (
        main(
            ["migrate", "--from", "prod", "--to", "dev", "--rollback", "/backup"],
            home=tmp_path,
            runner=Mock(),
            registry_path=REGISTRY,
        )
        == 0
    )
    migrator.rollback.assert_called_once_with(Path("/backup"))
    migrator.run.assert_not_called()


@pytest.mark.parametrize(
    "argv",
    [
        ["--env", "dev", "migrate", "--from", "prod", "--to", "dev"],
        ["migrate", "--from", "dev", "--to", "prod"],
        ["migrate", "--from", "prod"],
        [
            "migrate",
            "--from",
            "prod",
            "--to",
            "dev",
            "--rollback",
            "/backup",
            "--dry-run",
        ],
    ],
)
def test_ambiguous_or_unsupported_migrations_refuse_before_execution(tmp_path, argv):
    runner = Mock()
    with pytest.raises(SystemExit) as result:
        main(argv, home=tmp_path, runner=runner, registry_path=REGISTRY)
    assert result.value.code == 2
    runner.run.assert_not_called()


def test_migrated_roots_reach_installer_environment(tmp_path):
    env = load_registry(REGISTRY, tmp_path)["dev"]
    env.state_root.mkdir(parents=True)
    roots = {
        "projects_dir": str(tmp_path / "Projects"),
        "worktrees": str(tmp_path / "Projects/.build-worktrees"),
    }
    (env.state_root / "migration-roots.json").write_text(json.dumps(roots))
    child = env.bridge_env({"BRIDGE_PROJECTS_DIR": "/ignored", "PATH": "/bin"})
    assert child["BRIDGE_PROJECTS_DIR"] == roots["projects_dir"]
    assert child["BRIDGE_WORKTREES"] == roots["worktrees"]
    assert child["PATH"] == "/bin"


@pytest.mark.parametrize("invalid", ["prod", "dev", "relative", "alias", "malformed"])
def test_migrated_roots_reject_cross_environment_paths(tmp_path, invalid):
    env = load_registry(REGISTRY, tmp_path)["dev"]
    env.state_root.mkdir(parents=True)
    root = tmp_path / "Projects"
    root.mkdir()
    if invalid == "prod":
        root = tmp_path / ".build/worktrees"
    elif invalid == "dev":
        root = env.state_root / "worktrees"
    elif invalid == "relative":
        root = Path("relative")
    elif invalid == "alias":
        alias = tmp_path / "alias"
        alias.symlink_to(root, target_is_directory=True)
        root = alias
    manifest = env.state_root / "migration-roots.json"
    manifest.write_text(json.dumps({"projects_dir": str(root), "worktrees": str(root)}))
    if invalid == "malformed":
        manifest.write_text("[]")
    with pytest.raises(BuildctlError):
        env.bridge_env({})


def test_migration_respects_read_only_target_policy(tmp_path, monkeypatch):
    registry = tmp_path / 'registry.toml'
    registry.write_text(REGISTRY.read_text().replace('allow_mutation = true', 'allow_mutation = false'))
    factory = Mock()
    monkeypatch.setattr('buildctl.cli.Migrator', factory)
    assert main(['migrate', '--from', 'prod', '--to', 'dev', '--prod-stopped'], home=tmp_path, runner=Mock(), registry_path=registry) == 1
    factory.assert_not_called()
