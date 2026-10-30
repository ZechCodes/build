#!/usr/bin/env python3
"""Opt-in six-admission proposal. Never execute without a fresh reservation."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time
from datetime import datetime, timezone


def utc():
    return datetime.now(timezone.utc).isoformat()


def head_at(root):
    git = root / ".git"
    if git.is_file():
        git = (root / git.read_text().strip().removeprefix("gitdir: ")).resolve()
    head = (git / "HEAD").read_text().strip()
    if not head.startswith("ref: "):
        return head
    ref = head.removeprefix("ref: ")
    common = (git / (git / "commondir").read_text().strip()).resolve() if (git / "commondir").exists() else git
    for location in (git / ref, common / ref):
        if location.exists():
            return location.read_text().strip()
    for line in (common / "packed-refs").read_text().splitlines():
        if line.endswith(" " + ref):
            return line.split()[0]
    raise RuntimeError("Cannot establish exact frozen HEAD")


def children(include_all=False):
    # Inventory actual executable/state, not shell command-string matches.
    records = []
    for proc in Path("/proc").iterdir():
        if not proc.name.isdigit():
            continue
        try:
            stat = (proc / "stat").read_text()
            fields = stat[stat.rfind(")") + 2:].split()
            comm = (proc / "comm").read_text().strip()
            if not include_all and comm not in {"node", "cargo", "rustc", "chromium", "chrome", "esbuild"}:
                continue
            records.append({"pid": int(proc.name), "state": fields[0], "ppid": int(fields[1]),
                            "session": int(fields[3]), "comm": comm, "exe": str((proc / "exe").resolve())})
        except (OSError, ProcessLookupError):
            continue
    return records


def qualify(directory, red, head, source_hash):
    traces = {}
    for name in ("delayed-landing", "invalidation-after-commit"):
        data = json.loads((directory / f"{name}.json").read_text())
        if data.get("head") != head or data.get("mode") != ("red" if red else "green"):
            raise RuntimeError("INCONCLUSIVE artifact head/mode provenance")
        receipt = json.loads((directory / f"{name}.json.receipt").read_text())
        digest = hashlib.sha256((directory / f"{name}.json").read_bytes()).hexdigest()
        if receipt != {"persisted": True, "sha256": digest}:
            raise RuntimeError("INCONCLUSIVE capture persistence receipt")
        if not data["instrumentationComplete"] or data["lost"]:
            raise RuntimeError("INCONCLUSIVE capture loss")
        events = data["events"]
        hooks = [event for event in events if event["type"] == "fixture-hooks-registered"]
        if len(hooks) != 1 or hooks[0].get("sourceHash") != source_hash:
            raise RuntimeError("INCONCLUSIVE tested-source provenance")
        if [event["sequence"] for event in events] != list(range(1, len(events) + 1)):
            raise RuntimeError("INCONCLUSIVE trace sequence")
        if events[0]["type"] != "capture-registered" or events[-1]["type"] != "end-observation":
            raise RuntimeError("INCONCLUSIVE capture coverage/flush markers")
        traces[name] = events
    events = traces["delayed-landing"]
    types = [event["type"] for event in events]
    ordered = ["initial-complete-before-background-ack"]
    if red:
        ordered += ["seed-attempt", "seed-committed", "a805-barrier-unqualified-seed"]
    ordered += ["background-response-released", "successor-held", "generation-invalidated-during-formation",
                "successor-read-released", "coverage-revalidated"]
    if not red:
        ordered += ["seed-attempt", "seed-committed", "first-intended-push", "all-original-assertions-passed"]
    cursor = -1
    for kind in ordered:
        cursor = types.index(kind, cursor + 1)
    if any(event["result"] is not True for event in events if event["type"] == "pass-completed"):
        raise RuntimeError("Recovery pass did not succeed")
    if "invalidation-detected-after-seed-commit" not in [event["type"] for event in traces["invalidation-after-commit"]]:
        raise RuntimeError("INCONCLUSIVE missing invalidation case")
    secondary_types = [event["type"] for event in traces["invalidation-after-commit"]]
    if "raw-test-error" in secondary_types:
        raise RuntimeError("Invalidation case failed")
    errors = [event.get("error", "") for event in events if event["type"] == "raw-test-error"]
    if red and (len(errors) != 1 or "EXPECTED_READINESS_RED" not in errors[0]):
        raise RuntimeError("RED lacked the exact qualified detector")
    if not red and errors:
        raise RuntimeError("GREEN test failed")
    return {"complete": True, "ordering": ordered}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true", required=True)
    parser.add_argument("--reservation", required=True)
    parser.add_argument("--start", required=True, help="Explicit coordinator UTC ISO timestamp")
    parser.add_argument("--cutoff", required=True, help="Explicit UTC ISO timestamp, at most start+8min")
    parser.add_argument("--head", required=True, help="Reviewed full immutable successor SHA")
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--artifacts", type=Path, required=True)
    args = parser.parse_args()
    timestamps = [datetime.fromisoformat(value.replace("Z", "+00:00")) for value in (args.start, args.cutoff)]
    if any(value.tzinfo is None or value.utcoffset().total_seconds() != 0 for value in timestamps):
        raise RuntimeError("Explicit UTC START/cutoff timestamps are required")
    start, cutoff = (value.timestamp() for value in timestamps)
    if not 0 < cutoff - start <= 480 or not start <= time.time() < cutoff:
        raise RuntimeError("No current eight-minute admission authorization")
    if head_at(args.root) != args.head:
        raise RuntimeError("HEAD differs from reviewed successor")
    args.artifacts.mkdir(parents=True, exist_ok=False)
    ledger = args.artifacts / "ledger.ndjson"

    def record(data):
        with ledger.open("a") as stream:
            stream.write(json.dumps({"at": utc(), **data}) + "\n")
            stream.flush()
            os.fsync(stream.fileno())

    node = Path("/home/zech/.local/share/mise/installs/node/22.23.4/bin/node")
    npm = Path("/home/zech/.cache/node/corepack/v1/npm/12.0.2/bin/npm-cli.js")
    inventory = children()
    record({"type": "inventory", "reservation": args.reservation, "head": args.head, "processes": inventory,
            "node": str(node), "node_sha256": hashlib.sha256(node.read_bytes()).hexdigest(),
            "npm": str(npm), "npm_sha256": hashlib.sha256(npm.read_bytes()).hexdigest()})
    initial_active = any(item["state"] not in {"T", "t", "Z", "X"} for item in inventory)
    spa = args.root / "spa"
    vitest = [str(node), "node_modules/vitest/vitest.mjs", "run", "--project", "unit", "--maxWorkers", "1"]
    diagnostic = vitest + ["--config", "test/diagnostics/cacheSyncReadiness.config.mjs", "test/cacheSyncDelivery.test.js", "-t", "909 readiness diagnostic"]
    original = vitest + ["test/cacheSyncDelivery.test.js", "-t", "writes nothing back from a push read the sync layer stood down under"]
    steps = [
        ("transport-install", args.root, [str(node), str(npm), "ci", "--prefix", "transport/js", "--ignore-scripts", "--loglevel", "verbose"], None),
        ("spa-install", spa, [str(node), str(npm), "ci", "--legacy-peer-deps", "--ignore-scripts", "--loglevel", "verbose"], None),
        ("red", spa, diagnostic, "red"), ("green", spa, diagnostic, "green"),
        ("original-case", spa, original, None),
        ("full-file", spa, vitest + ["test/cacheSyncDelivery.test.js"], None),
    ]
    admitted, stop, completed, ledger_failed = 0, None, set(), False
    sessions = set()

    def safe_record(data):
        nonlocal stop, ledger_failed
        try:
            record(data)
        except Exception as error:
            ledger_failed = True
            stop = stop or "Ledger persistence failure"
            # The caller must retain driver stderr as well as raw child logs.
            print(json.dumps({"ledger_failure": str(error), "unpersisted": data}), file=sys.stderr, flush=True)

    def clean_source():
        if head_at(args.root) != args.head:
            raise RuntimeError("HEAD changed")
        # Read-only Git metadata, not an additional install/test admission.
        status = subprocess.run(["git", "status", "--porcelain", "--untracked-files=normal"], cwd=args.root,
                                capture_output=True, text=True, env={**os.environ, "GIT_OPTIONAL_LOCKS": "0"})
        if status.returncode or status.stdout:
            raise RuntimeError("Checkout is not clean; immutable provenance unavailable")

    def await_owned():
        remaining = [item for item in children(include_all=True) if item["session"] in sessions and item["state"] not in {"Z", "X"}]
        if remaining:
            safe_record({"type": "awaiting-natural-descendants", "remaining": remaining})
        while remaining:
            time.sleep(0.25)
            remaining = [item for item in children(include_all=True) if item["session"] in sessions and item["state"] not in {"Z", "X"}]
        safe_record({"type": "owned-child-inventory", "remaining": []})

    try:
        if initial_active:
            raise RuntimeError("Active possible competing work; zero admissions, coordinator clarification needed")
        clean_source()
        source_hash = hashlib.sha256((spa / "test/cacheSyncDelivery.test.js").read_bytes()).hexdigest()
        for name, cwd, command, mode in steps:
            if stop or time.time() >= cutoff:
                stop = stop or "admission cutoff"
                safe_record({"type": "skipped", "name": name, "reason": stop})
                completed.add(name)
                continue
            clean_source()
            fresh = children()
            record({"type": "pre-admission-inventory", "name": name, "processes": fresh})
            if any(item["state"] not in {"T", "t", "Z", "X"} for item in fresh):
                raise RuntimeError("Active possible competing work before admission")
            environment = dict(os.environ)
            if mode:
                directory = args.artifacts / name
                directory.mkdir()
                environment.update(READINESS_MODE=mode, READINESS_ROOT=str(args.root), READINESS_HEAD=args.head,
                                   READINESS_ARTIFACT_DIR=str(directory))
            invocation = ["nice", "-n", "10", *command]
            with (args.artifacts / f"{name}.log").open("w") as output:
                # Recheck immediately before admission, with a conservative
                # margin for process creation; no clock-based extension.
                if time.time() + 0.25 >= cutoff:
                    stop = "admission cutoff"
                    record({"type": "skipped", "name": name, "reason": stop})
                    completed.add(name)
                    continue
                process = subprocess.Popen(invocation, cwd=cwd, env=environment, stdout=output,
                                           stderr=subprocess.STDOUT, start_new_session=True)
                sessions.add(process.pid)
                admitted += 1
                try:
                    record({"type": "admitted", "name": name, "number": admitted, "pid": process.pid,
                            "session": process.pid, "cwd": str(cwd), "command": invocation})
                finally:
                    # Even a failed ledger write cannot orphan an admitted
                    # child or turn into a kill/compensation invocation.
                    code = process.wait()
                    safe_record({"type": "exit", "name": name, "code": code})
                    output.flush()
                    os.fsync(output.fileno())
            completed.add(name)
            await_owned()
            if mode:
                qualification = qualify(directory, mode == "red", args.head, source_hash)
                expected = 1 if mode == "red" else 0
                raw = (args.artifacts / f"{name}.log").read_text()
                if code != expected or (mode == "red" and "EXPECTED_READINESS_RED" not in raw):
                    raise RuntimeError("Unexpected raw diagnostic outcome")
                record({"type": "qualification", "name": name, **qualification})
            elif code != 0:
                stop = f"{name} failed"
    except Exception as error:
        stop = str(error)
        safe_record({"type": "stopped-or-inconclusive", "error": stop})
    finally:
        await_owned()
        for name, _, _, _ in steps:
            if name not in completed:
                safe_record({"type": "skipped-or-uncompleted", "name": name, "reason": stop})
        safe_record({"type": "finished", "admitted": admitted, "stop": stop, "success": stop is None and not ledger_failed and admitted == 6,
                     "head_after": head_at(args.root), "fresh_inventory": children()})
    return 0 if stop is None and not ledger_failed and admitted == 6 else 1


if __name__ == "__main__":
    sys.exit(main())
