#!/usr/bin/env python3
"""Record and summarize the approved 24-process public Store comparison.

This script never builds, checks out, copies, or changes source or binaries.
Both manifest-pinned release binaries must exist before `run` is invoked.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import statistics
import subprocess
import time

TESTS = (
    "identical_small_task_write_fixture",
    "identical_huge_history_write_fixture",
    "identical_small_metadata_write_fixture",
    "identical_huge_metadata_write_fixture",
)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def git(root, *args):
    return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()


def check(manifest):
    root = Path(manifest["source_root"])
    assert git(root, "rev-parse", "HEAD") == manifest["source_head"], "HEAD changed"
    assert not git(root, "status", "--porcelain"), "source tree is not clean"
    assert digest(root / "bridge/tests/bounded_task_write_perf.rs") == manifest["fixture_sha256"]
    assert digest(manifest["binary"]) == manifest["binary_sha256"], "binary changed"
    assert manifest["build_exit"] == 0
    assert manifest["profile"]["opt_level"] == "3"
    assert manifest["profile"]["debug_assertions"] is False
    assert "testing" in manifest["features"]


def run(args):
    manifests = {name: json.loads(Path(path).read_text()) for name, path in
                 (("base", args.base), ("candidate", args.candidate))}
    for manifest in manifests.values():
        check(manifest)
    assert manifests["base"]["source_head"] == "12b8f869bd94e259666df69f1a8c0cde5c695982"
    assert manifests["base"]["fixture_sha256"] == manifests["candidate"]["fixture_sha256"]
    assert manifests["base"]["binary"] != manifests["candidate"]["binary"]
    output = Path(args.output)
    output.mkdir(parents=True, exist_ok=False)
    (output / "manifests.json").write_text(json.dumps(manifests, indent=2) + "\n")
    (output / "runner-sha256.txt").write_text(digest(__file__) + "\n")
    index = 0
    with (output / "runs.jsonl").open("w") as journal:
        for round_number, order in enumerate((("base", "candidate"),
                                               ("candidate", "base"),
                                               ("base", "candidate")), 1):
            for test in TESTS:
                for variant in order:
                    manifest = manifests[variant]
                    check(manifest)
                    index += 1
                    log_path = output / f"{index:02d}-r{round_number}-{variant}-{test}.log"
                    command = ["nice", "-n", "10", manifest["binary"], test,
                               "--exact", "--nocapture", "--test-threads=1"]
                    metadata = dict(index=index, round=round_number, variant=variant,
                                    test=test, command=command, log=str(log_path),
                                    source_head=manifest["source_head"],
                                    fixture_sha256=manifest["fixture_sha256"],
                                    binary_sha256=manifest["binary_sha256"],
                                    inherited_niceness=os.getpriority(os.PRIO_PROCESS, 0),
                                    api_guard=manifest["api_guard"])
                    environment = os.environ.copy()
                    environment["TASK890_EXPECTED_API"] = manifest["api_guard"]
                    environment.pop("TASK890_PROFILE_SQL", None)
                    with log_path.open("w") as log:
                        log.write("run_metadata=" + json.dumps(metadata, sort_keys=True) + "\n")
                        log.flush()
                        started = time.monotonic_ns()
                        result = subprocess.run(command, cwd=manifest["source_root"],
                                                env=environment, stdout=log,
                                                stderr=subprocess.STDOUT)
                        metadata["process_elapsed_ns"] = time.monotonic_ns() - started
                        metadata["process_exit"] = result.returncode
                        log.write("\nprocess_exit=" + str(result.returncode) + "\n")
                    journal.write(json.dumps(metadata, sort_keys=True) + "\n")
                    journal.flush()
                    print(f"run={index}/24 variant={variant} test={test} exit={result.returncode}", flush=True)
                    assert result.returncode == 0, f"failed run retained: {log_path}"
                    check(manifest)
                    text = log_path.read_text()
                    assert "api_version=" + manifest["api_guard"] in text
                    assert "test result: ok. 1 passed; 0 failed" in text
    for manifest in manifests.values():
        check(manifest)


def sample(line):
    fields = dict(re.findall(r"(\w+)=([^\s]+)", line))
    if "migrated_reopen_us" in fields:
        phase, wall, cpu = "completed_reopen", "migrated_reopen_us", "migrated_reopen_cpu_us"
    elif "post_reopen_write_us" in fields:
        phase = "post_reopen_" + fields["post_reopen_write_kind"]
        wall, cpu = "post_reopen_write_us", "post_reopen_write_cpu_us"
    elif "append_us" in fields:
        phase = "initial_first" if fields["append_sample"] == "0" else "initial_warm"
        wall, cpu = "append_us", "append_cpu_us"
    elif "write_us" in fields:
        phase, wall, cpu = "metadata_" + fields["phase"], "write_us", "write_cpu_us"
    elif "startup_us" in fields:
        phase, wall, cpu = "forced_repair_startup", "startup_us", "startup_cpu_us"
    else:
        return None
    memory = re.search(r"VmHWM:\s+(\d+)", line)
    return dict(phase=phase, wall_us=int(fields[wall]), thread_cpu_us=int(fields[cpu]),
                raw_hwm_kib=int(memory.group(1)) if memory else None,
                raw_line=line)


def summarize(values):
    return dict(n=len(values), median=statistics.median(values),
                minimum=min(values), maximum=max(values))


def parse(args):
    output = Path(args.output)
    runs = [json.loads(line) for line in (output / "runs.jsonl").read_text().splitlines()]
    assert len(runs) == 24, f"incomplete cohort: {len(runs)}/24; raw failures remain retained"
    samples, groups, memory = [], {}, []
    for row in runs:
        assert row["process_exit"] == 0
        text = Path(row["log"]).read_text()
        hwms = [int(value) for value in re.findall(r"VmHWM:\s+(\d+)", text)]
        memory.append(dict(index=row["index"], variant=row["variant"], test=row["test"],
                           max_reported_hwm_kib=max(hwms) if hwms else None))
        for line in text.splitlines():
            parsed = sample(line)
            if parsed is None:
                continue
            parsed.update(index=row["index"], round=row["round"],
                          variant=row["variant"], test=row["test"])
            samples.append(parsed)
            key = (row["variant"], row["test"], parsed["phase"])
            groups.setdefault(key, []).append(parsed)
    summaries = [dict(variant=key[0], test=key[1], phase=key[2],
                      wall_us=summarize([row["wall_us"] for row in rows]),
                      thread_cpu_us=summarize([row["thread_cpu_us"] for row in rows]))
                 for key, rows in groups.items()]
    result = dict(runs=runs, samples=samples, groups=summaries,
                  process_max_reported_hwm=memory)
    (output / "parsed.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(summaries, indent=2))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    runner = commands.add_parser("run")
    runner.add_argument("--base", required=True)
    runner.add_argument("--candidate", required=True)
    runner.add_argument("--output", required=True)
    runner.set_defaults(function=run)
    summarizer = commands.add_parser("parse")
    summarizer.add_argument("--output", required=True)
    summarizer.set_defaults(function=parse)
    args = parser.parse_args()
    args.function(args)


if __name__ == "__main__":
    main()
