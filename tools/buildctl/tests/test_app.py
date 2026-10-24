"""App contracts use injected commands and HTTP; nothing contacts a service."""

import json
import ssl
import subprocess
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import MagicMock, patch

from buildctl.app import App, HTTP_TIMEOUT, MAX_VERSION_BYTES, default_fetch
from buildctl.runtime import BuildctlError


TIP = "a" * 40
TARGET = "b" * 40


class FakeRunner:
    def __init__(self, ancestor=True):
        self.calls = []
        self.ancestor = ancestor
        self.target = TARGET
        self.run_list = []

    def run(self, args, cwd=None, env=None, check=True):
        self.calls.append((args, cwd, env, check))
        output = ""
        code = 0
        if "rev-parse" in args:
            output = TIP if args[-1] == "FETCH_HEAD^{commit}" else self.target
        elif "merge-base" in args:
            code = 0 if self.ancestor else 1
        elif args[:3] == ["gh", "run", "list"]:
            output = json.dumps(self.run_list)
        return subprocess.CompletedProcess(args, code, output, "private stderr")


class AppTests(unittest.TestCase):
    def setUp(self):
        self.guards = []
        self.environment = SimpleNamespace(
            name="dev",
            branch="dev",
            web_url="https://dev.example.test/",
            deploy_workflow="dev-deploy.yml",
            repository="owner/project",
            require_mutation=lambda: self.guards.append("checked"),
        )
        self.runner = FakeRunner()
        self.urls = []
        self.app = App(self.environment, self.runner, Path("/checkout"), self.fetch)

    def fetch(self, url):
        self.urls.append(url)
        return {"version": TARGET, "identity": "private"}

    def commands(self):
        return [call[0] for call in self.runner.calls]

    def test_deploy_fetches_then_requires_ancestor_and_pushes_full_sha(self):
        result = self.app.deploy("feature")
        self.assertEqual(self.guards, ["checked"])
        self.assertEqual(result["commit"], TARGET)
        self.assertEqual(
            self.commands(),
            [
                [
                    "git",
                    "-C",
                    "/checkout",
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    "feature^{commit}",
                ],
                ["git", "-C", "/checkout", "fetch", "origin", "dev"],
                [
                    "git",
                    "-C",
                    "/checkout",
                    "rev-parse",
                    "--verify",
                    "--end-of-options",
                    "FETCH_HEAD^{commit}",
                ],
                ["git", "-C", "/checkout", "merge-base", "--is-ancestor", TIP, TARGET],
                [
                    "git",
                    "-C",
                    "/checkout",
                    "push",
                    "origin",
                    f"{TARGET}:refs/heads/dev",
                ],
            ],
        )

    def test_non_fast_forward_refused_without_force(self):
        self.runner.ancestor = False
        with self.assertRaises(BuildctlError):
            self.app.deploy("feature")
        self.assertFalse(any("push" in call for call in self.commands()))

    def test_force_uses_exact_remote_lease(self):
        self.runner.ancestor = False
        self.app.deploy("feature", force=True)
        push = self.commands()[-1]
        self.assertIn(f"--force-with-lease=refs/heads/dev:{TIP}", push)
        self.assertNotIn("--force", push)
        self.assertIn(f"{TARGET}:refs/heads/dev", push)

    def test_invalid_sha_stops_before_fetch_or_push(self):
        self.runner.target = "not-a-sha"
        with self.assertRaises(BuildctlError):
            self.app.deploy("feature")
        self.assertEqual(len(self.commands()), 1)

    def test_ancestry_error_is_not_treated_as_force_authority(self):
        original = self.runner.run

        def fail_ancestry(args, **kwargs):
            result = original(args, **kwargs)
            if "merge-base" in args:
                result.returncode = 128
            return result

        self.runner.run = fail_ancestry
        with self.assertRaises(BuildctlError):
            self.app.deploy("feature", force=True)
        self.assertFalse(any("push" in call for call in self.commands()))

    def test_mutation_guard_runs_before_commands(self):
        def refuse():
            raise BuildctlError("refused")

        self.environment.require_mutation = refuse
        with self.assertRaises(BuildctlError):
            self.app.deploy("feature")
        self.assertEqual(self.commands(), [])

    def test_status_has_only_public_version_and_allowed_run_metadata(self):
        self.runner.run_list = [
            {
                "databaseId": 123,
                "status": "completed",
                "conclusion": "success",
                "headSha": TARGET,
                "url": "https://github.com/owner/project/actions/runs/123",
                "createdAt": "2026-10-24T00:00:00Z",
                "secret": "private",
            }
        ]
        result = self.app.status()
        self.assertEqual(
            self.urls, ["https://dev.example.test/app/static/version.json"]
        )
        self.assertEqual(result["version"], TARGET)
        self.assertEqual(result["deployment"]["databaseId"], 123)
        self.assertNotIn("private", json.dumps(result))
        args = self.commands()[0]
        self.assertEqual(args[:3], ["gh", "run", "list"])
        self.assertNotIn("--branch", args)
        self.assertIn("dev-deploy.yml", args)
        self.assertEqual(args[-1], "databaseId,status,conclusion,headSha,url,createdAt")
        self.assertEqual(self.guards, [])

    def test_status_handles_no_workflow_runs(self):
        self.assertIsNone(self.app.status()["deployment"])

    def test_status_never_returns_nested_workflow_values(self):
        self.runner.run_list = [{"status": {"secret": "private"}}]
        result = self.app.status()
        self.assertNotIn("private", json.dumps(result))
        self.assertIn("deployment_error", result)

    def test_status_rejects_malformed_public_version(self):
        self.app.fetch = lambda url: {"version": {"private": "hidden"}}
        result = self.app.status()
        self.assertIsNone(result["version"])
        self.assertIn("version_error", result)
        self.assertNotIn("hidden", json.dumps(result))
        self.assertEqual(len(self.commands()), 1)

    def test_status_retains_public_version_when_gh_fails(self):
        def fail(*args, **kwargs):
            raise BuildctlError("secret response body")

        self.runner.run = fail
        result = self.app.status()
        self.assertEqual(result["version"], TARGET)
        self.assertIsNone(result["deployment"])
        self.assertNotIn("secret response body", json.dumps(result))

    @patch("buildctl.app.HTTPSConnection")
    def test_default_fetch_has_timeout_and_bounds_response(self, connection_type):
        response = MagicMock()
        response.status = 200
        response.read.return_value = b'{"version":"dev"}'
        connection = connection_type.return_value
        connection.getresponse.return_value = response
        self.assertEqual(
            default_fetch("https://dev.example.test/version.json"), {"version": "dev"}
        )
        connection_type.assert_called_once()
        self.assertEqual(connection_type.call_args.args, ("dev.example.test",))
        self.assertEqual(connection_type.call_args.kwargs["timeout"], HTTP_TIMEOUT)
        context = connection_type.call_args.kwargs["context"]
        self.assertEqual(context.verify_mode, ssl.CERT_REQUIRED)
        self.assertTrue(context.check_hostname)
        connection.request.assert_called_once_with(
            "GET",
            "/version.json",
            headers={
                "Accept": "application/json",
                "Cache-Control": "no-store",
            },
        )
        response.read.assert_called_once_with(MAX_VERSION_BYTES + 1)
        connection.close.assert_called_once()

    @patch("buildctl.app.HTTPSConnection")
    def test_default_fetch_refuses_oversized_response(self, connection_type):
        connection = connection_type.return_value
        connection.getresponse.return_value.status = 200
        connection.getresponse.return_value.read.return_value = b"x" * (
            MAX_VERSION_BYTES + 1
        )
        with self.assertRaises(BuildctlError):
            default_fetch("https://dev.example.test/version.json")
        connection.close.assert_called_once()

    @patch("buildctl.app.HTTPSConnection")
    def test_default_fetch_diagnostics_do_not_echo_response_errors(
        self, connection_type
    ):
        connection = connection_type.return_value
        connection.request.side_effect = OSError("private response body")
        with self.assertRaises(BuildctlError) as caught:
            default_fetch("https://dev.example.test/version.json")
        self.assertNotIn("private", str(caught.exception))
        connection.close.assert_called_once()

    @patch("buildctl.app.HTTPSConnection")
    def test_default_fetch_rejects_unsafe_urls_before_connecting(self, connection_type):
        for url in (
            "file:///private",
            "http://dev.example.test/version.json",
            "ftp://dev.example.test/version.json",
            "https:///missing-host",
            "https://user:password@dev.example.test/version.json",
            "https://dev.example.test/version.json?secret=private",
            "https://dev.example.test/version.json#private",
        ):
            with self.subTest(url=url), self.assertRaises(BuildctlError):
                default_fetch(url)
        connection_type.assert_not_called()

    @patch("buildctl.app.HTTPSConnection")
    def test_default_fetch_refuses_redirects_and_other_non_success_statuses(
        self, connection_type
    ):
        connection = connection_type.return_value
        response = connection.getresponse.return_value
        for status in (301, 302, 307, 308, 404, 500):
            response.status = status
            with self.subTest(status=status), self.assertRaises(BuildctlError):
                default_fetch("https://dev.example.test/version.json")
        response.read.assert_not_called()
        self.assertEqual(connection.request.call_count, 6)
        self.assertEqual(connection.close.call_count, 6)


if __name__ == "__main__":
    unittest.main()
