"""Termination witnesses for both preparation and the real isolated runtime."""

import ctypes
import importlib.util
import json
import os
from pathlib import Path
import signal
import socket
import ssl
import subprocess
import sys
import tempfile
import time
import unittest
import urllib.error
import urllib.request

SCRIPTS = Path(__file__).resolve().parent
REPO = SCRIPTS.parents[1]
SPEC = importlib.util.spec_from_file_location("termination_runner", SCRIPTS / "run.py")
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


def child_processes(parent):
    try:
        children = Path(f"/proc/{parent}/task/{parent}/children").read_text().split()
    except FileNotFoundError:
        return []
    records = []
    for value in children:
        pid = int(value)
        try:
            command = Path(f"/proc/{pid}/cmdline").read_bytes().decode().split("\0")
            records.append({"pid": pid, "pgid": os.getpgid(pid), "command": command})
        except ProcessLookupError:
            pass
        except FileNotFoundError:
            pass
    return records


def group_exists(group):
    try:
        os.killpg(group, 0)
        return True
    except ProcessLookupError:
        return False


def process_exists(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False


def reap_group(group):
    try:
        while os.waitpid(-group, os.WNOHANG)[0]:
            pass
    except ChildProcessError:
        pass


def groups_gone(groups, timeout):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        for group in groups:
            reap_group(group)
        if all(not group_exists(group) for group in groups):
            return True
        time.sleep(0.05)
    return False


def cleanup(process, groups):
    # Each group comes from a process this test actually launched or observed
    # as the runner's child; no command-name matching or broad process kill.
    for group in {*groups, process.pid}:
        try:
            os.killpg(group, signal.SIGKILL)
        except ProcessLookupError:
            pass
    process.wait(timeout=5)
    groups_gone(groups, 5)


def unused_ports():
    with socket.socket() as issuer, socket.socket() as bridge:
        issuer.bind(("127.0.0.1", 0))
        bridge.bind(("127.0.0.1", 0))
        return issuer.getsockname()[1], bridge.getsockname()[1]


def bridge_ready(fixture):
    context = ssl.create_default_context(cafile=fixture["caFile"])
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
        urllib.request.HTTPSHandler(context=context))
    try:
        with opener.open(fixture["bridgeOrigin"] + "/.well-known/build-bridge", timeout=0.5):
            return True
    except urllib.error.HTTPError as error:
        return error.code < 500
    except (OSError, urllib.error.URLError):
        return False


class TerminationWitnesses(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        # A failed control deliberately orphans the service groups. Reparent
        # those children to this test so exact-group cleanup can reap zombies.
        if ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) != 0:
            raise OSError(ctypes.get_errno(), "PR_SET_CHILD_SUBREAPER failed")

    def start(self, label, command, extra_env=None):
        directory = Path(tempfile.mkdtemp(prefix=f"termination-{label}-", dir=runner.SCRATCH))
        for name in ["home", "tmp"]:
            (directory / name).mkdir()
        env = {**runner.base_environment(directory), **(extra_env or {})}
        output = self.enterContext((directory / "runner.log").open("w"))
        process = subprocess.Popen(["nice", "-n", "10", *map(str, command)],
            cwd=REPO, env=env, stdout=output, stderr=subprocess.STDOUT, start_new_session=True)
        groups = set()
        self.addCleanup(cleanup, process, groups)
        return process, directory, groups

    def record_children(self, process, groups):
        children = child_processes(process.pid)
        groups.update(record["pgid"] for record in children if record["pgid"] == record["pid"])
        return children

    def terminate(self, process, directory, groups, services):
        self.record_children(process, groups)
        before = time.monotonic()
        os.kill(process.pid, signal.SIGTERM)
        code = process.wait(timeout=15)
        evidence = {"runner_pid": process.pid, "runner_exit": code,
            "service_processes": services, "recorded_process_groups": sorted(groups)}
        try:
            self.assertEqual(code, 143, f"SIGTERM must unwind cleanup; evidence: {directory}")
            self.assertTrue(groups_gone(groups, max(0, 15 - (time.monotonic() - before))),
                f"Service process groups survived SIGTERM; evidence: {directory}")
            for service in services:
                self.assertFalse(process_exists(service["pid"]), f"Service PID survived: {service}")
            evidence["all_groups_gone"] = True
            evidence["cleanup_seconds"] = round(time.monotonic() - before, 3)
        finally:
            (directory / "termination-evidence.json").write_text(json.dumps(evidence, indent=2) + "\n")
            print(f"Termination evidence: {directory}", flush=True)

    def test_sigterm_during_prepare_unwinds_run_logged(self):
        directory = Path(tempfile.mkdtemp(prefix="termination-prepare-helper-", dir=runner.SCRATCH))
        child_file = directory / "child.pid"
        helper = "import os,time,pathlib; pathlib.Path(" + repr(str(child_file)) + ").write_text(str(os.getpid())); time.sleep(60)"
        issuer_port, bridge_port = unused_ports()
        # Replace only the external preparation command. main() must register
        # handlers before calling the real run_logged(), whose child is real.
        program = "\n".join([
            "import importlib.util,pathlib,sys",
            f"spec=importlib.util.spec_from_file_location('runner',{str(SCRIPTS / 'run.py')!r})",
            "module=importlib.util.module_from_spec(spec);spec.loader.exec_module(module)",
            "original=module.run_logged",
            f"def prepare(command,**kwargs): original([sys.executable,'-c',{helper!r}],**kwargs)",
            "module.run_logged=prepare",
            "module.qualification_binary=lambda value:pathlib.Path('/unused-test-binary')",
            f"sys.argv=['qualification','--issuer-port',{str(issuer_port)!r},'--bridge-port',{str(bridge_port)!r}]",
            "module.main()",
        ])
        process, artifact, groups = self.start("prepare", [sys.executable, "-c", program])
        deadline = time.monotonic() + 10
        while not child_file.exists() and time.monotonic() < deadline:
            self.record_children(process, groups)
            self.assertIsNone(process.poll(), f"Runner exited before preparation: {artifact}")
            time.sleep(0.05)
        self.assertTrue(child_file.exists(), f"Preparation did not start: {artifact}")
        pid = int(child_file.read_text())
        groups.add(os.getpgid(pid))
        self.terminate(process, artifact, groups, [{"kind": "prepare", "pid": pid, "pgid": pid}])

    @unittest.skipUnless(os.environ.get("BUILD_HTTP_QUALIFICATION_TERMINATION") == "1",
        "set BUILD_HTTP_QUALIFICATION_TERMINATION=1 for the real Skrift/bridge witness")
    def test_sigterm_stops_real_issuer_and_bridge_process_groups(self):
        binary = runner.qualification_binary(os.environ.get("BUILD_HTTP_QUALIFICATION_BINARY"))
        issuer_port, bridge_port = unused_ports()
        process, directory, groups = self.start("runtime", [sys.executable, SCRIPTS / "run.py",
            "--issuer-port", issuer_port, "--bridge-port", bridge_port],
            {"BUILD_HTTP_QUALIFICATION_BINARY": str(binary)})
        services = self.wait_for_runtime(process, directory, groups)
        self.terminate(process, directory, groups, services)

    def wait_for_runtime(self, process, directory, groups):
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            self.assertIsNone(process.poll(), f"Runner exited before bridge readiness: {directory}")
            children = self.record_children(process, groups)
            services = [self.service_record(child) for child in children]
            services = [service for service in services if service is not None]
            lines = (directory / "runner.log").read_text().splitlines()
            fixture_path = next((line.removeprefix("Private fixture: ") for line in lines
                if line.startswith("Private fixture: ")), None)
            if fixture_path and {service["kind"] for service in services} == {"issuer", "bridge"}:
                fixture = json.loads(Path(fixture_path).read_text())
                if bridge_ready(fixture):
                    return services
            time.sleep(0.05)
        self.fail(f"Real issuer/bridge did not become ready: {directory}")

    @staticmethod
    def service_record(child):
        command = child["command"]
        if "serve" not in command:
            return None
        if any(value.endswith("fixture.py") for value in command):
            kind = "issuer"
        elif any(value.endswith("build-bridge") for value in command):
            kind = "bridge"
        else:
            return None
        return {"kind": kind, "pid": child["pid"], "pgid": child["pgid"]}


if __name__ == "__main__":
    unittest.main()
