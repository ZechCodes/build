"""Validate host-local HTTPS settings without opening certificate or key files."""

import ipaddress
import os
from pathlib import Path
import re
import stat
import tomllib
from urllib.parse import urlsplit

from .runtime import BuildctlError

REQUIRED = {"bind", "origin", "tls_cert", "tls_key"}


def load_http_settings(env):
    path = env.path(env.state_root / "http.toml")
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        return {}
    except OSError as error:
        raise BuildctlError("cannot open HTTP configuration") from error
    try:
        with os.fdopen(descriptor, "rb") as stream:
            require_private_file(os.fstat(stream.fileno()))
            raw = tomllib.load(stream)
        return validate_http_settings(env, raw)
    except (OSError, ValueError, TypeError) as error:
        raise BuildctlError("invalid HTTP configuration") from error


def require_private_file(metadata):
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_IMODE(metadata.st_mode) != 0o600
        or metadata.st_uid != os.getuid()
    ):
        raise BuildctlError(
            "HTTP configuration and TLS key must be owner-owned mode 0600 files"
        )


def validate_http_settings(env, raw):
    if not REQUIRED <= raw.keys() or raw.keys() - (REQUIRED | {"allowed_origins"}):
        raise BuildctlError("HTTP configuration has missing or unknown keys")
    validate_bind(raw["bind"])
    validate_origin(raw["origin"])
    result = {
        "BRIDGE_HTTP_ENABLED": "true",
        "BRIDGE_HTTP_BIND": raw["bind"],
        "BRIDGE_HTTP_ORIGIN": raw["origin"],
    }
    for name in ("tls_cert", "tls_key"):
        path = tls_path(env, raw[name])
        metadata = path.stat()
        if not stat.S_ISREG(metadata.st_mode):
            raise BuildctlError("HTTP TLS paths must be regular files")
        if name == "tls_key":
            require_private_file(metadata)
        result[f"BRIDGE_HTTP_{name.upper()}"] = str(path)
    if "allowed_origins" in raw:
        result["BRIDGE_HTTP_ALLOWED_ORIGINS"] = validate_allowlist(
            raw["allowed_origins"]
        )
    return result


def tls_path(env, value):
    if not isinstance(value, str) or not value or any(ord(char) < 32 for char in value):
        raise BuildctlError(
            "HTTP TLS paths must be absolute paths inside the state root"
        )
    path = Path(value)
    if not path.is_absolute():
        raise BuildctlError(
            "HTTP TLS paths must be absolute paths inside the state root"
        )
    return env.path(path)


def validate_bind(value):
    if not isinstance(value, str):
        raise BuildctlError("HTTP bind must be an explicit IP:port with a nonzero port")
    match = re.fullmatch(r"(?:\[([0-9a-fA-F:]+)\]|([0-9.]+)):([0-9]+)", value)
    if not match or not 1 <= int(match[3]) <= 65535:
        raise BuildctlError("HTTP bind must be an explicit IP:port with a nonzero port")
    address = ipaddress.ip_address(match[1] or match[2])
    if bool(match[1]) != (address.version == 6):
        raise BuildctlError("HTTP IPv6 bind addresses must use brackets")


def validate_origin(value):
    if not isinstance(value, str) or any(char in value for char in "*\\%,"):
        raise BuildctlError("HTTP origins must be exact HTTPS origins without a path")
    if any(char.isspace() or ord(char) < 32 or ord(char) == 127 for char in value):
        raise BuildctlError("HTTP origins must be exact HTTPS origins without a path")
    url = urlsplit(value)
    if (
        url.scheme != "https"
        or not url.hostname
        or url.username is not None
        or url.password is not None
        or url.path
        or "?" in value
        or "#" in value
        or url.port == 0
        or url.netloc.endswith(":")
    ):
        raise BuildctlError(
            "HTTP origins must be exact HTTPS origins without a path or port 0"
        )


def validate_allowlist(values):
    if not isinstance(values, list) or not 1 <= len(values) <= 16:
        raise BuildctlError(
            "HTTP allowed_origins must contain 1..16 exact HTTPS origins"
        )
    for value in values:
        validate_origin(value)
    return ",".join(values)
