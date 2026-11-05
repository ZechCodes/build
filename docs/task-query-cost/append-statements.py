#!/usr/bin/python3
"""Validate and compare the matched #894 per-append SQLite execution maps."""
import argparse
import hashlib
import json
from pathlib import Path
import re

LENGTHS = {1, 60, 4096}
SQL_ROW = re.compile(r"append_sql length=(\d+) connection=(\w+) executions=(\d+) selects=(\d+) statements=(\{.*\})")
WRITE_ROW = re.compile(r"\bappend length=(\d+) micros=\d+ writes=(\d+) tables=(\{.*\})")


def read_log(path):
    sql, writes = {}, {}
    for line in path.read_text().splitlines():
        if match := SQL_ROW.search(line):
            length, connection, total, selects, raw = match.groups()
            length, total, selects = int(length), int(total), int(selects)
            if length in sql:
                raise ValueError(f"duplicate SQL observation for history {length}: {path}")
            statements = json.loads(raw)
            if connection != "writer" or not all(type(count) is int and count > 0 for count in statements.values()):
                raise ValueError(f"invalid writer statement map: {path}")
            if sum(statements.values()) != total:
                raise ValueError(f"execution total does not match complete SQL map: {path}")
            actual_selects = sum(count for statement, count in statements.items() if statement.lstrip().upper().startswith("SELECT"))
            if actual_selects != selects:
                raise ValueError(f"SELECT total does not match complete SQL map: {path}")
            sql[length] = {"connection": connection, "executions": total, "selects": selects, "statements": statements}
        if match := WRITE_ROW.search(line):
            length, count, raw = match.groups()
            length, count = int(length), int(count)
            if length in writes:
                raise ValueError(f"duplicate write observation for history {length}: {path}")
            tables = json.loads(raw)
            if sum(tables.values()) != count:
                raise ValueError(f"write total does not match complete table map: {path}")
            writes[length] = {"writes": count, "tables": tables}
    if sql.keys() != LENGTHS or writes.keys() != LENGTHS:
        raise ValueError(f"expected exactly histories 1,60,4096: {path}")
    first = sql[1]
    if any(row != first for row in sql.values()):
        raise ValueError(f"SQL executions/SELECTs/full maps differ across history lengths: {path}")
    if any(row != writes[1] for row in writes.values()):
        raise ValueError(f"write counts/table maps differ across history lengths: {path}")
    return {"log": str(path), "log_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
            "histories": {str(length): {**sql[length], **writes[length]} for length in sorted(LENGTHS)}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--baseline", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    baseline, candidate = read_log(args.baseline), read_log(args.candidate)
    old, new = baseline["histories"]["1"], candidate["histories"]["1"]
    for metric in ["executions", "selects", "writes"]:
        if new[metric] > old[metric]:
            raise ValueError(f"candidate {metric} increased: {old[metric]} -> {new[metric]}")
    summary = {"histories": sorted(LENGTHS), "constant_full_maps_within_each_arm": True,
               "before_executions": old["executions"], "after_executions": new["executions"],
               "before_selects": old["selects"], "after_selects": new["selects"],
               "before_writes": old["writes"], "after_writes": new["writes"],
               "baseline": baseline, "candidate": candidate}
    args.output.mkdir(parents=True, exist_ok=True)
    (args.output / "comparison.json").write_text(json.dumps(summary, indent=2) + "\n")
    lines = ["| History | Executions before → after | SELECT starts before → after | Changed rows before → after |",
             "| ---: | ---: | ---: | ---: |"]
    for length in sorted(LENGTHS):
        old, new = baseline["histories"][str(length)], candidate["histories"][str(length)]
        lines.append(f"| {length} | {old['executions']} → {new['executions']} | "
                     f"{old['selects']} → {new['selects']} | {old['writes']} → {new['writes']} |")
    (args.output / "comparison.md").write_text("\n".join(lines) + "\n")
    print(json.dumps({key: value for key, value in summary.items() if key not in {"baseline", "candidate"}}))


if __name__ == "__main__":
    main()
