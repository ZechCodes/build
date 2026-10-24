"""Deploy the web application through its branch, and inspect public status."""

import json
import re
import ssl
from contextlib import suppress
from http.client import HTTPException, HTTPSConnection
from pathlib import Path
from urllib.parse import urlsplit

from buildctl.runtime import BuildctlError, redact


RUN_FIELDS = ("databaseId", "status", "conclusion", "headSha", "url", "createdAt")
MAX_VERSION_BYTES = 64 * 1024
HTTP_TIMEOUT = 10


def default_fetch(url):
    """Fetch a bounded public JSON document without including bodies in errors."""
    target = _https_target(url)
    connection = None
    try:
        # Python >=3.11; explicit context verifies certificates and hostnames.
        # nosemgrep: python.lang.security.audit.httpsconnection-detected.httpsconnection-detected
        connection = HTTPSConnection(
            target.hostname,
            port=target.port,
            timeout=HTTP_TIMEOUT,
            context=ssl.create_default_context(),
        )
        connection.request(
            "GET",
            target.path or "/",
            headers={
                "Accept": "application/json",
                "Cache-Control": "no-store",
            },
        )
        return _read_version_response(connection.getresponse())
    except (HTTPException, OSError, ValueError) as error:
        raise BuildctlError("Could not fetch public application version") from error
    finally:
        if connection is not None:
            with suppress(OSError):
                connection.close()


def _https_target(url):
    try:
        target = urlsplit(url)
        valid = (
            target.scheme == "https"
            and bool(target.hostname)
            and target.username is None
            and target.password is None
            and not target.query
            and not target.fragment
        )
        if not valid:
            raise ValueError("unsupported public URL")
        return target
    except ValueError as error:
        raise BuildctlError(
            "Public version URL must use HTTPS without credentials, query or fragment"
        ) from error


def _read_version_response(response):
    if response.status != 200:
        raise BuildctlError("Could not fetch public application version")
    body = response.read(MAX_VERSION_BYTES + 1)
    if len(body) > MAX_VERSION_BYTES:
        raise BuildctlError("Public version response exceeds the size limit")
    return json.loads(body)


def _full_sha(value):
    sha = value.strip()
    if not re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", sha):
        raise BuildctlError("Git did not resolve a full commit SHA")
    return sha


def _public_version(document):
    version = document.get("version") if isinstance(document, dict) else None
    if not isinstance(version, str) or not re.fullmatch(
        r"[A-Za-z0-9][A-Za-z0-9._+-]{0,127}", version
    ):
        raise BuildctlError("Public application version is missing or invalid")
    return version


def _latest_run(output):
    try:
        runs = json.loads(output)
    except ValueError as error:
        raise BuildctlError("Deployment workflow response is invalid") from error
    if not isinstance(runs, list):
        raise BuildctlError("Deployment workflow response is invalid")
    if not runs:
        return None
    if not isinstance(runs[0], dict):
        raise BuildctlError("Deployment workflow response is invalid")
    return {
        key: _public_run_value(key, runs[0][key])
        for key in RUN_FIELDS
        if key in runs[0]
    }


def _public_run_value(key, value):
    if key == "databaseId" and type(value) is int:
        return value
    if key == "conclusion" and value is None:
        return None
    if key != "databaseId" and isinstance(value, str):
        return redact(value)
    raise BuildctlError("Deployment workflow response is invalid")


class App:
    """Application deployment uses ordinary pushes or an explicit exact lease."""

    def __init__(self, env, runner, repo: Path, fetch=default_fetch):
        self.env = env
        self.runner = runner
        self.repo = repo
        self.fetch = fetch

    def _git(self, *args, check=True):
        return self.runner.run(["git", "-C", str(self.repo), *args], check=check)

    def _resolve(self, ref):
        result = self._git(
            "rev-parse", "--verify", "--end-of-options", f"{ref}^{{commit}}"
        )
        return _full_sha(result.stdout)

    def deploy(self, ref, force=False):
        self.env.require_mutation()
        target = self._resolve(ref)
        self._git("fetch", "origin", self.env.branch)
        remote_tip = self._resolve("FETCH_HEAD")
        ancestry = self._git(
            "merge-base", "--is-ancestor", remote_tip, target, check=False
        )
        if ancestry.returncode not in (0, 1):
            raise BuildctlError("Could not check deployment branch ancestry")
        if ancestry.returncode and not force:
            raise BuildctlError(
                "Deployment is not a fast-forward; use --force for an explicit lease"
            )
        args = ["push", "origin"]
        if force:
            args.append(f"--force-with-lease=refs/heads/{self.env.branch}:{remote_tip}")
        self._git(*args, f"{target}:refs/heads/{self.env.branch}")
        return {
            "environment": self.env.name,
            "branch": self.env.branch,
            "commit": target,
            "forced": force,
        }

    def status(self):
        status = {"environment": self.env.name, "version": None, "deployment": None}
        try:
            document = self.fetch(
                f"{self.env.web_url.rstrip('/')}/app/static/version.json"
            )
            status["version"] = redact(_public_version(document))
        except (BuildctlError, OSError, ValueError):
            status["version_error"] = "Could not read public application version"
        try:
            status["deployment"] = self._deployment_status()
        except (BuildctlError, OSError, ValueError):
            status["deployment_error"] = "Could not read deployment workflow status"
        return status

    def _deployment_status(self):
        result = self.runner.run(
            [
                "gh",
                "run",
                "list",
                "--repo",
                self.env.repository,
                "--workflow",
                self.env.deploy_workflow,
                "--limit",
                "1",
                "--json",
                ",".join(RUN_FIELDS),
            ]
        )
        return _latest_run(result.stdout)
