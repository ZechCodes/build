#!/usr/bin/env python3
"""Opt-in real Pi TUI qualification; run under nice -n 10, never against a daemon."""
import argparse
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import select
import shutil
import signal
import struct
import subprocess
import tempfile
import termios
import time
import uuid

PI_SHA256 = "9b939197c9ca98182981fc448ac6d441076d3c37360cf8dc649193e664fcf6f7"
SCENARIOS = ("manual", "overflow", "reload", "slow_mcp", "mcp_failure", "sink_failure", "read_loss")
REPO = Path(__file__).resolve().parents[1]


def records(path):
    if not path.exists():
        return []
    data = path.read_bytes()
    return [json.loads(line) for line in data.split(b"\n")[:-1] if line]


def count(events, kind):
    return sum(event["type"] == kind for event in events)


class Probe:
    def __init__(self, root, scenario, binary, node, observer):
        self.home = root / scenario
        self.home.mkdir(mode=0o700)
        self.scenario = scenario
        self.events = self.home / "events.jsonl"
        self.telemetry = self.home / "telemetry.jsonl"
        self.telemetry.touch(mode=0o600)
        self.owner = str(uuid.uuid4())
        self.report = self.home / "observer-report.json"
        self.observer_stop = self.home / "observer-stop"
        self.observer_output = (self.home / "observer-output.log").open("wb")
        observer_env = {"HOME": str(self.home), "PATH": "/usr/bin:/bin",
                        "PI_QUAL_LOG": str(self.telemetry), "PI_QUAL_OWNER": self.owner,
                        "PI_QUAL_THREAD_ID": "thread-qualification", "PI_QUAL_LAUNCH_ID": "launch-qualification",
                        "PI_QUAL_REPORT": str(self.report), "PI_QUAL_STOP": str(self.observer_stop)}
        self.observer = subprocess.Popen([str(observer), "--ignored", "--exact",
            "harness::pi_telemetry::tests::pi_qualification_observer", "--nocapture"],
            cwd=self.home, env=observer_env, stdout=self.observer_output, stderr=subprocess.STDOUT)
        agent = self.home / "agent"
        agent.mkdir()
        (agent / "settings.json").write_text(json.dumps({
            "compaction": {"enabled": True, "keepRecentTokens": 10, "reserveTokens": 16384},
            "retry": {"enabled": False},
        }))
        launcher = self.home / "fake-mcp"
        launcher.write_text("#!/usr/bin/python3\nimport os, sys, time\n"
                            "open(os.environ['FAKE_MCP_PID'], 'w').write(str(os.getpid()))\n"
                            f"time.sleep({11.5 if scenario == 'slow_mcp' else 0})\n"
                            f"os.execv({str(node)!r}, [{str(node)!r}, "
                            f"{str(root / 'pi-mcp-child.mjs')!r}, *sys.argv[1:]])\n")
        launcher.chmod(0o700)
        self.master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))
        # Only fixed fake endpoint values enter the isolated child's environment.
        # The socket path is unused: the owned fixture serves MCP over stdio.
        env = {
            "HOME": str(self.home), "PATH": f"{node.parent}:/usr/bin:/bin", "TERM": "xterm-256color",
            "PI_OFFLINE": "1", "PI_CODING_AGENT_DIR": str(agent),
            "QUAL_HOME": str(self.home), "QUAL_EVENTS": str(self.events), "QUAL_SCENARIO": scenario,
            "BUILD_PI_MCP_COMMAND": str(launcher), "BUILD_PI_MCP_OWNER": self.owner,
            "BUILD_PI_TELEMETRY_LOG": str(self.telemetry),
            "BUILD_PI_COMPACTION_POLICY": json.dumps({"owner": self.owner, "thread_id": "thread-qualification",
                                                     "launch_id": "launch-qualification", "tokens": 150000, "enabled": True}),
            "BRIDGE_MCP_SOCKET": str(self.home / "unused-fake-mcp.sock"),
            "BRIDGE_MCP_TOKEN": "qualification-dummy-token",
            "FAKE_MCP_PID": str(self.home / "fake-mcp.pid"),
            "FAKE_MCP_LOG": str(self.home / "fake-mcp.jsonl"),
            "FAKE_MCP_MODE": "child_exit" if scenario == "mcp_failure" else "normal",
        }
        args = [str(binary), "--approve", "--no-session", "--session-id", self.owner, "--no-extensions",
                "--extension", str(root / "qualification.ts"), "--no-skills", "--no-prompt-templates",
                "--no-themes", "--tools", "done,post_thread_message", "--provider", "qualification", "--model", "local"]
        self.child = subprocess.Popen(args, stdin=slave, stdout=slave, stderr=slave, cwd=self.home,
                                      env=env, start_new_session=True)
        os.close(slave)
        self.output = b""
        self.paste_ready = False
        self.mcp_handles = {}
        self.started = time.monotonic()
        self.evidence = {"scenario": scenario, "fake_endpoint": str(self.home / "unused-fake-mcp.sock")}

    def pump(self, delay=0.02):
        self.capture_mcp()
        if not select.select([self.master], [], [], delay)[0]:
            return
        try:
            chunk = os.read(self.master, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                return
            raise
        self.output = (self.output + chunk)[-24000:]
        self.paste_ready |= b"\x1b[?2004h" in self.output
        for query, response in ((b"\x1b[6n", b"\x1b[1;1R"), (b"\x1b[?u", b"\x1b[?0u"),
                                (b"\x1b[16t", b"\x1b[6;20;10t")):
            if query in chunk:
                os.write(self.master, response)

    def capture_mcp(self):
        try:
            pid = int((self.home / "fake-mcp.pid").read_text())
            if pid in self.mcp_handles:
                return
            stat = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
            assert int(stat[1]) == self.child.pid, "fixture PID must belong to the owned Pi child"
            self.mcp_handles[pid] = os.pidfd_open(pid)
        except (FileNotFoundError, ProcessLookupError, ValueError):
            pass

    def telemetry_diagnostics(self):
        try:
            return records(self.telemetry)[-12:]
        except PermissionError:
            return "unreadable"

    def observed(self):
        return json.loads(self.report.read_text()) if self.report.exists() else {}

    def wait(self, predicate, label, timeout=25):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            self.pump()
            assert self.observer.poll() is None, ("Rust observer exited early: " +
                (self.home / "observer-output.log").read_text())
            assert self.child.poll() is None, f"{self.scenario}: Pi exited {self.child.returncode} during {label}"
            if predicate():
                return
        raise AssertionError(f"{self.scenario}: timed out during {label}; events={records(self.events)[-12:]}; "
                             f"observer={self.observed()}; telemetry={self.telemetry_diagnostics()}; "
                             f"terminal={self.output[-2000:].decode(errors='replace')}")

    def pause(self, duration):
        deadline = time.monotonic() + duration
        while time.monotonic() < deadline:
            self.pump()

    def send(self, text):
        os.write(self.master, b"\x1b[200~" + text.encode() + b"\x1b[201~")
        self.pause(0.75)
        os.write(self.master, b"\r")

    def prompt(self, text):
        settled = count(records(self.events), "agent_settled")
        self.send(text)
        self.wait(lambda: count(records(self.events), "agent_settled") > settled, "prompt settlement")

    def start(self):
        if self.scenario == "slow_mcp":
            self.pause(10.5)
            assert self.child.poll() is None
            assert not any(event["type"] == "ready" for event in records(self.telemetry))
            assert count(records(self.telemetry), "heartbeat") >= 5
            self.evidence["alive_before_ready_after_seconds"] = round(time.monotonic() - self.started, 3)
        self.wait(lambda: any(event["type"] == "ready" for event in records(self.telemetry)), "extension ready")
        self.wait(lambda: self.observed().get("enforcement") == "BetweenTurn", "Rust observer readiness")
        self.wait(lambda: self.paste_ready, "TUI paste mode")
        self.pause(0.6)
        self.prompt("Synthetic first context. " * 20)
        self.prompt("Synthetic second context. " * 20)

    def manual(self):
        before = count(records(self.events), "session_compact")
        self.send("/compact")
        self.wait(lambda: count(records(self.events), "session_compact") > before, "native manual compact")
        self.pause(0.1)
        actual = [event for event in records(self.events) if event["type"] == "session_compact"][-1]
        assert actual["reason"] == "manual"
        assert actual["usage"]["tokens"] is None
        completed = [event for event in records(self.telemetry) if event["type"] == "compaction" and event.get("completed")]
        assert completed[-1]["reason"] == "manual"
        assert any(event["type"] == "settled" for event in records(self.telemetry)[records(self.telemetry).index(completed[-1]):])

    def reload(self):
        prior = [event for event in records(self.telemetry) if event["type"] == "ready"][-1]
        starts = count(records(self.events), "session_start")
        self.send("/reload")
        self.wait(lambda: count(records(self.events), "session_start") > starts, "extension reload")
        self.pause(0.6)
        self.prompt("Synthetic prompt after reload.")
        ready = [event for event in records(self.telemetry) if event["type"] == "ready"][-1]
        assert ready["generation"] > prior["generation"]
        assert any(event["type"] == "settled" and event["generation"] == ready["generation"] for event in records(self.telemetry))

    def transport_failure(self):
        (self.home / "call-tool").touch()
        self.send("Synthetic work triggers actual MCP child exit.")
        deadline = time.monotonic() + 10
        while self.child.poll() is None and time.monotonic() < deadline:
            self.pump()
        assert self.child.poll() is not None, "Pi must terminate on MCP transport failure"
        assert any(event["type"] == "signal" and event["signal"] == "SIGTERM"
                   for event in records(self.events)), "MCP failure must deliver SIGTERM"
        assert any(event["type"] == "unsupported" for event in records(self.telemetry))
        deadline = time.monotonic() + 2
        while self.observed().get("enforcement") != "Unsupported" and time.monotonic() < deadline:
            self.pump()
        assert self.observed().get("enforcement") == "Unsupported"
        self.evidence["transport_failure_exit"] = self.child.returncode

    def telemetry_failure(self):
        before = count(records(self.events), "session_compact")
        original_size = self.telemetry.stat().st_size
        self.telemetry.chmod(0o200 if self.scenario == "read_loss" else 0o644)
        if self.scenario == "read_loss":
            try:
                self.telemetry.read_bytes()
            except PermissionError:
                self.evidence["read_denied"] = True
            else:
                raise AssertionError("write-only sidecar must deny read")
        self.wait(lambda: self.observed().get("enforcement") == "Unsupported", "Rust observes telemetry loss")
        self.pause(1.2)
        if self.scenario == "read_loss":
            assert self.telemetry.stat().st_size > original_size, "JS must still append after read loss"
            self.evidence["js_appended_after_read_loss"] = True
        (self.home / "call-tool").touch()
        self.prompt("Synthetic work after telemetry loss must call the healthy MCP tool.")
        self.wait(lambda: count(records(self.events), "session_compact") > before, "native compaction after loss of fallback")
        assert any(event["type"] == "session_compact" and event["reason"] == "threshold"
                   for event in records(self.events))
        tools = [event for event in records(self.events) if event["type"] == "tool_execution_end"]
        assert any(not event["isError"] and any(item.get("text") == "one" for item in event["result"]["content"])
                   for event in tools), "healthy MCP tool must succeed after telemetry failure"
        self.prompt("Synthetic follow-up proves Pi survived.")
        self.evidence["observer_after_failure"] = self.observed()
        assert self.child.poll() is None
        if self.scenario == "read_loss":
            self.telemetry.chmod(0o600)

    def verify(self):
        events = records(self.events)
        telemetry = records(self.telemetry)
        assert any(event["type"] == "session_compact_failed" and event["reason"] == "threshold"
                   and event["aborted"] for event in events)
        assert all(event["owner"] == self.owner for event in telemetry)
        generation, sequence = 0, 0
        for event in telemetry:
            if event["generation"] != generation:
                assert event["generation"] == generation + 1 and event["type"] == "starting"
                generation, sequence = event["generation"], 0
            assert event["sequence"] == sequence + 1
            sequence = event["sequence"]
        receipt_path = Path(str(self.telemetry) + ".ack")
        assert not receipt_path.is_symlink() and receipt_path.is_file()
        assert receipt_path.stat().st_mode & 0o777 == 0o600
        receipt = json.loads(receipt_path.read_text())
        assert receipt["owner"] == self.owner and receipt["thread_id"] == "thread-qualification"
        assert receipt["launch_id"] == "launch-qualification" and receipt["generation"] == generation
        assert 0 < receipt["sequence"] <= sequence
        self.evidence["observer_receipt"] = receipt
        startup = records(self.home / "fake-mcp.jsonl")[0]["startup"]
        assert startup["socket"] == self.evidence["fake_endpoint"]
        assert startup["token"] == "qualification-dummy-token"
        assert startup["argv"] == ["mcp", "--task", self.owner]
        assert self.telemetry.stat().st_mode & 0o777 == (0o644 if self.scenario == "sink_failure" else 0o600)
        if self.scenario == "overflow":
            boundary = next(index for index, event in enumerate(events)
                            if event["type"] == "session_compact" and event["reason"] == "overflow")
            assert events[boundary]["willRetry"]
            assert any(event["type"] == "agent_start" for event in events[boundary + 1:])
        self.evidence.update(events=events, telemetry=telemetry, observer=self.observed(),
                             pi_alive_after_assertions=self.child.poll() is None)
        return self.evidence

    def close(self):
        self.telemetry.chmod(0o600)
        self.observer_stop.touch()
        try:
            self.observer.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.observer.kill()
            self.observer.wait(timeout=3)
        self.observer_output.close()
        self.evidence["observer_exit"] = self.observer.returncode
        self.capture_mcp()
        if self.child.poll() is None:
            self.child.terminate()
        try:
            self.child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            self.child.kill()
            self.child.wait(timeout=3)
        os.close(self.master)
        # Retained pidfds prevent a recycled PID from targeting another process.
        for fd in self.mcp_handles.values():
            try:
                signal.pidfd_send_signal(fd, signal.SIGTERM)
                if not select.select([fd], [], [], 2)[0]:
                    signal.pidfd_send_signal(fd, signal.SIGKILL)
                assert select.select([fd], [], [], 2)[0], "owned MCP fixture did not exit"
            except ProcessLookupError:
                pass
            finally:
                os.close(fd)
        self.evidence["pi_cleanup_exit"] = self.child.returncode
        self.evidence["owned_mcp_children_stopped"] = len(self.mcp_handles)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--pi", type=Path, required=True)
    parser.add_argument("--node", type=Path, required=True)
    parser.add_argument("--observer", type=Path, required=True)
    parser.add_argument("--extension-source", type=Path, default=REPO / "bridge/src/harness/build-tools.ts")
    parser.add_argument("--scenario", choices=SCENARIOS, action="append")
    args = parser.parse_args()
    assert args.pi.is_absolute() and args.node.is_absolute() and args.observer.is_absolute()
    assert hashlib.sha256(args.pi.read_bytes()).hexdigest() == PI_SHA256, "unqualified Pi executable"
    evidence = []
    with tempfile.TemporaryDirectory(prefix="build-pi-tui-qualification-") as directory:
        root = Path(directory)
        shutil.copyfile(args.extension_source, root / "build-tools.ts")
        extension_hash = hashlib.sha256((root / "build-tools.ts").read_bytes()).hexdigest()
        shutil.copyfile(REPO / "scripts/qualify-pi-compaction-tui.ts", root / "qualification.ts")
        shutil.copyfile(REPO / "bridge/tests/fixtures/pi-mcp-child.mjs", root / "pi-mcp-child.mjs")
        for scenario in args.scenario or SCENARIOS:
            probe = Probe(root, scenario, args.pi, args.node, args.observer)
            try:
                probe.start()
                if scenario == "reload":
                    probe.reload()
                elif scenario == "mcp_failure":
                    probe.transport_failure()
                elif scenario in ("sink_failure", "read_loss"):
                    probe.telemetry_failure()
                elif scenario != "overflow":
                    probe.manual()
                evidence.append(probe.verify())
            finally:
                probe.close()
            assert probe.observer.returncode == 0, "Rust observer gate failed"
    print(json.dumps({"carrier": "tui", "pi": str(args.pi), "sha256": PI_SHA256,
                      "node": str(args.node), "observer": str(args.observer),
                      "extension_sha256": extension_hash, "evidence": evidence}, indent=2))


if __name__ == "__main__":
    main()
