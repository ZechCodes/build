#!/usr/bin/env python3
"""Compare #894's matched logical-RPC / actual fake-IDB harness records."""
import argparse
import csv
import json
from pathlib import Path


def read(path):
    records = [json.loads(line) for line in path.read_text().splitlines() if line]
    indexed = {(row["peerVersion"], row["label"]): row for row in records}
    if len(indexed) != len(records):
        raise ValueError(f"duplicate profile rows in {path}")
    for row in records:
        if row["rpcCount"] <= 0 or row["cache"]["idbCallbacks"] <= 0:
            raise ValueError(f"profile lacks RPC/cache work: {row['label']}")
        if row["publication"] is None:
            raise ValueError(f"profile lacks row-publication witness: {row['label']}")
    return indexed


def compare(before, after):
    if before.keys() != after.keys():
        raise ValueError("before/after profile sets differ")
    for key, old in before.items():
        new = after[key]
        yield {
            "peer": key[0], "profile": key[1],
            "response_bytes_before": old["responseBytes"], "response_bytes_after": new["responseBytes"],
            "request_bytes_before": old["requestBytes"], "request_bytes_after": new["requestBytes"],
            "rpcs_before": old["rpcCount"], "rpcs_after": new["rpcCount"],
            "logical_ms_before": old["logicalMs"], "logical_ms_after": new["logicalMs"],
            "idb_callbacks_before": old["cache"]["idbCallbacks"], "idb_callbacks_after": new["cache"]["idbCallbacks"],
            "cache_notifications_before": old["cache"]["cacheNotifications"], "cache_notifications_after": new["cache"]["cacheNotifications"],
            "row_publications_before": old["cache"]["rowPublications"], "row_publications_after": new["cache"]["rowPublications"],
            "cache_wall_ms_before": old["cache"]["workWallMs"], "cache_wall_ms_after": new["cache"]["workWallMs"],
            "publication_ms_before": old["publication"]["logicalMs"], "publication_ms_after": new["publication"]["logicalMs"],
            "publication_callbacks_before": old["publication"]["idbCallbacks"], "publication_callbacks_after": new["publication"]["idbCallbacks"],
            "publication_wall_ms_before": old["publication"]["wallMs"], "publication_wall_ms_after": new["publication"]["wallMs"],
        }


def markdown(rows):
    lines = ["| Peer | Profile/start | Response bytes | RPCs | Logical ms | IDB callbacks | Cache wall ms | Row publication ms |",
             "| --- | --- | ---: | ---: | ---: | ---: | ---: | ---: |"]
    names = ["response_bytes", "rpcs", "logical_ms", "idb_callbacks", "cache_wall_ms", "publication_ms"]
    for row in rows:
        fields = []
        for name in names:
            old, new = row[f"{name}_before"], row[f"{name}_after"]
            fields.append(f"{old:.3f} → {new:.3f}" if isinstance(old, float) else f"{old} → {new}")
        lines.append("| " + " | ".join([row["peer"], row["profile"], *fields]) + " |")
    return "\n".join(lines) + "\n"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("before", type=Path)
    parser.add_argument("after", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    rows = list(compare(read(args.before), read(args.after)))
    args.output.mkdir(parents=True, exist_ok=True)
    with (args.output / "cutover.csv").open("w", newline="") as out:
        writer = csv.DictWriter(out, fieldnames=list(rows[0]), lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)
    (args.output / "cutover.md").write_text(markdown(rows))
    # Logical quantities and deterministic cache work are comparable here.
    # Wall timings are retained observations, not a production-latency gate.
    regressions = [row["profile"] for row in rows if row["profile"].startswith("small:") and any(
        row[f"{name}_after"] > row[f"{name}_before"]
        for name in ["response_bytes", "rpcs", "logical_ms", "idb_callbacks"])]
    print(json.dumps({"rows": len(rows), "small_logical_or_cache_regressions": regressions}))
    return bool(regressions)


if __name__ == "__main__":
    raise SystemExit(main())
