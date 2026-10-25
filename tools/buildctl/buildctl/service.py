"""Service lifecycle boundary. A launchd implementation can share this protocol."""

from dataclasses import dataclass
from pathlib import Path
import hashlib
import os
import sys
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
    def prepare_install(self) -> None: ...
    def reload(self) -> None: ...
    def stop(self) -> None: ...
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

    def start(self):
        self.env.require_mutation()
        self.command("start")


def service_manager(env, runner):
    if sys.platform == "linux" and env.service_manager == "systemd":
        return Systemd(env, runner)
    raise BuildctlError(
        "only Linux/systemd is implemented; launchd can implement ServiceManager later"
    )
