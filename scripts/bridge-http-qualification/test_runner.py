"""Boundary checks keep qualification away from installed binaries/live listeners."""

import importlib.util
import os
from pathlib import Path
import socket
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("qualification_runner", Path(__file__).with_name("run.py"))
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class RunnerBoundaries(unittest.TestCase):
    def test_only_explicit_executable_inside_scratch_target_is_accepted(self):
        with tempfile.TemporaryDirectory() as directory:
            scratch = Path(directory)
            target = scratch / "target"
            target.mkdir()
            binary = target / "build-bridge"
            binary.write_text("disposable executable fixture\n")
            binary.chmod(0o700)
            outside = scratch / "installed-bridge"
            outside.write_text("never run\n")
            outside.chmod(0o700)
            escaped = target / "escaped-bridge"
            escaped.symlink_to(outside)
            with patch.object(runner, "SCRATCH", scratch):
                self.assertEqual(runner.qualification_binary(str(binary)), binary)
                for value in [None, "build-bridge", str(outside), str(escaped)]:
                    with self.subTest(value=value), self.assertRaises(ValueError):
                        runner.qualification_binary(value)
                binary.chmod(0o600)
                with self.assertRaises(ValueError):
                    runner.qualification_binary(str(binary))

    def test_service_start_refuses_an_existing_loopback_listener(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            port = listener.getsockname()[1]
            with self.assertRaises(OSError):
                runner.ensure_free_ports([port])

    def test_bridge_environment_excludes_inherited_authority_and_real_home(self):
        with patch.dict(os.environ, {"BRIDGE_API_URL": "https://live.invalid", "CODEX_HOME": "/private",
                "CLAUDE_CONFIG_DIR": "/private", "PI_CODING_AGENT_DIR": "/private", "DBUS_SESSION_BUS_ADDRESS": "private"}):
            run_dir = Path("/home/zech/Projects/.agent-scratch/976/run-boundary-test")
            env = runner.base_environment(run_dir)
        self.assertEqual(env["HOME"], str(run_dir / "home"))
        self.assertEqual(env["TMPDIR"], str(run_dir / "tmp"))
        for name in ["BRIDGE_API_URL", "CODEX_HOME", "CLAUDE_CONFIG_DIR", "PI_CODING_AGENT_DIR", "DBUS_SESSION_BUS_ADDRESS"]:
            self.assertNotIn(name, env)
        fixture = {"runDir": str(run_dir), "hostedOrigin": "https://localhost:18476",
            "bridgeOrigin": "https://localhost:19476", "identityFile": str(run_dir / "home/identity.json"),
            "projectsDir": str(run_dir / "home/projects"), "caFile": str(run_dir / "ca.pem")}
        bridge = runner.bridge_environment(env, fixture, 19476)
        self.assertEqual(bridge["BRIDGE_CHANNEL"], "dev")
        self.assertEqual(bridge["BRIDGE_API_URL"], fixture["hostedOrigin"])
        for name in ["BRIDGE_CONFIG", "BRIDGE_TASKS_DIR", "BRIDGE_PROJECTS_DIR", "BRIDGE_WORKTREES", "BRIDGE_MCP_SOCKET", "BRIDGE_IDENTITY_FILE"]:
            self.assertTrue(Path(bridge[name]).is_relative_to(run_dir))


if __name__ == "__main__":
    unittest.main()
