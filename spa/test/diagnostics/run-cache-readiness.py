#!/usr/bin/env python3
"""Opt-in bounded proposals. Never execute without a fresh reservation."""
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


def read_traces(directory, names, mode, head, source_hash):
    traces = {}
    for name in names:
        data = json.loads((directory / f"{name}.json").read_text())
        if data.get("head") != head or data.get("mode") != mode:
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
    return traces


def qualify(directory, red, head, source_hash):
    traces = read_traces(directory, ("delayed-landing", "invalidation-after-commit"),
                         "red" if red else "green", head, source_hash)
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


def qualify_visibility(directory, head, source_hash):
    names = ("visibility-pending", "visibility-sealed", "visibility-disposed")
    traces = read_traces(directory, names, "visibility", head, source_hash)
    for name, events in traces.items():
        def one(kind):
            matches = [event for event in events if event["type"] == kind]
            if len(matches) != 1:
                raise RuntimeError(f"INCONCLUSIVE {name}: expected one {kind}")
            return matches[0]

        if any(event["type"] == "raw-test-error" for event in events):
            raise RuntimeError(f"Visibility case failed: {name}")
        final = one(f"{name}-qualified")
        bubble_registration = one("bubble-probe-registered")
        bubble_removal = one("bubble-probe-removed")
        probe_disposed = one("visibility-probe-disposed")
        hidden_restored = one("visibility-hidden-spy-restored")
        if any(event["type"] == "guard-cleanup-fallback" for event in events):
            raise RuntimeError(f"INCONCLUSIVE fixture guard required fallback cleanup: {name}")
        registered, removed = one("guard-registered"), one("guard-removed")
        dispatch, bubble = one("visibility-dispatched"), one("bubble-probe")
        observed, completed_visibility = one("production-pass-observed"), one("production-pass-completed")
        if (registered.get("capture") is not True or registered.get("nativeRegistered") is not True
                or registered.get("origin") != "rejectVisibility"
                or registered.get("original") is None or registered.get("wrapper") is None
                or registered.get("original") == registered.get("wrapper")
                or removed.get("capture") is not True or removed.get("nativeRemoved") is not True
                or removed.get("reason") != "fixture"
                or (registered.get("original"), registered.get("wrapper")) != (removed.get("original"), removed.get("wrapper"))):
            raise RuntimeError(f"INCONCLUSIVE guard registration/removal identity: {name}")
        event_id = dispatch.get("event")
        if (event_id is None or any(event.get("event") != event_id for event in (bubble, observed, completed_visibility))
                or observed.get("pass") is None or observed.get("pass") != dispatch.get("productionPass")
                or completed_visibility.get("pass") != observed.get("pass") or completed_visibility.get("result") is not True):
            raise RuntimeError(f"INCONCLUSIVE event/production pass identity: {name}")
        expected_hits = 0 if name == "visibility-disposed" else 1
        if (dispatch.get("dispatchReturned") is not True or dispatch.get("defaultPrevented") is not False
                or bubble.get("defaultPrevented") is not False
                or bubble.get("eventPhase") != 3
                or bubble_registration.get("nativeRegistered") is not True
                or bubble_registration.get("capture") is not False
                or bubble_registration.get("target") is None
                or bubble_removal.get("nativeRemoved") is not True
                or (bubble_registration.get("original"), bubble_registration.get("target"))
                != (bubble_removal.get("original"), bubble_removal.get("target"))
                or dispatch.get("guardCallsBefore") != 0 or dispatch.get("guardCallsAfter") != expected_hits
                or dispatch.get("bubbleCallsAfter") != dispatch.get("bubbleCallsBefore", -2) + 1):
            raise RuntimeError(f"Visibility event was suppressed or guard calls mismatched: {name}")
        if not (registered["sequence"] < dispatch["sequence"] < observed["sequence"]
                < completed_visibility["sequence"] < final["sequence"]
                and bubble_registration["sequence"] < bubble["sequence"] < dispatch["sequence"]
                and removed["sequence"] < final["sequence"]
                < hidden_restored["sequence"] < bubble_removal["sequence"] < probe_disposed["sequence"]):
            raise RuntimeError(f"INCONCLUSIVE visibility control ordering: {name}")
        calls = [event for event in events if event["type"] == "guard-called"]
        if len(calls) != expected_hits:
            raise RuntimeError(f"INCONCLUSIVE actual guard call coverage: {name}")
        if calls and (calls[0].get("event") != event_id or calls[0].get("original") != registered.get("original")
                      or calls[0].get("wrapper") != registered.get("wrapper") or calls[0]["sequence"] >= dispatch["sequence"]):
            raise RuntimeError(f"INCONCLUSIVE actual guard call identity/order: {name}")
        if name == "visibility-pending":
            before = one("initial-complete-before-background-ack")
            rejected = one("ready-rejected-visibility")
            invalidated = one("pending-invalidated-visibility")
            if not before["sequence"] < dispatch["sequence"] < rejected["sequence"] < invalidated["sequence"] < final["sequence"]:
                raise RuntimeError("INCONCLUSIVE pending-readiness rejection order")
        else:
            if not one("barrier-ready")["sequence"] < dispatch["sequence"]:
                raise RuntimeError("INCONCLUSIVE event occurred before seal")
            if name == "visibility-sealed":
                for kind in ("post-seal-invalidated-visibility", "ready-remained-fulfilled"):
                    if not dispatch["sequence"] < one(kind)["sequence"] < final["sequence"]:
                        raise RuntimeError("INCONCLUSIVE post-seal visibility invalidation")
            elif not (removed["sequence"] < one("guard-disposed")["sequence"] < dispatch["sequence"]
                      < one("dispatch-after-dispose-no-guard-hits")["sequence"] < final["sequence"]):
                raise RuntimeError("INCONCLUSIVE guard disposal/noninterference order")
        completed = [event for event in events if event["type"] == "pass-completed"]
        if not completed or any(event["result"] is not True for event in completed):
            raise RuntimeError(f"Production visibility pass did not complete successfully: {name}")
    return {"complete": True, "cases": list(names)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--execute", action="store_true", required=True)
    parser.add_argument("--plan", choices=("readiness", "visibility"), default="readiness")
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
    if args.plan == "visibility" and cutoff - start > 180:
        raise RuntimeError("Visibility proposal requires a minute-three admission cutoff")
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
    record({"type": "inventory", "reservation": args.reservation, "plan": args.plan,
            "start": args.start, "cutoff": args.cutoff, "max_admissions": 3 if args.plan == "visibility" else 6,
            "head": args.head, "processes": inventory,
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
    if args.plan == "visibility":
        command = vitest + ["--config", "test/diagnostics/cacheSyncReadiness.config.mjs",
                            "test/cacheSyncDelivery.test.js", "-t", "909 visibility diagnostic"]
        steps = steps[:2] + [("visibility", spa, command, "visibility")]
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
                qualification = (qualify_visibility(directory, args.head, source_hash) if mode == "visibility"
                                 else qualify(directory, mode == "red", args.head, source_hash))
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
        try:
            clean_source()
        except Exception as error:
            stop = f"Final source provenance failed: {error}"
            safe_record({"type": "stopped-or-inconclusive", "error": stop})
        for name, _, _, _ in steps:
            if name not in completed:
                safe_record({"type": "skipped-or-uncompleted", "name": name, "reason": stop})
        safe_record({"type": "finished", "admitted": admitted, "stop": stop, "success": stop is None and not ledger_failed and admitted == len(steps),
                     "head_after": head_at(args.root), "fresh_inventory": children()})
    return 0 if stop is None and not ledger_failed and admitted == len(steps) else 1


if __name__ == "__main__":
    sys.exit(main())
