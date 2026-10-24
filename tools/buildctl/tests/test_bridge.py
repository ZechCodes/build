"""All side effects use a temporary home and injected command/service runners."""

import json
import os
from pathlib import Path
import subprocess

import pytest

from buildctl.bridge import Bridge
from buildctl.registry import load_registry
from buildctl.runtime import BuildctlError
from buildctl.service import Systemd

A, B = "a" * 40, "b" * 40
REGISTRY = Path(__file__).parents[1] / "environments.toml"


class FakeRunner:
    def __init__(self, home):
        self.home = home
        self.calls = []
        self.sha = A
        self.proc = home / "proc"
        (self.proc / "123").mkdir(parents=True)
        self.exe = self.proc / "123/exe"
        self.installed = None
        self.auth = True
        self.active = True
        self.install_ok = True
        self.build_ok = True
        self.ancestor = True
        self.mismatch = False
        self.pairs = []
        self.rotate = False
        self.old_auth_on_stop = False
        self.stop_seen = False

    def run(self, args, *, cwd=None, env=None, check=True):
        self.calls.append((args, cwd, env))
        output, code = "", 0
        if "rev-parse" in args:
            output = self.sha
        elif "merge-base" in args:
            code = 0 if self.ancestor else 1
        elif "worktree" in args:
            self.worktree(args)
        elif "cargo" in args:
            code = self.cargo(args, cwd, env)
        elif args[0] == "systemctl":
            output, code = self.systemctl(args)
        elif args[-1] == "install-service":
            self.installed = Path(args[0])
            assert {k for k in env if k.startswith("BRIDGE_")} == {
                "BRIDGE_CHANNEL",
                "BRIDGE_API_URL",
                "BRIDGE_WEB_URL",
                "BRIDGE_RELAY_URL",
            }
            assert env["BRIDGE_CHANNEL"] == "dev"
            code = 0 if self.install_ok else 1
        result = subprocess.CompletedProcess(args, code, output, "secret stderr")
        if check and code:
            raise BuildctlError("command failed")
        return result

    def worktree(self, args):
        if "add" in args:
            (Path(args[-2]) / "bridge").mkdir(parents=True)
            self.worktree_path = Path(args[-2])
        elif "remove" in args:
            import shutil

            shutil.rmtree(args[-1])

    def cargo(self, args, cwd, env):
        assert args[:4] == ["nice", "-n", "10", "cargo"]
        assert args[4:] == ["build", "--release", "--locked", "--bin", "build-bridge"]
        assert cwd.name == "bridge"
        if not self.build_ok:
            return 1
        binary = Path(env["CARGO_TARGET_DIR"]) / "release/build-bridge"
        binary.parent.mkdir(parents=True)
        binary.write_text("fake binary " + self.sha)
        binary.chmod(0o700)
        return 0

    def systemctl(self, args):
        assert args[1] == "--user"
        if args[2] == "daemon-reload":
            return "", 0
        assert args[-1] == "build-bridge-dev.service"
        if args[2] == "is-active":
            return ("active", 0) if self.active else ("inactive", 3)
        if args[2] == "show":
            return "123", 0
        if args[2] == "stop":
            self.stop_seen = True
            if self.old_auth_on_stop:
                self.append_auth()
        if args[2] == "start":
            assert self.stop_seen
            self.exe.unlink(missing_ok=True)
            if self.mismatch:
                target = self.home / "wrong-binary"
                target.write_text("wrong")
            else:
                target = self.installed
            self.exe.symlink_to(target)
            if self.auth:
                self.append_auth()
        return "", 0

    def append_auth(self):
        log = self.home / ".local/state/build-dev/log/bridge.err.log"
        log.parent.mkdir(parents=True, exist_ok=True)
        if self.rotate and log.exists():
            log.rename(log.with_suffix(".old"))
        with log.open("a") as stream:
            stream.write("relay: authenticated\n")

    def pair(self, binary, *, env, approve_url):
        assert env["BRIDGE_CHANNEL"] == "dev"
        assert "BRIDGE_UNKNOWN_SECRET" not in env
        self.pairs.append(binary)
        return approve_url + "/approve-code"


@pytest.fixture
def setup(tmp_path):
    home = tmp_path / "home"
    home.mkdir()
    env = load_registry(REGISTRY, home)["dev"]
    runner = FakeRunner(home)
    service = Systemd(env, runner, proc_root=runner.proc)
    bridge = Bridge(
        env,
        runner,
        tmp_path / "repo",
        service=service,
        verify_timeout=0.01,
        poll_interval=0,
    )
    return bridge, runner, env


def test_deploy_status_rollback(setup):
    bridge, runner, env = setup
    assert bridge.deploy()["current"] == A
    runner.sha = B
    bridge.deploy(B)
    assert bridge.records() == {"current": B, "previous": A}
    env.state_root.joinpath("identity.json").write_text("identity-private-secret")
    status = bridge.status()
    assert status["running"]["commit"] == B
    assert status["running"]["managed"] is True
    assert status["paired"] is True
    assert bridge.rollback()["current"] == A
    assert bridge.records() == {"current": A, "previous": B}
    assert not runner.worktree_path.exists()
    assert all("build-bridge.service" not in args for args, _, _ in runner.calls)
    assert not env.home.joinpath(".build").exists()


def test_inherited_bridge_vars_stripped(setup, monkeypatch):
    bridge, runner, env = setup
    monkeypatch.setenv("BRIDGE_CHANNEL", "prod")
    monkeypatch.setenv("BRIDGE_API_URL", "https://prod.invalid")
    monkeypatch.setenv("BRIDGE_UNKNOWN_SECRET", "inherited-secret")
    monkeypatch.setenv("BRIDGE_IDENTITY_FILE", str(env.home / ".build/identity.json"))
    bridge.deploy()
    bridge.pair()
    child = next(
        values for args, _, values in runner.calls if args[-1] == "install-service"
    )
    assert "inherited-secret" not in str(child)
    assert child["BRIDGE_API_URL"] == "https://dev.getbuild.ing"
    assert os.environ["BRIDGE_CHANNEL"] == "prod"


@pytest.mark.parametrize("failure", ["auth", "active", "mismatch"])
def test_verification_failure_preserves_records(setup, failure):
    bridge, runner, _ = setup
    bridge.deploy()
    runner.sha = B
    setattr(runner, failure, failure == "mismatch")
    with pytest.raises(BuildctlError, match="verification"):
        bridge.deploy(B)
    assert bridge.records() == {"current": A, "previous": None}


def test_first_run_pairs_staged_candidate(setup):
    bridge, runner, _ = setup
    runner.install_ok = False
    with pytest.raises(BuildctlError, match="pair"):
        bridge.deploy()
    assert bridge.records() == {"current": None, "previous": None}
    assert "/approve-code" in bridge.pair()
    runner.install_ok = True
    bridge.deploy()
    assert bridge.records() == {"current": A, "previous": None}
    with pytest.raises(BuildctlError, match="previous"):
        bridge.rollback()


def test_unmanaged_legacy_takeover_and_prune(setup):
    bridge, runner, env = setup
    legacy = env.home / ".local/lib/build-dev/be57ec11/build-bridge"
    legacy.parent.mkdir(parents=True)
    legacy.write_text("legacy binary")
    runner.exe.symlink_to(legacy)
    status = bridge.status()
    assert status["running"]["managed"] is False
    assert status["running"]["executable"] == str(legacy)
    assert status["running"]["sha256"]
    assert status["current"] is None
    bridge.deploy()
    assert bridge.status()["running"]["commit"] == A
    assert bridge.records()["previous"] is None
    bridge.prune(0)
    assert legacy.read_text() == "legacy binary"


def test_old_process_auth_does_not_verify_new_process(setup):
    bridge, runner, _ = setup
    bridge.deploy()
    runner.sha = B
    runner.old_auth_on_stop = True
    runner.auth = False
    with pytest.raises(BuildctlError, match="verification"):
        bridge.deploy(B)


def test_rotated_log_auth_is_accepted(setup):
    bridge, runner, _ = setup
    bridge.deploy()
    runner.rotate = True
    runner.sha = B
    bridge.deploy(B)


def test_old_ref_refused_before_build_or_install(setup):
    bridge, runner, _ = setup
    runner.ancestor = False
    with pytest.raises(BuildctlError, match="channel"):
        bridge.deploy("old")
    assert not any("cargo" in args for args, _, _ in runner.calls)
    assert runner.installed is None


def test_build_failure_cleans_worktree(setup):
    bridge, runner, _ = setup
    runner.build_ok = False
    with pytest.raises(BuildctlError):
        bridge.deploy()
    assert not runner.worktree_path.exists()
    assert bridge.records()["current"] is None


def test_prune_retains_records_and_newest(setup):
    bridge, runner, env = setup
    bridge.deploy()
    runner.sha = B
    bridge.deploy(B)
    for index, char in enumerate("cde"):
        path = env.binary_dir / (char * 40)
        path.mkdir()
        os.utime(path, (100 + index, 100 + index))
    for sha in (A, B):
        os.utime(env.binary_dir / sha, (1, 1))
    bridge.prune(1)
    assert {p.name for p in env.binary_dir.iterdir()} == {A, B, "e" * 40}


def test_corrupt_and_symlinked_records_refused(setup):
    bridge, _, env = setup
    env.state_root.mkdir(parents=True)
    record = env.state_root / "buildctl.json"
    record.write_text(json.dumps({"current": "../../prod"}))
    with pytest.raises(BuildctlError):
        bridge.records()
    record.unlink()
    record.symlink_to(env.home / ".build/identity.json")
    with pytest.raises(BuildctlError):
        bridge.records()


def test_systemd_unit_symlink_refused_before_install(setup):
    bridge, runner, env = setup
    unit = env.home / ".config/systemd/user/build-bridge-dev.service"
    unit.parent.mkdir(parents=True)
    unit.symlink_to(env.home / ".build/private")
    with pytest.raises(BuildctlError, match="symlink"):
        bridge.deploy()
    assert runner.installed is None


def test_logs_hide_actual_identity_field_and_pem(setup, capsys):
    bridge, _, env = setup
    env.logs_dir.mkdir(parents=True)
    fake_key = "test-value"
    label = "PRIVATE KEY"
    (env.logs_dir / "bridge.log").write_text(
        json.dumps({"identity_private_key_b64": fake_key})
        + "\n"
        + f"-----BEGIN {label}-----\nprivate-pem-secret\n-----END {label}-----\n"
        + "relay: connected token_b64=private-token-secret\n"
    )
    bridge.logs()
    output = capsys.readouterr().out
    for secret in (fake_key, "private-pem-secret", "private-token-secret"):
        assert secret not in output


def test_partial_staging_copy_does_not_publish_a_binary(setup, monkeypatch):
    bridge, runner, env = setup

    def partial_copy(source, destination):
        Path(destination).write_text("partial")
        raise OSError("copy failed")

    with monkeypatch.context() as patch:
        patch.setattr("buildctl.bridge.shutil.copyfile", partial_copy)
        with pytest.raises(OSError):
            bridge.deploy()
    assert not (env.binary_dir / A / "build-bridge").exists()
    assert runner.installed is None
    bridge.deploy()
    assert bridge.records()["current"] == A


def test_management_lock_prevents_concurrent_operation(setup):
    from buildctl.bridge import management_lock

    bridge, runner, env = setup
    with management_lock(env):
        with pytest.raises(BuildctlError, match="another"):
            bridge.deploy()
    assert runner.calls == []


def test_log_tail_does_not_start_inside_private_key(setup, capsys):
    bridge, _, env = setup
    env.logs_dir.mkdir(parents=True)
    label = "PRIVATE KEY"
    (env.logs_dir / "bridge.log").write_text(
        f"-----BEGIN {label}-----\n"
        + "hidden-key-chunk\n" * 100
        + f"-----END {label}-----\nrelay: connected\n"
    )
    bridge.logs()
    output = capsys.readouterr().out
    assert "hidden-key-chunk" not in output
    assert "relay: connected" in output


def test_follow_logs_redacts_appended_credentials(setup, capsys, monkeypatch):
    bridge, _, env = setup
    env.logs_dir.mkdir(parents=True)
    log = env.logs_dir / "bridge.log"
    log.write_text("relay: existing\n")
    polls = []

    def append_once(_):
        if polls:
            raise KeyboardInterrupt
        polls.append(True)
        with log.open("a") as stream:
            stream.write("relay: new token_b64=hidden-follow-secret\n")

    monkeypatch.setattr("buildctl.bridge.time.sleep", append_once)
    with pytest.raises(KeyboardInterrupt):
        bridge.logs(follow=True)
    output = capsys.readouterr().out
    assert "relay: new" in output
    assert "hidden-follow-secret" not in output


@pytest.mark.parametrize(
    "relative",
    [
        "buildctl-candidate",
        "buildctl.json",
        f"builds/{A}/build-bridge",
    ],
)
def test_deploy_refuses_managed_file_symlinks_into_prod(setup, relative):
    bridge, runner, env = setup
    prod = env.home / ".build"
    prod.mkdir()
    sentinel = prod / "private"
    sentinel.write_text("do-not-read-or-change")
    link = env.state_root / relative
    link.parent.mkdir(parents=True, exist_ok=True)
    link.symlink_to(sentinel)
    with pytest.raises(BuildctlError, match="symlink"):
        bridge.deploy()
    assert runner.installed is None
    assert sentinel.read_text() == "do-not-read-or-change"
