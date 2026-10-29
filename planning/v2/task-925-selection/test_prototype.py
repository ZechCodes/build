"""Independent exhaustive oracle and deterministic bounds for the research model."""
import json
import sqlite3
import tempfile
import unittest

from prototype import BudgetExceeded, Pending, SelectionDB, Unsupported


def header(number, **values):
    result = dict(id=f"t{number:05}", project="p", number=number, state="open",
                  status="backlog", priority=0, holder_kind="none", holder_id="",
                  workspace="w", attention=False, readiness=False, blocked=False, supported=True)
    result.update(values)
    return result


def oracle(tasks, labels, roster, wanted_labels, workspaces, statuses, coordinator=None):
    """Intentionally no SQL and no calls to implementation classification helpers."""
    selected = []
    for task in tasks:
        if task["project"] != "p":
            continue
        if workspaces is not None and (task["holder_kind"] != "agent" or task["holder_id"] not in workspaces):
            continue
        if wanted_labels and not (set(labels.get(task["id"], [])) & wanted_labels):
            continue
        selected.append(task)
    rows = []
    counts = {}
    for task in selected:
        unfinished = task["state"] != "closed" and task["status"] != "done"
        holder = task["holder_id"] if task["holder_kind"] == "agent" else (
            coordinator if task["holder_kind"] == "project_agent" else None)
        group = 1 if unfinished and holder in roster else (2 if unfinished and task["attention"] else 3)
        rows.append((task["id"], group, task["number"], task["status"], task["priority"], task["blocked"]))
        counts[task["status"]] = counts.get(task["status"], 0) + 1
    return dict(rows=sorted(rows, key=lambda row: (row[1], -row[2], row[0])),
                counts=counts, total=len(rows), maximum=max((t["number"] for t in selected), default=0),
                readiness=any(t["status"] == "backlog" and t["readiness"] for t in selected),
                groups={group: sum(row[1] == group for row in rows) for group in (1, 2, 3)})


class SelectionTests(unittest.TestCase):
    def setUp(self):
        self.db = SelectionDB(soft_ms=float("inf"))

    def tearDown(self):
        self.db.connection.close()

    def stage(self, kind, values):
        token = self.db.create_set(kind, len(set(values)))
        for offset in range(0, len(values), 100):
            self.db.stage_set(token, values[offset:offset + 100])
        self.db.seal_set(token)
        return token

    def start(self, roster=(), labels=(), workspaces=None, statuses=("backlog", "done"), **values):
        return self.db.start("p", roster=self.stage("roster", list(roster)),
                             labels=self.stage("labels", list(labels)),
                             workspaces=None if workspaces is None else self.stage("workspaces", list(workspaces)),
                             statuses=self.stage("statuses", list(statuses)), **values)

    def finish(self, build):
        for _ in range(20000):
            state = self.db.quantum(build)
            if state != "pending":
                self.assertEqual(state, "sealed")
                return self.db.summary(build)
        self.fail("build did not converge")

    def test_oracle_overlaps_large_roster_status_vocabulary_and_strays(self):
        roster = {f"a{i}" for i in range(230)}
        statuses = {f"s{i}" for i in range(601)} | {"backlog", "done"}
        tasks = [header(i, status=f"s{i % 601}", holder_kind="agent", holder_id=f"a{i % 240}",
                        attention=i % 3 == 0, priority=i % 5, readiness=i % 7 == 0,
                        blocked=i % 11 == 0, workspace="other" if i % 9 == 0 else "w",
                        state="closed" if i % 19 == 0 else "open") for i in range(1, 720)]
        tasks += [header(900, status="stray", holder_kind="project_agent", attention=True),
                  header(901, project="other"), header(902, status="done", attention=True)]
        labels = {task["id"]: ["one", "two"] if task["number"] % 2 else ["two", "three"] for task in tasks}
        for task in tasks:
            self.db.put_task(task, labels[task["id"]])
        scope = {f"a{i}" for i in range(240) if i % 9}
        build = self.start(roster=roster, labels=["one", "two"], workspaces=scope, statuses=statuses,
                           coordinator="a229")
        expected = oracle(tasks, labels, roster, {"one", "two"}, scope, statuses, "a229")
        self.assertEqual(self.finish(build), {k: v for k, v in expected.items() if k != "rows"})
        self.assertEqual(self.db.all_rows(build), expected["rows"])
        self.assertLessEqual(max(q["headers"] for q in self.db.evidence), 64)
        self.assertLessEqual(max(q["labels"] for q in self.db.evidence), 64)
        self.assertLessEqual(max(q["vm_upper"] for q in self.db.evidence), 100000)

    def test_large_labels_resume_independent_subcursor(self):
        labels = [f"label-{i:05}" for i in range(503)]
        self.db.put_task(header(1), labels)
        build = self.start(labels=[labels[-1]])
        self.assertEqual(self.db.quantum(build), "pending")
        checkpoint = self.db.checkpoint(build)
        self.assertEqual(checkpoint["cursor"], "")
        self.assertTrue(checkpoint["label_cursor"])
        self.assertEqual(self.finish(build)["total"], 1)
        self.assertGreater(len(self.db.evidence), 7)

    def test_pending_is_not_zero_and_partials_are_unreadable(self):
        self.db.put_task(header(1))
        build = self.start()
        with self.assertRaises(Pending):
            self.db.summary(build)
        with self.assertRaises(Pending):
            self.db.all_rows(build)
        self.assertEqual(self.finish(build)["total"], 1)

    def test_sealed_exact_set_counts_duplicates_and_is_immutable(self):
        token = self.db.create_set("labels", 2)
        self.db.stage_set(token, ["one", "one"])
        with self.assertRaises(Pending):
            self.db.seal_set(token)
        self.db.stage_set(token, ["two"])
        self.db.seal_set(token)
        with self.assertRaises(ValueError):
            self.db.stage_set(token, ["three"])

    def test_input_bounds_refuse_without_truncating(self):
        token = self.db.create_set("labels", 101)
        for values in (["x"] * 101, ["x" * 129]):
            with self.assertRaises(Unsupported):
                self.db.stage_set(token, values)
        for task in (header(1, status="s" * 129), header(2, holder_id="a" * 129),
                     header(3, extra="x" * 3000), header(4, state="unknown")):
            with self.assertRaises(Unsupported):
                self.db.put_task(task)

    def test_global_and_project_epoch_invalidation(self):
        self.db.put_task(header(1))
        global_build = self.start(basis="global")
        project_build = self.start(basis="project")
        self.db.put_task(header(2, project="other"))
        self.assertEqual(self.db.quantum(global_build), "invalid")
        self.assertEqual(self.finish(project_build)["total"], 1)
        stale = self.start(basis="project")
        self.db.put_task(header(3))
        self.assertEqual(self.db.quantum(stale), "invalid")

    def test_mutation_after_last_scan_before_atomic_seal_is_rejected(self):
        self.db.put_task(header(1))
        build = self.start()
        while self.db.checkpoint(build)["phase"] != "finish":
            self.assertEqual(self.db.quantum(build), "pending")
        self.db.put_task(header(2))
        self.assertEqual(self.db.quantum(build), "invalid")
        with self.assertRaises(Pending):
            self.db.summary(build)

    def test_budget_interrupt_rolls_back_checkpoint_output_and_counts(self):
        for i in range(140):
            self.db.put_task(header(i))
        build = self.start()
        before = self.db.checkpoint(build)
        with self.assertRaises(BudgetExceeded):
            self.db.quantum(build, vm_budget=500)
        self.assertEqual(self.db.checkpoint(build), before)
        self.assertEqual(self.db.connection.execute("select count(*) from result").fetchone()[0], 0)
        self.assertEqual(self.finish(build)["total"], 140)

    def test_cancel_and_gc_delete_at_most_64_records_no_cascade(self):
        for i in range(150):
            self.db.put_task(header(i))
        build = self.start()
        self.db.quantum(build)
        self.db.cancel(build)
        with self.assertRaises(Pending):
            self.db.summary(build)
        deleted = []
        while self.db.exists(build):
            deleted.append(self.db.gc(build))
        self.assertTrue(deleted)
        self.assertLessEqual(max(deleted), 64)
        self.assertEqual(self.db.connection.execute("select count(*) from task").fetchone()[0], 150)

    def test_coordinator_pin_absent_and_legacy_resolved_inputs(self):
        self.db.put_task(header(1, holder_kind="project_agent", attention=True))
        self.db.put_task(header(2, holder_kind="agent", holder_id="unknown", attention=True))
        for coordinator, expected in [("pinned", 1), (None, 2), ("absent", 2), ("legacy", 1)]:
            build = self.start(roster=["pinned", "legacy"], coordinator=coordinator)
            self.finish(build)
            self.assertEqual(self.db.all_rows(build)[0][1], expected)

    def test_board_priority_backlog_activation_and_status_stray_max(self):
        for task in [header(1, priority=4), header(2, priority=2, blocked=True),
                     header(3, priority=4, readiness=True), header(99, status="stray")]:
            self.db.put_task(task)
        build = self.start()
        result = self.finish(build)
        self.assertTrue(result["readiness"])
        self.assertEqual(result["maximum"], 99)
        self.assertEqual(result["counts"], {"backlog": 3, "stray": 1})
        self.assertEqual([row[0] for row in self.db.board_rows(build, "backlog")], ["t00002", "t00003", "t00001"])

    def test_empty_selection_seals_exact_zero(self):
        self.db.put_task(header(1))
        build = self.start(workspaces=["missing"])
        self.assertEqual(self.finish(build), dict(total=0, maximum=0, readiness=False, counts={}, groups={1: 0, 2: 0, 3: 0}))

    def test_exact_scope_empty_is_zero_and_requires_actual_agent(self):
        for task in [header(1, holder_kind="agent", holder_id="a", workspace="wrong"),
                     header(2, holder_kind="project_agent"), header(3, workspace="a")]:
            self.db.put_task(task)
        self.assertEqual(self.finish(self.start(workspaces=[]))["total"], 0)
        scoped = self.start(workspaces=["a"], coordinator="a", roster=["a"])
        self.finish(scoped)
        self.assertEqual([row[0] for row in self.db.all_rows(scoped)], ["t00001"])

    def test_cancelled_open_is_unfinished_and_non_backlog_readiness_does_not_activate(self):
        self.db.put_task(header(1, status="cancelled", holder_kind="agent", holder_id="a", readiness=True))
        self.db.put_task(header(2, status="in_review", priority=0))
        self.db.put_task(header(3, status="in_review", priority=4))
        self.db.put_task(header(4, status="in_review", priority=0))
        build = self.start(roster=["a"])
        self.assertFalse(self.finish(build)["readiness"])
        self.assertEqual(self.db.all_rows(build)[0][1], 1)
        self.assertEqual([row[0] for row in self.db.board_rows(build, "in_review")], ["t00004", "t00003", "t00002"])

    def test_off_page_invalid_source_projection_refuses_instead_of_exact_zero(self):
        for source_failure in ("status over 128 bytes", "actor over 128 bytes", "labels over 20 or 40 bytes"):
            for i in range(1, 141):
                self.db.put_task(header(i))
            # Models today's off-domain compact source marker, even when its
            # lossy status/actor/labels fields would fail the requested filter.
            self.db.put_task(header(99999, supported=False))
            build = self.start(labels=["not-in-lossy-empty-projection"])
            self.assertEqual(self.db.quantum(build), "pending", source_failure)
            for _ in range(20):
                outcome = self.db.quantum(build)
                if outcome != "pending":
                    break
            self.assertEqual(outcome, "unsupported", source_failure)
            with self.assertRaises(Pending):
                self.db.summary(build)

    def test_configured_column_order_zero_counts_max_and_large_status_filter(self):
        statuses = [f"s{i:04}" for i in reversed(range(601))]
        self.db.put_task(header(1, status="s0600"))
        self.db.put_task(header(99, status="stray"))
        selected = self.stage("status_filter", [f"s{i:04}" for i in range(601)])
        build = self.start(statuses=statuses, status_filter=selected)
        self.assertEqual(self.finish(build)["total"], 1)
        columns, after = [], -1
        while page := self.db.column_page(build, after):
            columns.extend(page)
            after = page[-1][0]
        self.assertEqual([row[1] for row in columns], statuses)
        self.assertEqual(columns[0], (0, "s0600", 1, 1))
        self.assertEqual(columns[-1], (600, "s0000", 0, 0))

    def test_abandoned_input_gc_is_incremental(self):
        token = self.db.create_set("labels", 230)
        for offset in range(0, 230, 100):
            self.db.stage_set(token, [f"l{i}" for i in range(offset, min(offset + 100, 230))])
        self.db.cancel_set(token)
        deleted = []
        while self.db.connection.execute("SELECT 1 FROM input_set WHERE id=?", (token,)).fetchone():
            deleted.append(self.db.gc_set(token))
        self.assertEqual(sum(deleted), 231)
        self.assertLessEqual(max(deleted), 64)

    def test_restart_resumes_durable_label_subcursor(self):
        with tempfile.TemporaryDirectory() as directory:
            self.db.connection.close()
            path = directory + "/research.sqlite"
            self.db = SelectionDB(path, soft_ms=float("inf"))
            self.db.put_task(header(1), [f"l{i:04}" for i in range(200)])
            build = self.start(labels=["l0199"])
            self.db.quantum(build)
            before = self.db.checkpoint(build)
            self.db.connection.close()
            self.db = SelectionDB(path, soft_ms=float("inf"))
            self.assertEqual(self.db.checkpoint(build), before)
            self.assertEqual(self.finish(build)["total"], 1)

    def test_board_removes_status_filter_and_stray_backlog_stays_number_order(self):
        self.db.put_task(header(1, readiness=True, priority=4, blocked=True))
        self.db.put_task(header(2, priority=0))
        chosen = self.stage("status_filter", ["done"])
        build = self.start(statuses=["done"], status_filter=chosen, view="board")
        self.assertEqual(self.finish(build)["total"], 2)
        self.assertEqual([row[0] for row in self.db.board_rows(build, "backlog")], ["t00002", "t00001"])

    def test_late_page_keyset_seek_is_bounded_with_10000_same_rank(self):
        for number in range(10000):
            self.db.put_task(header(number))
        build = self.start()
        self.finish(build)
        first = self.db.list_page(build)
        first_steps = self.db.operations[-1]["vm_upper"]
        late = self.db.list_page(build, (3, 100, "t00100"))
        late_steps = self.db.operations[-1]["vm_upper"]
        self.assertEqual(len(first), 64)
        self.assertEqual(len(late), 64)
        self.assertLessEqual(late_steps, first_steps * 2 + 500)
        plan = self.db.connection.execute("""EXPLAIN QUERY PLAN SELECT task FROM result
          WHERE owner=? AND (rank,number_key,task)>(?,?,?) ORDER BY rank,number_key,task LIMIT 64""",
          (build, 3, -100, "t00100")).fetchone()[3]
        self.assertIn("number_key", plan)

    def test_shared_sealed_input_lifetime_and_bounded_release_gc(self):
        token = self.stage("roster", [f"a{i}" for i in range(230)])
        other = dict(labels=self.stage("labels", []), workspaces=None, statuses=self.stage("statuses", ["backlog"]))
        builds = [self.db.start("p", roster=token, **other) for _ in range(2)]
        with self.assertRaises(ValueError):
            self.db.release_set(token)
        for build in builds:
            self.db.cancel(build)
            while self.db.exists(build):
                self.db.gc(build)
        self.db.release_set(token)
        deleted = []
        while self.db.connection.execute("SELECT 1 FROM input_set WHERE id=?", (token,)).fetchone():
            deleted.append(self.db.gc_set(token))
        self.assertEqual(sum(deleted), 231)
        self.assertLessEqual(max(deleted), 64)

    def test_stride_one_vm_counter_covers_many_distinct_short_statements(self):
        with self.db._guard() as metrics:
            for value in range(1000):
                self.db.connection.execute(f"SELECT {value}").fetchone()
        self.assertGreaterEqual(metrics["vm_upper"], 5000)


if __name__ == "__main__":
    unittest.main()
