"""Exploratory research runner; deterministic counters, no production SLA claim.

Example: nice -n 10 python planning/v2/task-925-selection/measure.py \
  --rows 25 1000 10000 100000 --basis global project \
  --schedule quiet normal sustained unrelated --jobs 1 --max-quanta 1200
Output is JSON lines. Fixture ingestion is outside the build service envelope.
"""
import argparse
import json
import time

from prototype import BudgetExceeded, SelectionDB


def task(number, project="p"):
    return dict(id=f"t{number:08}", project=project, number=number, state="open",
                status="backlog" if number % 7 == 0 else f"s{number % 1000:04}",
                priority=number % 5, holder_kind="agent", holder_id=f"a{number % 1001:04}",
                workspace="display-only", attention=number % 3 == 0,
                readiness=number % 11 == 0, blocked=number % 13 == 0, supported=True)


def stage(db, kind, values):
    token = db.create_set(kind, len(values))
    for offset in range(0, len(values), 100):
        db.stage_set(token, values[offset:offset + 100])
    db.seal_set(token)
    return token


def storage(db):
    connection = db.connection
    logical = {}
    for table, expression in {
        "task": "length(cast(id as blob))+length(cast(project as blob))+length(cast(header as blob))",
        "label": "length(cast(task as blob))+length(cast(value as blob))",
        "member": "length(owner)+length(cast(value as blob))+8",
        "result": "length(owner)+length(task)+length(cast(status as blob))+56",
        "status_count": "length(owner)+length(cast(status as blob))+16",
    }.items():
        # Both identifiers/expressions come solely from the fixed diagnostic map above.
        # nosemgrep: python.lang.security.audit.formatted-sql-query.formatted-sql-query, python.sqlalchemy.security.sqlalchemy-execute-raw-query.sqlalchemy-execute-raw-query
        row = connection.execute(f"SELECT count(*),coalesce(sum({expression}),0) FROM {table}").fetchone()
        logical[table] = dict(rows=row[0], payload_bytes=row[1])
    pages = connection.execute("PRAGMA page_count").fetchone()[0]
    page_size = connection.execute("PRAGMA page_size").fetchone()[0]
    try:
        physical = {row[0]: row[1] for row in connection.execute("SELECT name,sum(pgsize) FROM dbstat GROUP BY name")}
    except Exception:
        physical = None
    return dict(logical=logical, sqlite_allocated_bytes=pages * page_size, sqlite_dbstat_bytes=physical)


def counters(operations):
    names = ("headers", "labels", "header_bytes", "label_bytes", "probes", "statements", "vm_upper")
    return dict(calls=len(operations), totals={name: sum(op[name] for op in operations) for name in names},
                maxima={name: max((op[name] for op in operations), default=0) for name in names})


def run(args, size, basis, schedule):
    began = time.monotonic()
    db = SelectionDB(soft_ms=args.soft_ms)
    for number in range(size):
        labels = ["common", "overlap-a", "overlap-b"] if args.overlap else []
        if args.long_labels and number == 0:
            labels = [f"label-{value:05}" for value in range(503)]
        db.put_task(task(number), labels)
    fixture_seconds = time.monotonic() - began
    started = time.monotonic()
    inputs = dict(roster=stage(db, "roster", [f"a{i:04}" for i in range(1001)]),
                  statuses=stage(db, "statuses", ["backlog"] + [f"s{i:04}" for i in range(1000)]),
                  labels=stage(db, "labels", ["label-00502"] if args.long_labels else (["common", "overlap-a"] if args.overlap else [])),
                  workspaces=None, basis=basis, coordinator="a0000")
    setup_operations = list(db.operations)
    before = storage(db)
    jobs = []
    charged = len(setup_operations)
    for _ in range(args.jobs):
        jobs.append(dict(build=db.start("p", **inputs), attempts=1, quanta=0, state="pending", failures=0))
        charged += 1
    build_quanta = mutations = 0
    normal_mutated = False
    while charged < args.max_quanta and any(job["state"] == "pending" for job in jobs):
        for job in jobs:
            if charged >= args.max_quanta:
                break
            if job["state"] != "pending":
                continue
            mutate = build_quanta > 0 and build_quanta % 4 == 0
            if schedule == "quiet" or (schedule == "normal" and normal_mutated):
                mutate = False
            if mutate:
                project = "other" if schedule == "unrelated" else "p"
                changed = task(0 if project == "p" else size + 1, project)
                changed["attention"] = bool(mutations % 2)
                labels = ["common", "overlap-a", "overlap-b"] if args.overlap and project == "p" else []
                if args.long_labels and project == "p":
                    labels = [f"label-{value:05}" for value in range(503)]
                db.put_task(changed, labels)
                mutations += 1
                normal_mutated = True
            charged += 1
            build_quanta += 1
            job["quanta"] += 1
            try:
                outcome = db.quantum(job["build"])
            except BudgetExceeded:
                job["failures"] += 1
                continue
            if outcome == "invalid" and job["attempts"] < args.attempts and charged < args.max_quanta:
                job["build"] = db.start("p", **inputs)
                job["attempts"] += 1
                charged += 1
            elif outcome != "pending":
                job["state"] = outcome
    worker_operations = list(db.operations[len(setup_operations):])
    after = storage(db)
    for job in jobs:
        if job["state"] == "pending":
            job["state"] = "unavailable_envelope"
        state = db.checkpoint(job["build"])
        job["selected_total"] = state["total"] if job["state"] == "sealed" else None
        del job["build"]
    # Exercise bounded cleanup of every build, including abandoned attempts.
    build_ids = [row[0] for row in db.connection.execute("SELECT id FROM build")]
    cleanup_start = len(db.operations)
    gc_records, gc_calls, gc_max = 0, 0, 0
    for token in build_ids:
        db.cancel(token)
        while db.exists(token):
            removed = db.gc(token)
            gc_records += removed
            gc_calls += 1
            gc_max = max(gc_max, removed)
    for token in (inputs["roster"], inputs["statuses"], inputs["labels"]):
        db.release_set(token)
        while db.connection.execute("SELECT 1 FROM input_set WHERE id=?", (token,)).fetchone():
            removed = db.gc_set(token)
            gc_records += removed
            gc_calls += 1
            gc_max = max(gc_max, removed)
    cleanup_operations = db.operations[cleanup_start:]
    result = dict(rows=size, basis=basis, schedule=schedule, jobs=args.jobs,
                  overlap=args.overlap, long_labels=args.long_labels,
                  attempts_limit=args.attempts, charged_service_quanta=charged,
                  build_quanta=build_quanta, max_quanta=args.max_quanta,
                  global_service_hz=10, envelope_seconds=args.max_quanta / 10,
                  mutations=mutations, results=jobs, setup=counters(setup_operations),
                  worker=counters(worker_operations), cleanup=counters(cleanup_operations),
                  gc_records=gc_records, gc_calls=gc_calls, gc_max_records=gc_max,
                  storage_before=before, storage_after=after,
                  storage_allocation_delta=after["sqlite_allocated_bytes"] - before["sqlite_allocated_bytes"],
                  fixture_seconds_exploratory=fixture_seconds,
                  elapsed_seconds_exploratory=time.monotonic() - started,
                  timing_context="ambient host; Python stride-one VM callbacks; not production latency")
    db.connection.close()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rows", type=int, nargs="+", default=[25, 1000, 10000])
    parser.add_argument("--basis", choices=["global", "project"], nargs="+", default=["global", "project"])
    parser.add_argument("--schedule", choices=["quiet", "normal", "sustained", "unrelated"], nargs="+", default=["quiet"])
    parser.add_argument("--jobs", type=int, default=1)
    parser.add_argument("--attempts", type=int, default=3)
    parser.add_argument("--max-quanta", type=int, default=1200)
    parser.add_argument("--soft-ms", type=float, default=20)
    parser.add_argument("--overlap", action="store_true")
    parser.add_argument("--long-labels", action="store_true")
    args = parser.parse_args()
    if min(args.rows) < 0 or min(args.jobs, args.attempts, args.max_quanta) < 1:
        parser.error("nonnegative row counts; positive jobs, attempts and quantum limit required")
    for size in args.rows:
        for basis in args.basis:
            for schedule in args.schedule:
                print(json.dumps(run(args, size, basis, schedule), sort_keys=True), flush=True)


if __name__ == "__main__":
    main()
