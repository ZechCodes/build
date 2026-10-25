"""Verified bridge deployment, with all managed data inside an environment root."""

from collections import deque
from contextlib import contextmanager
import fcntl
import json
import os
from pathlib import Path
import shutil
import tempfile
import time

from .migrate import migration_lock
from .registry import clean_environment, full_sha
from .runtime import BuildctlError, LogRedactor, digest, redact
from .service import service_manager


@contextmanager
def management_lock(env):
    env.require_mutation()
    root = env.path(env.state_root)
    with migration_lock(root):
        with environment_lock(env, root):
            yield


@contextmanager
def environment_lock(env, root):
    root.mkdir(parents=True, exist_ok=True, mode=0o700)
    path = env.path(root / "buildctl.lock")
    descriptor = os.open(path, os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    try:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise BuildctlError(
                "another buildctl bridge operation is running"
            ) from error
        yield
    finally:
        os.close(descriptor)


def atomic_write(env, path, text):
    env.path(path)
    with tempfile.NamedTemporaryFile(
        mode="w", dir=env.state_root, prefix=".buildctl-", delete=False
    ) as stream:
        temporary = Path(stream.name)
        try:
            stream.write(text)
            stream.flush()
            os.fsync(stream.fileno())
            os.replace(temporary, path)
        finally:
            temporary.unlink(missing_ok=True)


class LogCursor:
    def __init__(self, env):
        self.env = env
        self.positions = {path: self.position(path) for path in self.files()}

    def files(self):
        return [
            self.env.path(self.env.logs_dir / name)
            for name in ("bridge.log", "bridge.err.log")
        ]

    @staticmethod
    def position(path):
        try:
            stat = path.stat()
            return stat.st_ino, stat.st_size
        except FileNotFoundError:
            return 0, 0

    def authenticated(self):
        return any(self.new_auth(path) for path in self.files())

    def new_auth(self, path):
        inode, size = self.position(path)
        old_inode, offset = self.positions[path]
        if inode == 0:
            return False
        if inode != old_inode or size < offset:
            offset = 0
        with path.open("rb") as stream:
            stream.seek(offset)
            return b"relay: authenticated" in stream.read()


class Bridge:
    def __init__(
        self, env, runner, repo, *, service=None, verify_timeout=60, poll_interval=1
    ):
        self.env, self.runner, self.repo = env, runner, repo
        self.service = service or service_manager(env, runner)
        self.verify_timeout, self.poll_interval = verify_timeout, poll_interval

    def records(self):
        path = self.env.path(self.env.state_root / "buildctl.json")
        if not path.exists():
            return {"current": None, "previous": None}
        try:
            data = json.loads(path.read_text())
            record = {key: data.get(key) for key in ("current", "previous")}
        except (ValueError, AttributeError) as error:
            raise BuildctlError("invalid buildctl record") from error
        if any(value is not None and not full_sha(value) for value in record.values()):
            raise BuildctlError("invalid commit in buildctl record")
        return record

    def binary(self, sha):
        if not full_sha(sha):
            raise BuildctlError("invalid staged commit")
        path = self.env.path(self.env.binary_dir / sha / "build-bridge")
        if not path.is_file() or not os.access(path, os.X_OK):
            raise BuildctlError("no staged binary; deploy a build first")
        return path

    def deploy(self, ref=None):
        with management_lock(self.env):
            self.records()  # Refuse corrupt records before building or changing the unit.
            sha = self.resolve(ref or f"origin/{self.env.branch}")
            self.build(sha)
            atomic_write(
                self.env, self.env.state_root / "buildctl-candidate", sha + "\n"
            )
            return self.activate(sha)

    def resolve(self, ref):
        result = self.runner.run(
            [
                "git",
                "-C",
                str(self.repo),
                "rev-parse",
                "--verify",
                "--end-of-options",
                ref + "^{commit}",
            ]
        )
        sha = result.stdout.strip()
        if not full_sha(sha):
            raise BuildctlError("ref did not resolve to a full commit")
        result = self.runner.run(
            [
                "git",
                "-C",
                str(self.repo),
                "merge-base",
                "--is-ancestor",
                self.env.minimum_bridge_commit,
                sha,
            ],
            check=False,
        )
        if result.returncode:
            raise BuildctlError(
                "ref predates required dev channel isolation; use a newer commit"
            )
        return sha

    def build(self, sha):
        self.env.require_mutation()
        destination = self.env.path(self.env.binary_dir / sha / "build-bridge")
        if destination.exists():
            self.binary(sha)
            return
        with tempfile.TemporaryDirectory(
            prefix=".buildctl-build-", dir=self.env.state_root
        ) as temporary:
            root = self.env.path(Path(temporary))
            source = root / "source"
            try:
                self.runner.run(
                    [
                        "git",
                        "-C",
                        str(self.repo),
                        "worktree",
                        "add",
                        "--detach",
                        str(source),
                        sha,
                    ]
                )
                child_env = clean_environment(os.environ)
                child_env["CARGO_TARGET_DIR"] = str(root / "target")
                self.runner.run(
                    [
                        "nice",
                        "-n",
                        "10",
                        "cargo",
                        "build",
                        "--release",
                        "--locked",
                        "--bin",
                        "build-bridge",
                    ],
                    cwd=source / "bridge",
                    env=child_env,
                    diagnostics=True,
                )
                self.stage(root / "target/release/build-bridge", destination)
            finally:
                self.runner.run(
                    [
                        "git",
                        "-C",
                        str(self.repo),
                        "worktree",
                        "remove",
                        "--force",
                        str(source),
                    ],
                    check=False,
                )

    def stage(self, source, destination):
        self.env.require_mutation()
        self.env.path(destination)
        destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        with tempfile.NamedTemporaryFile(
            dir=destination.parent, prefix=".stage-", delete=False
        ) as stream:
            temporary = Path(stream.name)
        try:
            shutil.copyfile(source, temporary)
            temporary.chmod(0o700)
            os.replace(temporary, destination)
        finally:
            temporary.unlink(missing_ok=True)

    def activate(self, sha):
        self.env.require_mutation()
        record = self.records()
        binary = self.binary(sha)
        expected = digest(binary)
        self.service.prepare_install()
        child_env = self.env.bridge_env(os.environ)
        result = self.runner.run(
            [str(binary), "install-service"], env=child_env, check=False
        )
        if result.returncode:
            raise BuildctlError(
                "install failed; if unpaired, run bridge pair, approve on dev, then bridge deploy again"
            )
        self.service.reload()
        self.service.stop()
        cursor = LogCursor(self.env)  # The old process is stopped before the baseline.
        self.service.start()
        self.verify(expected, cursor)
        if sha != record["current"]:
            record = {"current": sha, "previous": record["current"]}
        atomic_write(
            self.env, self.env.state_root / "buildctl.json", json.dumps(record) + "\n"
        )
        return {**record, "environment": self.env.name, "relay": "authenticated"}

    def verify(self, expected, cursor):
        deadline = time.monotonic() + self.verify_timeout
        while True:
            process = self.service.process()
            if (
                self.service.state() == "active"
                and process
                and process.sha256 == expected
                and cursor.authenticated()
            ):
                return
            if time.monotonic() >= deadline:
                raise BuildctlError(
                    "verification failed; records unchanged; inspect bridge status/logs and redeploy or rollback"
                )
            time.sleep(self.poll_interval)

    def rollback(self):
        with management_lock(self.env):
            previous = self.records()["previous"]
            if not previous:
                raise BuildctlError(
                    "no previous verified build; redeploy the recorded current ref to recover"
                )
            atomic_write(
                self.env, self.env.state_root / "buildctl-candidate", previous + "\n"
            )
            return self.activate(previous)

    def pair(self):
        with management_lock(self.env):
            candidate = self.env.path(self.env.state_root / "buildctl-candidate")
            sha = (
                candidate.read_text().strip()
                if candidate.exists()
                else self.records()["current"]
            )
            return self.runner.pair(
                self.binary(sha),
                env=self.env.bridge_env(os.environ),
                approve_url=self.env.web_url.rstrip("/") + "/app/#/pair",
            )

    def status(self):
        record = self.records()
        process = self.service.process()
        running = self.running_status(process)
        identity = self.env.path(self.env.state_root / "identity.json")
        return {
            "environment": self.env.name,
            "unit": self.env.service,
            "state": self.service.state(),
            **record,
            "running": running,
            "paired": identity.is_file(),
            "relay": self.relay_lines(),
        }

    def running_status(self, process):
        if not process:
            return None
        result = {
            "pid": process.pid,
            "executable": process.executable,
            "sha256": process.sha256,
            "managed": False,
            "commit": None,
        }
        path = Path(process.executable)
        if path.parent.parent == self.env.binary_dir and full_sha(path.parent.name):
            binary = self.binary(path.parent.name)
            if digest(binary) == process.sha256:
                result.update(managed=True, commit=path.parent.name)
        return result

    def relay_lines(self):
        result = []
        for name in ("bridge.log", "bridge.err.log"):
            path = self.env.path(self.env.logs_dir / name)
            if path.exists():
                with path.open(errors="replace") as stream:
                    result.extend(
                        redact(line.rstrip())
                        for line in deque(stream, maxlen=100)
                        if "relay:" in line
                    )
        return result[-10:]

    @staticmethod
    def safe_log_lines(stream):
        redactor = LogRedactor()
        for line in stream:
            safe = redactor.line(line)
            if safe is not None:
                yield safe

    def log_lines(self):
        for name in ("bridge.log", "bridge.err.log"):
            path = self.env.path(self.env.logs_dir / name)
            if path.exists():
                with path.open(errors="replace") as stream:
                    yield from deque(self.safe_log_lines(stream), maxlen=50)

    def logs(self, follow=False):
        for line in self.log_lines():
            print(line, flush=True)
        if follow:
            self.follow_logs()

    def follow_logs(self):
        cursor = LogCursor(self.env)
        redactors = {}
        for path in cursor.files():
            redactors[path] = LogRedactor()
            if path.exists():
                with path.open(errors="replace") as stream:
                    for line in stream:
                        redactors[path].line(line)
                    cursor.positions[path] = (path.stat().st_ino, stream.tell())
        while True:
            time.sleep(0.5)
            for path in cursor.files():
                self.print_appended(path, cursor, redactors)

    @staticmethod
    def print_appended(path, cursor, redactors):
        inode, size = cursor.position(path)
        previous_inode, offset = cursor.positions[path]
        if not inode:
            return
        if inode != previous_inode or size < offset:
            offset = 0
            redactors[path] = LogRedactor()
        with path.open(errors="replace") as stream:
            stream.seek(offset)
            for line in stream:
                safe = redactors[path].line(line)
                if safe is not None:
                    print(safe, flush=True)
            cursor.positions[path] = (inode, stream.tell())

    def prune(self, keep=3):
        if keep < 0:
            raise BuildctlError("keep count must be non-negative")
        with management_lock(self.env):
            directory = self.env.path(self.env.binary_dir)
            if not directory.exists():
                return {"removed": []}
            record = self.records()
            paths = [
                self.env.path(path)
                for path in directory.iterdir()
                if full_sha(path.name)
            ]
            paths.sort(key=lambda path: path.stat().st_mtime, reverse=True)
            protected = {record["current"], record["previous"]}
            process = self.service.process()
            if process:
                protected.add(Path(process.executable).parent.name)
            removed = []
            for path in paths[keep:]:
                if path.name not in protected:
                    shutil.rmtree(path)
                    removed.append(path.name)
            return {"removed": removed}
