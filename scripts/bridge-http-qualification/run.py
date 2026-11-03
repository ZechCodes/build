"""Run both real clients against an explicitly supplied cargo-built bridge."""

import argparse
from contextlib import ExitStack
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
import urllib.error
import urllib.request

SCRATCH = Path("/home/zech/Projects/.agent-scratch/976")


def scratch_for(task):
    if task not in (976, 977):
        raise ValueError("qualification only supports task 976 or 977 scratch")
    return Path("/home/zech/Projects/.agent-scratch") / str(task)


def install_shutdown_handlers():
    def shutdown(number, _frame):
        raise SystemExit(128 + number)

    for number in (signal.SIGTERM, signal.SIGHUP):
        signal.signal(number, shutdown)


def qualification_binary(raw):
    if not raw:
        raise ValueError("BUILD_HTTP_QUALIFICATION_BINARY must explicitly name the cargo-built binary")
    path = Path(raw)
    if not path.is_absolute():
        raise ValueError("BUILD_HTTP_QUALIFICATION_BINARY must be absolute")
    path = path.resolve(strict=True)
    if not path.is_relative_to((SCRATCH / "target").resolve()):
        raise ValueError("Only the task scratch cargo target binary may run")
    if not path.is_file() or not os.access(path, os.X_OK):
        raise ValueError("Qualification binary must be an executable file")
    return path


def base_environment(run_dir):
    return {
        "PATH": os.environ.get("PATH", "/usr/local/bin:/usr/bin:/bin"),
        "LANG": "C.UTF-8", "HOME": str(run_dir / "home"),
        "XDG_CONFIG_HOME": str(run_dir / "home/.config"),
        "XDG_CACHE_HOME": str(run_dir / "home/.cache"),
        "XDG_DATA_HOME": str(run_dir / "home/.local/share"),
        "XDG_STATE_HOME": str(run_dir / "home/.local/state"),
        "TMPDIR": str(run_dir / "tmp"), "CARGO_BUILD_JOBS": "8",
        "CARGO_TARGET_DIR": str(SCRATCH / "target"),
        "CHROMIUM_PATH": os.environ.get("CHROMIUM_PATH", "/usr/bin/chromium"),
    }


def bridge_environment(env, fixture, port):
    home = Path(env["HOME"])
    run_dir = Path(fixture["runDir"])
    return {**env,
        "SSL_CERT_FILE": fixture["caFile"], "SSL_CERT_DIR": str(run_dir / "empty-ca-dir"),
        "BRIDGE_CHANNEL": "dev", "BRIDGE_API_URL": fixture["hostedOrigin"],
        "BRIDGE_WEB_URL": fixture["hostedOrigin"], "BRIDGE_RELAY_URL": "ws://127.0.0.1:18477",
        "BRIDGE_IDENTITY_FILE": fixture["identityFile"], "BRIDGE_CONFIG": str(home / "config.json"),
        "BRIDGE_TASKS_DIR": str(home / "tasks"), "BRIDGE_PROJECTS_DIR": fixture["projectsDir"],
        "BRIDGE_WORKTREES": str(home / "worktrees"), "BRIDGE_MCP_SOCKET": str(home / "mcp.sock"),
        "BRIDGE_CHILD_SCOPE": "0", "BRIDGE_PATH_FROM_SHELL": "0", "BRIDGE_QA_AGENT": "1",
        "BRIDGE_WORKSPACE_PRUNE": "0", "BRIDGE_HTTP_ENABLED": "true",
        "BRIDGE_HTTP_BIND": f"127.0.0.1:{port}", "BRIDGE_HTTP_ORIGIN": fixture["bridgeOrigin"],
        "BRIDGE_HTTP_TLS_CERT": str(run_dir / "cert.pem"), "BRIDGE_HTTP_TLS_KEY": str(run_dir / "key.pem"),
        "BRIDGE_HTTP_ALLOWED_ORIGINS": fixture["hostedOrigin"],
    }


def bridge_launch(task, binary, fixture_path, env, fixture, port):
    if task == 977:
        if binary.name != "bridge-http-qualification":
            raise ValueError("task 977 requires the typed bridge-http-qualification example")
        return [binary, fixture_path], {**env, "SSL_CERT_FILE": fixture["caFile"],
            "SSL_CERT_DIR": str(Path(fixture["runDir"]) / "empty-ca-dir")}
    return [binary, "serve"], bridge_environment(env, fixture, port)


def stop_process(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=10)
    except subprocess.TimeoutExpired:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=5)


def ensure_free_ports(ports):
    with ExitStack() as stack:
        for port in ports:
            listener = stack.enter_context(socket.socket())
            listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
            listener.bind(("127.0.0.1", port))


def launch(stack, command, *, env, cwd, log):
    stream = stack.enter_context(log.open("w"))
    process = subprocess.Popen(["nice", "-n", "10", *map(str, command)], env=env,
        cwd=cwd, stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
    stack.callback(stop_process, process)
    return process


def wait_ready(process, url, ca_file, log):
    context = ssl.create_default_context(cafile=str(ca_file))
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}),
        urllib.request.HTTPSHandler(context=context))
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"Local service exited {process.returncode}; see {log}")
        try:
            with opener.open(url, timeout=1):
                return
        except urllib.error.HTTPError as error:
            if error.code < 500:
                return
        except (OSError, urllib.error.URLError):
            pass
        time.sleep(0.2)
    raise TimeoutError(f"Local HTTPS service did not start; see {log}")


def run_logged(command, *, env, cwd, log, timeout=300):
    with log.open("w") as stream:
        process = subprocess.Popen(["nice", "-n", "10", *map(str, command)], env=env, cwd=cwd,
            stdout=stream, stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        finally:
            stop_process(process)
    print(f"{log.name}: exit={code}", flush=True)
    if code:
        raise RuntimeError(f"Qualification command failed; see {log}")


def main():
    global SCRATCH
    install_shutdown_handlers()
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--issuer-port", type=int, default=18476)
    parser.add_argument("--bridge-port", type=int, default=19476)
    parser.add_argument("--scratch-task", type=int, choices=[976, 977], default=976)
    args = parser.parse_args()
    SCRATCH = scratch_for(args.scratch_task)
    if not all(1024 <= port <= 65535 for port in [args.issuer_port, args.bridge_port]):
        parser.error("ports must be unprivileged loopback ports")
    try:
        binary = qualification_binary(os.environ.get("BUILD_HTTP_QUALIFICATION_BINARY"))
        ensure_free_ports([args.issuer_port, args.bridge_port])
    except (ValueError, OSError) as error:
        parser.error(str(error))
    repo = args.repo.resolve()
    script_dir = Path(__file__).resolve().parent
    SCRATCH.mkdir(parents=True, exist_ok=True)
    run_dir = Path(tempfile.mkdtemp(prefix="run-", dir=SCRATCH))
    for name in ["home", "tmp", "empty-ca-dir"]:
        (run_dir / name).mkdir()
    env = base_environment(run_dir)
    fixture_args = [sys.executable, script_dir / "fixture.py", "--repo", repo, "--run-dir", run_dir,
        "--scratch-task", args.scratch_task,
        "--issuer-port", args.issuer_port, "--bridge-port", args.bridge_port]
    run_logged([*fixture_args, "prepare"], env=env, cwd=repo, log=run_dir / "prepare.log")
    fixture_path = run_dir / "runtime.json"
    fixture = json.loads(fixture_path.read_text())
    print(f"Private fixture: {fixture_path}", flush=True)
    with ExitStack() as stack:
        issuer = launch(stack, [*fixture_args, "serve"], env=env, cwd=repo, log=run_dir / "issuer.log")
        wait_ready(issuer, fixture["hostedOrigin"] + "/auth/dummy/login", run_dir / "ca.pem", run_dir / "issuer.log")
        command, bridge_env = bridge_launch(args.scratch_task, binary, fixture_path, env, fixture, args.bridge_port)
        bridge = launch(stack, command, env=bridge_env,
            cwd=run_dir, log=run_dir / "bridge.log")
        wait_ready(bridge, fixture["bridgeOrigin"] + "/.well-known/build-bridge", run_dir / "ca.pem", run_dir / "bridge.log")
        client_env = {**env, "BUILD_HTTP_QUALIFICATION_FIXTURE": str(fixture_path),
            "NODE_EXTRA_CA_CERTS": str(run_dir / "ca.pem"), "SCREENSHOT_DIR": str(run_dir / "screenshots")}
        run_logged([sys.executable, script_dir / "discovery.py", "--repo", repo, "--scratch-task", args.scratch_task],
            env=client_env, cwd=repo, log=run_dir / "discovery.log")
        run_logged(["node", script_dir / "browser.mjs"], env=client_env, cwd=repo, log=run_dir / "browser.log")
        run_logged(["node", "--test", repo / "desktop/test/bridge-http-qualification.test.mjs"],
            env=client_env, cwd=repo, log=run_dir / "desktop.log")
    print(f"Both client lifecycles passed; evidence: {run_dir}", flush=True)


if __name__ == "__main__":
    main()
