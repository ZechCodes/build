"""One checked-in environment registry; resolved paths cannot cross environments."""

from dataclasses import dataclass
from pathlib import Path
import re
import tomllib
from urllib.parse import urlsplit

from .runtime import BuildctlError

CHANNELS = {
    "dev": (".local/state/build-dev", "build-bridge-dev.service"),
    "prod": (".build", "build-bridge.service"),
}


def full_sha(value):
    return isinstance(value, str) and bool(
        re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", value)
    )


def expand(raw, home):
    if raw.startswith("~/"):
        return home / raw[2:]
    path = Path(raw)
    if not path.is_absolute():
        raise BuildctlError("registry paths must be absolute or start with ~/")
    return path


@dataclass(frozen=True)
class Environment:
    name: str
    home: Path
    channel: str
    service_manager: str
    service: str
    state_root: Path
    logs_dir: Path
    binary_dir: Path
    web_url: str
    api_url: str
    relay_url: str
    branch: str
    repository: str
    deploy_workflow: str
    allow_mutation: bool
    minimum_bridge_commit: str
    other_roots: tuple[Path, ...] = ()

    def require_mutation(self):
        if self.channel == "prod" or self.name == "prod" or not self.allow_mutation:
            raise BuildctlError(
                "mutation refused: prod bridge rolls and deploys need Zech’s authorization "
                "and keep their current path; this environment is read-only"
            )

    def path(self, path):
        """Validate each accessed path, including symlinks and missing leaves."""
        resolved = path.resolve()
        root = self.state_root.resolve()
        if resolved != path or not resolved.is_relative_to(root):
            raise BuildctlError(
                "path escapes its environment state root or uses a symlink"
            )
        forbidden = self.other_roots
        if self.channel != "prod":
            forbidden += (self.home / ".build",)
        if any(resolved.is_relative_to(other.resolve()) for other in forbidden):
            raise BuildctlError("path crosses another environment state root")
        return path

    def bridge_env(self, inherited):
        self.require_mutation()
        child = {
            key: value
            for key, value in inherited.items()
            if not key.startswith("BRIDGE_")
        }
        child.update(
            HOME=str(self.home),
            BRIDGE_CHANNEL=self.channel,
            BRIDGE_API_URL=self.api_url,
            BRIDGE_WEB_URL=self.web_url,
            BRIDGE_RELAY_URL=self.relay_url,
        )
        return child


def validate_settings(env):
    if env.channel not in CHANNELS:
        raise BuildctlError(
            "unsupported bridge channel; add bridge support before registering it"
        )
    root, service = CHANNELS[env.channel]
    if env.state_root != env.home / root or env.service != service:
        raise BuildctlError(
            "registry paths/service do not match bridge channel support"
        )
    if env.logs_dir != env.state_root / "log":
        raise BuildctlError("logs_dir must match the bridge channel log path")
    if env.service_manager != "systemd":
        raise BuildctlError("service manager not implemented; Linux/systemd only")
    if not full_sha(env.minimum_bridge_commit):
        raise BuildctlError("invalid minimum bridge commit")
    validate_remote_settings(env)


def validate_remote_settings(env):
    for value, scheme in (
        (env.web_url, "https"),
        (env.api_url, "https"),
        (env.relay_url, "wss"),
    ):
        url = urlsplit(value)
        if (
            url.scheme != scheme
            or not url.hostname
            or url.username
            or url.password
            or url.query
            or url.fragment
        ):
            raise BuildctlError(
                "registry URLs must be public HTTPS/WSS origins without credentials"
            )
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", env.repository):
        raise BuildctlError("invalid repository")
    if (
        not re.fullmatch(r"[\w./-]+", env.branch)
        or env.branch.startswith("-")
        or ".." in env.branch
    ):
        raise BuildctlError("invalid deployment branch")
    if not re.fullmatch(r"[\w.-]+\.ya?ml", env.deploy_workflow):
        raise BuildctlError("invalid deploy workflow")


def load_registry(path: Path, home: Path):
    home = home.resolve()
    try:
        with path.open("rb") as stream:
            raw = tomllib.load(stream)
        roots = {name: expand(entry["state_root"], home) for name, entry in raw.items()}
        return {
            name: make_environment(name, entry, home, roots)
            for name, entry in raw.items()
        }
    except (OSError, KeyError, TypeError, ValueError) as error:
        raise BuildctlError("invalid or unreadable environment registry") from error


def make_environment(name, entry, home, roots):
    fields = dict(entry)
    for key in ("state_root", "logs_dir", "binary_dir"):
        fields[key] = expand(fields[key], home)
    others = tuple(root for key, root in roots.items() if key != name)
    env = Environment(name=name, home=home, other_roots=others, **fields)
    validate_settings(env)
    for path in (env.state_root, env.logs_dir, env.binary_dir):
        env.path(path)
    return env
