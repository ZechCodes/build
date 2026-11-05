#!/usr/bin/env python3
"""Fixed, paired #894 append cohort. No retries; retain every process exit."""
import argparse
import hashlib
import json
import re
import shlex
import statistics
import subprocess
from pathlib import Path

TEST = "store::tests::conversations::appending_one_message_writes_a_constant_row_budget_however_long_the_conversation_is"
SAMPLE = re.compile(r"^warm_append length=(\d+) sample=(\d+) micros=(\d+) writes=(\d+)$", re.M)


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def run(base, candidate, output):
    output.mkdir()
    binaries = {"base": base.resolve(), "candidate": candidate.resolve()}
    hashes = {arm: digest(path) for arm, path in binaries.items()}
    runs = []
    for round_number in range(1, 4):
        order = ["candidate", "base"] if round_number == 2 else ["base", "candidate"]
        for arm in order:
            assert digest(binaries[arm]) == hashes[arm], "binary changed before sampling"
            command = ["nice", "-n", "10", str(binaries[arm]), TEST, "--exact", "--nocapture", "--test-threads=1"]
            log = output / f"round-{round_number}-{arm}.log"
            with log.open("w") as stream:
                result = subprocess.run(command, stdout=stream, stderr=subprocess.STDOUT, check=False)
            record = {"round": round_number, "arm": arm, "command": shlex.join(command),
                      "exit_code": result.returncode, "binary_sha256": hashes[arm], "log": log.name}
            runs.append(record)
            (output / "runs.json").write_text(json.dumps(runs, indent=2) + "\n")
            if result.returncode:
                raise SystemExit(result.returncode)
            assert digest(binaries[arm]) == hashes[arm], "binary changed during sampling"
    return summarize(output, runs)


def summarize(output, runs):
    values = {}
    for run_record in runs:
        samples = SAMPLE.findall((output / run_record["log"]).read_text())
        assert len(samples) == 87, "expected 29 warmed samples at each history length"
        for length in (1, 60, 4096):
            selected = [(int(micros), int(writes)) for size, _, micros, writes in samples if int(size) == length]
            assert len(selected) == 29
            budget = 44 if run_record["arm"] == "base" else 37
            assert all(writes == budget for _, writes in selected), "unexpected append row budget"
            values[(run_record["arm"], length, run_record["round"])] = [micros for micros, _ in selected]
    rows = []
    for length in (1, 60, 4096):
        row = {"starting_history": length, "samples_per_arm": 87, "before_rows": 44, "after_rows": 37}
        for arm in ("base", "candidate"):
            medians = [statistics.median(values[(arm, length, round_number)]) for round_number in range(1, 4)]
            row[f"{arm}_round_medians_us"] = medians
            row[f"{arm}_median_us"] = statistics.median(medians)
        rows.append(row)
    summary = {"clock": "warm in-process elapsed microseconds; median of three process medians",
               "protocol": "three untimed warm-ups then 29 samples; history grows identically by 32 in each process",
               "source": "coalescing-only baseline uses exact e1c4b21f journal/device/retention code; both arms use final schema and fixture",
               "rows": rows}
    (output / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    lines = ["| Starting history | Samples/arm | Writes before→after | Warm median µs before→after |",
             "| --- | ---: | ---: | ---: |"]
    lines.extend(f"| {row['starting_history']} | 87 | 44→37 | {row['base_median_us']}→{row['candidate_median_us']} |" for row in rows)
    (output / "comparison.md").write_text("\n".join(lines) + "\n")
    return summary


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", required=True, type=Path)
    parser.add_argument("--candidate", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    print(json.dumps(run(args.base, args.candidate, args.output), indent=2))
