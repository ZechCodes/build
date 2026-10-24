"""Build diagnostics use synthetic subprocess results, never a real compiler."""

import subprocess
from unittest.mock import Mock

import pytest

from buildctl.runtime import BuildctlError, Runner

CARGO = [
    "nice",
    "-n",
    "10",
    "cargo",
    "build",
    "--release",
    "--locked",
    "--bin",
    "build-bridge",
]


def test_failed_build_shows_only_last_40_redacted_stderr_lines(monkeypatch):
    stderr = "\n".join(f"compiler message {index}" for index in range(60))
    stderr += "\ntoken=synthetic-diagnostic-secret\nerror: unresolved import\n"
    result = subprocess.CompletedProcess(CARGO, 101, "stdout must stay hidden", stderr)
    monkeypatch.setattr("buildctl.runtime.subprocess.run", Mock(return_value=result))
    with pytest.raises(BuildctlError) as caught:
        Runner().run(CARGO, diagnostics=True)
    output = str(caught.value)
    assert "error: unresolved import" in output
    assert "compiler message 0\n" not in output
    assert "compiler message 21\n" not in output
    assert "compiler message 22\n" in output
    assert "stdout must stay hidden" not in output
    assert "synthetic-diagnostic-secret" not in output


def test_successful_build_diagnostics_stay_quiet(monkeypatch, capsys):
    result = subprocess.CompletedProcess(CARGO, 0, "build stdout", "build warning")
    monkeypatch.setattr("buildctl.runtime.subprocess.run", Mock(return_value=result))
    assert Runner().run(CARGO, diagnostics=True).returncode == 0
    assert capsys.readouterr() == ("", "")


def test_other_command_failure_still_suppresses_diagnostics(monkeypatch):
    result = subprocess.CompletedProcess(["git"], 1, "", "private response")
    monkeypatch.setattr("buildctl.runtime.subprocess.run", Mock(return_value=result))
    with pytest.raises(BuildctlError) as caught:
        Runner().run(["git"])
    assert "private response" not in str(caught.value)


def test_oversized_diagnostic_line_is_omitted_before_redaction(monkeypatch):
    from buildctl.runtime import LogRedactor, diagnostic_tail

    original = LogRedactor.line

    def bounded_line(self, text):
        assert len(text) <= 4096
        return original(self, text)

    monkeypatch.setattr(LogRedactor, "line", bounded_line)
    output = diagnostic_tail("x" * 100000 + "\nerror: useful diagnostic\n")
    assert "oversized" in output
    assert "error: useful diagnostic" in output
    assert len(output) < 4096


def test_diagnostics_hide_secret_value_on_following_line():
    from buildctl.runtime import diagnostic_tail

    output = diagnostic_tail(
        "identity_private_key_b64:\nsynthetic-next-line-value\nerror: broken build\n"
    )
    assert "synthetic-next-line-value" not in output
    assert "error: broken build" in output
