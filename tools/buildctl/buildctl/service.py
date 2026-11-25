"""Service lifecycle boundary. A launchd implementation can share this protocol."""

from dataclasses import dataclass
from pathlib import Path
import hashlib
import os
import re
import sys
from tempfile import NamedTemporaryFile
from typing import Protocol

from .runtime import BuildctlError


@dataclass(frozen=True)
class Process:
    pid: int
    executable: str
    sha256: str


class ServiceManager(Protocol):
    def state(self) -> str: ...
    def process(self) -> Process | None: ...
    def invocation(self) -> str | None: ...
    def prepare_install(self) -> None: ...
    def reload(self) -> None: ...
    def stop(self) -> None: ...
    def stop_for_restart(self) -> None: ...
    def start(self) -> None: ...


class Systemd:
    def __init__(self, env, runner, *, proc_root=Path("/proc")):
        self.env = env
        self.runner = runner
        self.proc_root = proc_root

    def command(self, *args, check=True):
        return self.runner.run(
            ["systemctl", "--user", *args, self.env.service], check=check
        )

    def state(self):
        return self.command("is-active", check=False).stdout.strip() or "unknown"

    def process(self):
        value = self.command(
            "show", "--property=MainPID", "--value", check=False
        ).stdout.strip()
        if not value.isdigit() or int(value) == 0:
            return None
        path = self.proc_root / value / "exe"
        try:
            with path.open("rb") as stream:
                executable = os.readlink(f"/proc/self/fd/{stream.fileno()}")
                sha = hashlib.file_digest(stream, "sha256").hexdigest()
                return Process(int(value), executable, sha)
        except OSError:
            return None

    def prepare_install(self):
        self.env.require_mutation()
        unit = self.env.home / ".config/systemd/user" / self.env.service
        if unit.resolve() != unit:
            raise BuildctlError(
                "systemd unit path uses a symlink; refusing installation"
            )

    def reload(self):
        self.env.require_mutation()
        self.runner.run(["systemctl", "--user", "daemon-reload"])

    def stop(self):
        self.env.require_mutation()
        self.command("stop")

    def invocation(self):
        value = self.command(
            "show", "--property=InvocationID", "--value", check=False
        ).stdout.strip()
        if re.fullmatch(r"[0-9a-f]{32}", value) and value != "0" * 32:
            return value
        return None

    def stop_for_restart(self):
        self.env.require_mutation()
        invocation = self.invocation()
        if invocation is None:
            self.stop()
            return
        directory = self.env.path(self.env.state_root / "tasks")
        marker = self.env.path(directory / "http-endpoint.restart")
        directory.mkdir(exist_ok=True, mode=0o700)
        with NamedTemporaryFile(mode="w", dir=directory, prefix=".restart-", delete=False) as stream:
            temporary = Path(stream.name)
        try:
            temporary.write_text(invocation + "\n")
            os.replace(temporary, marker)
            self.stop()
        finally:
            temporary.unlink(missing_ok=True)
            marker.unlink(missing_ok=True)

    def start(self):
        self.env.require_mutation()
        self.command("start")


def service_manager(env, runner):
    if sys.platform == "linux" and env.service_manager == "systemd":
        return Systemd(env, runner)
    raise BuildctlError(
        "only Linux/systemd is implemented; launchd can implement ServiceManager later"
    )
