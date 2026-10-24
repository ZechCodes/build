"""Process execution and the public-output boundary."""

import hashlib
from collections import deque
import os
from pathlib import Path
import re
import subprocess


class BuildctlError(Exception):
    """An operational error whose message is safe to show."""


def redact(text: str) -> str:
    """Do not echo inherited credentials, bearer tokens or structured secrets."""
    for name, value in os.environ.items():
        sensitive = re.search(r"TOKEN|SECRET|PASSWORD|PRIVATE_KEY|IDENTITY", name)
        if sensitive and value:
            text = text.replace(value, "[redacted]")
    text = re.sub(r"(?i)Bearer\s+\S+", "Bearer [redacted]", text)
    text = re.sub(
        r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b", "[redacted]", text
    )
    return re.sub(
        r"""(?ix)(["']?(?:[\w-]*(?:token|secret|password|private[_-]?key|authorization|identity)[\w-]*)["']?\s*[:=]\s*)
        ("[^"\n]*"|'[^'\n]*'|[^\s,;}]+)""",
        r"\1[redacted]",
        text,
    )


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class Runner:
    def run(self, args, *, cwd=None, env=None, check=True, diagnostics=False):
        try:
            result = subprocess.run(
                args, cwd=cwd, env=env, text=True, capture_output=True
            )
        except OSError as error:
            raise BuildctlError(f"cannot execute {Path(args[0]).name}") from error
        if check and result.returncode:
            message = f"{Path(args[0]).name} failed (exit {result.returncode})"
            detail = (
                diagnostic_tail(result.stderr)
                if diagnostics
                else "diagnostics suppressed"
            )
            raise BuildctlError(message + "\n" + detail)
        return result

    def pair(self, binary, *, env, approve_url):
        """Stream only the expected public approve URL, never raw bridge output."""
        try:
            child = subprocess.Popen(
                [str(binary), "pair"],
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                bufsize=1,
            )
        except OSError as error:
            raise BuildctlError("cannot start pairing") from error
        link = None
        pattern = re.compile(re.escape(approve_url) + r"/[A-Za-z0-9_-]+")
        try:
            for line in child.stdout:
                match = pattern.search(line)
                if match:
                    link = match.group()
                    print(link, flush=True)
            if child.wait():
                raise BuildctlError(
                    "pairing failed; approve the link on the selected environment"
                )
        finally:
            child.stdout.close()
            if child.poll() is None:
                child.terminate()
                child.wait()
        return link or "pairing complete (identity already paired)"


def public_data(value):
    if isinstance(value, str):
        return redact(value)
    if isinstance(value, dict):
        return {key: public_data(item) for key, item in value.items()}
    if isinstance(value, list):
        return [public_data(item) for item in value]
    return value


class LogRedactor:
    """Suppress multiline private keys as well as inline credential fields."""

    def __init__(self):
        self.private_block = False
        self.pending_value = False

    def line(self, text):
        if re.search(r"-----BEGIN .*PRIVATE KEY-----", text):
            self.private_block = True
            return "[redacted private key]"
        if self.private_block:
            if re.search(r"-----END .*PRIVATE KEY-----", text):
                self.private_block = False
            return None
        if self.pending_value:
            self.pending_value = not bool(text.strip())
            return "[redacted]"
        if re.fullmatch(
            r"(?i)\s*[\w-]*(?:token|secret|password|private[_-]?key|authorization|identity)[\w-]*\s*[:=]\s*",
            text,
        ):
            self.pending_value = True
        return redact(text.rstrip())


def diagnostic_tail(stderr, lines=40):
    redactor = LogRedactor()
    bounded_lines = (
        line if len(line) <= 4096 else "[oversized diagnostic line omitted]"
        for line in stderr.splitlines()
    )
    safe_lines = (redactor.line(line) for line in bounded_lines)
    return "\n".join(
        deque((line for line in safe_lines if line is not None), maxlen=lines)
    )
