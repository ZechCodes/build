"""Exact-fact fixture producer costs; not matched Build/Rust producer costs."""
import json
from measure import storage, task
from prototype import SelectionDB


def run(label_count):
    db = SelectionDB(soft_ms=float("inf"))
    labels = [f"label-{i:06}" for i in range(label_count)]
    record = task(1)
    rows = []
    for mode in ["first", "replace_same"]:
        steps = 0
        statements = 0

        def progress():
            nonlocal steps
            steps += 1
            return 0

        def trace(_sql):
            nonlocal statements
            statements += 1

        db.connection.set_progress_handler(progress, 1)
        db.connection.set_trace_callback(trace)
        db.put_task(record, labels)
        db.connection.set_progress_handler(None, 0)
        db.connection.set_trace_callback(None)
        rows.append(dict(mode=mode, sql_vm_steps=steps, statements=statements))
    assert db.connection.execute("SELECT count(*) FROM label").fetchone()[0] == label_count
    result = dict(labels=label_count, operations=rows, storage=storage(db),
                  caveat="Fixture put_task replaces exact labels with whole-task DELETE/INSERT; outside selection quantum. This is a deliberately unbounded producer total, not authorized production code or regression evidence.")
    db.connection.close()
    return result


if __name__ == "__main__":
    for count in [0, 3, 503, 10000]:
        print(json.dumps(run(count), sort_keys=True))
