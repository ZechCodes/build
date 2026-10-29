"""Isolated SQLite design experiment; not a Build runtime implementation.

The source is a *new exact normalized research schema*, not today's bounded
summary projection. Source ingestion and fixture generation are outside build
quanta. Every build/input lifecycle call uses a transaction and VM guard.
Consumer conveniences assemble pages in Python; only their individual pages
are bounded. No claim of hard wall-time or production latency follows.
"""
from contextlib import contextmanager
import json
import sqlite3
import time
import uuid


class Pending(Exception):
    """No exact readable answer has been published."""


class Unsupported(ValueError):
    """The exact input is outside this experiment's explicit domain."""


class BudgetExceeded(Exception):
    """The whole quantum rolled back, including its durable checkpoint."""


SCHEMA = """
CREATE TABLE epoch(scope TEXT PRIMARY KEY, value INTEGER NOT NULL);
INSERT INTO epoch VALUES ('*', 0);
CREATE TABLE task(id TEXT PRIMARY KEY, project TEXT NOT NULL, header TEXT NOT NULL);
CREATE INDEX task_project ON task(project,id);
CREATE TABLE label(task TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(task,value));
CREATE TABLE input_set(id TEXT PRIMARY KEY, kind TEXT NOT NULL, expected INTEGER NOT NULL,
  count INTEGER NOT NULL DEFAULT 0, state TEXT NOT NULL DEFAULT 'staging', refs INTEGER NOT NULL DEFAULT 0);
CREATE TABLE member(owner TEXT NOT NULL, value TEXT NOT NULL, position INTEGER NOT NULL, PRIMARY KEY(owner,value));
CREATE UNIQUE INDEX member_position ON member(owner,position);
CREATE TABLE build(id TEXT PRIMARY KEY, project TEXT NOT NULL, basis TEXT NOT NULL,
  revision INTEGER NOT NULL, view TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending', phase TEXT NOT NULL DEFAULT 'scan',
  roster TEXT NOT NULL, labels TEXT NOT NULL, workspaces TEXT, statuses TEXT NOT NULL,
  status_filter TEXT, coordinator TEXT, cursor TEXT NOT NULL DEFAULT '', pending TEXT,
  label_cursor TEXT NOT NULL DEFAULT '', matched INTEGER NOT NULL DEFAULT 0,
  total INTEGER NOT NULL DEFAULT 0, maximum INTEGER NOT NULL DEFAULT 0,
  readiness INTEGER NOT NULL DEFAULT 0, g1 INTEGER NOT NULL DEFAULT 0,
  g2 INTEGER NOT NULL DEFAULT 0, g3 INTEGER NOT NULL DEFAULT 0);
CREATE TABLE result(owner TEXT NOT NULL, task TEXT NOT NULL, rank INTEGER NOT NULL,
  number INTEGER NOT NULL, status TEXT NOT NULL, priority INTEGER NOT NULL, blocked INTEGER NOT NULL,
  number_key INTEGER NOT NULL, priority_key INTEGER NOT NULL, blocked_key INTEGER NOT NULL,
  PRIMARY KEY(owner,task));
CREATE INDEX result_list ON result(owner,rank,number_key,task);
CREATE INDEX result_board ON result(owner,status,number_key,task);
CREATE INDEX result_backlog ON result(owner,status,blocked_key,priority_key,number_key,task);
CREATE TABLE status_count(owner TEXT NOT NULL, status TEXT NOT NULL, count INTEGER NOT NULL, maximum INTEGER NOT NULL,
  PRIMARY KEY(owner,status));
"""


def encoded(value):
    return json.dumps(value, ensure_ascii=False, separators=(",", ":"))


def exact_id(value):
    if not isinstance(value, str) or not 0 < len(value.encode()) <= 128:
        raise Unsupported("exact identifiers must contain 1..128 UTF-8 bytes")
    return value


class SelectionDB:
    HEADER_LIMIT = 64
    LABEL_LIMIT = 64
    SET_LIMIT = 100
    SET_BYTES = 65536
    ROW_BYTES = 2048
    GC_LIMIT = 64

    def __init__(self, path=":memory:", *, soft_ms=20):
        self.connection = sqlite3.connect(path, isolation_level=None)
        self.connection.row_factory = sqlite3.Row
        if self.connection.execute("SELECT 1 FROM sqlite_master WHERE type='table' AND name='build'").fetchone() is None:
            self.connection.executescript(SCHEMA)
        self.evidence = []
        self.operations = []
        self.metrics = None
        self.soft_ms = soft_ms

    @contextmanager
    def _guard(self, vm_budget=100000, soft_ms=None, operation="lifecycle"):
        if vm_budget < 1:
            raise ValueError("positive VM budget required")
        # Stride one is deliberate: SQLite progress residuals are per prepared
        # statement; callbacks*100+99 is NOT a quantum-wide upper bound when
        # many statements are executed. Python callback overhead is measured.
        stride = 1
        soft_ms = self.soft_ms if soft_ms is None else soft_ms
        metrics = dict(operation=operation, headers=0, labels=0, header_bytes=0, label_bytes=0,
                       probes=0, statements=0, vm_upper=0)
        started = time.monotonic()
        instructions = 0

        def progress():
            nonlocal instructions
            instructions += stride
            return instructions >= vm_budget or (time.monotonic() - started) * 1000 >= soft_ms

        self.metrics = metrics
        self.connection.set_progress_handler(progress, stride)
        self.connection.set_trace_callback(lambda _sql: metrics.__setitem__("statements", metrics["statements"] + 1))
        try:
            self.connection.execute("BEGIN IMMEDIATE")
            yield metrics
            self.connection.execute("COMMIT")
        except sqlite3.OperationalError as error:
            self.connection.set_progress_handler(None, 0)
            self.connection.set_trace_callback(None)
            if self.connection.in_transaction:
                self.connection.execute("ROLLBACK")
            if "interrupted" in str(error):
                raise BudgetExceeded("quantum VM/soft-time budget; no partial progress") from error
            raise
        except BaseException:
            self.connection.set_progress_handler(None, 0)
            self.connection.set_trace_callback(None)
            if self.connection.in_transaction:
                self.connection.execute("ROLLBACK")
            raise
        finally:
            self.connection.set_progress_handler(None, 0)
            self.connection.set_trace_callback(None)
            metrics["vm_upper"] = instructions + stride - 1
            self.operations.append(dict(metrics))
            self.metrics = None

    def _one(self, sql, args=()):
        if self.metrics is not None:
            self.metrics["probes"] += 1
            if self.metrics["probes"] > 1024:
                raise BudgetExceeded("indexed point-probe budget")
        return self.connection.execute(sql, args).fetchone()

    def create_set(self, kind, expected):
        if kind not in {"roster", "labels", "workspaces", "statuses", "status_filter"}:
            raise Unsupported("unknown exact-set kind")
        if not isinstance(expected, int) or not 0 <= expected < 2**63:
            raise Unsupported("invalid expected cardinality")
        token = uuid.uuid4().hex
        with self._guard(operation="create_set"):
            self.connection.execute("INSERT INTO input_set(id,kind,expected) VALUES (?,?,?)", (token, kind, expected))
        return token

    def stage_set(self, token, values):
        if not isinstance(values, (list, tuple)) or len(values) > self.SET_LIMIT:
            raise Unsupported("set chunk exceeds 100 values")
        if len(encoded(values).encode()) > self.SET_BYTES:
            raise Unsupported("set chunk exceeds 64 KiB")
        for value in values:
            exact_id(value)
        with self._guard(operation="stage_set"):
            state = self._one("SELECT * FROM input_set WHERE id=?", (token,))
            if state is None or state["state"] != "staging":
                raise ValueError("set is not mutable staging input")
            added = 0
            for value in values:
                inserted = self.connection.execute("INSERT OR IGNORE INTO member VALUES (?,?,?)", (token, value, state["count"] + added)).rowcount
                if state["kind"] == "statuses" and not inserted:
                    raise Unsupported("duplicate configured columns are outside this prototype")
                added += inserted
            if state["count"] + added > state["expected"]:
                raise Unsupported("more unique values than declared")
            self.connection.execute("UPDATE input_set SET count=count+? WHERE id=?", (added, token))

    def seal_set(self, token):
        """O(1) indexed metadata check; never COUNT or sort staged members."""
        with self._guard(operation="seal_set"):
            state = self._one("SELECT * FROM input_set WHERE id=?", (token,))
            if state is None or state["state"] != "staging":
                raise ValueError("set cannot be sealed")
            if state["count"] != state["expected"]:
                raise Pending("expected exact input has not all arrived")
            self.connection.execute("UPDATE input_set SET state='sealed' WHERE id=?", (token,))

    def _member(self, token, value):
        return self._one("SELECT 1 FROM member WHERE owner=? AND value=?", (token, value)) is not None

    def _set(self, token, kind):
        state = self._one("SELECT * FROM input_set WHERE id=?", (token,))
        if state is None or state["state"] != "sealed" or state["kind"] != kind:
            raise Pending("exact sealed input set required")
        return state

    def _validate_task(self, task):
        required = {"id", "project", "number", "state", "status", "priority", "holder_kind", "holder_id",
                    "workspace", "attention", "readiness", "blocked", "supported"}
        if set(task) != required or len(encoded(task).encode()) > self.ROW_BYTES:
            raise Unsupported("header shape or 2 KiB row bound")
        for name in ("id", "project", "status", "workspace"):
            exact_id(task[name])
        if task["holder_id"]:
            exact_id(task["holder_id"])
        if task["state"] not in {"open", "closed"} or task["holder_kind"] not in {"none", "user", "agent", "project_agent"}:
            raise Unsupported("unknown state/assignee variant")
        if task["holder_kind"] == "agent" and not task["holder_id"]:
            raise Unsupported("agent holder needs exact identifier")
        if type(task["number"]) is not int or not 0 <= task["number"] < 2**63:
            raise Unsupported("number outside signed SQLite integer domain")
        if type(task["priority"]) is not int or not 0 <= task["priority"] <= 4:
            raise Unsupported("priority must be normalized unknown/low/medium/high/urgent rank 0..4")
        if any(type(task[name]) is not bool for name in ("attention", "readiness", "blocked", "supported")):
            raise Unsupported("exact Boolean attention/readiness/blocked facts required")

    def put_task(self, task, labels=()):
        """Fixture/source writer, deliberately not a bounded build operation.

        A real source writer must atomically maintain its exact compact facts,
        relation rows and invalidation epoch. Labels are not a summary array.
        """
        self._validate_task(task)
        labels = [exact_id(label) for label in labels]
        with self.connection:
            self.connection.execute("BEGIN IMMEDIATE")
            old = self._one("SELECT project FROM task WHERE id=?", (task["id"],))
            self.connection.execute("INSERT OR REPLACE INTO task VALUES (?,?,?)", (task["id"], task["project"], encoded(task)))
            self.connection.execute("DELETE FROM label WHERE task=?", (task["id"],))
            self.connection.executemany("INSERT OR IGNORE INTO label VALUES (?,?)", ((task["id"], label) for label in labels))
            projects = {task["project"]} | ({old["project"]} if old else set())
            for scope in {"*"} | projects:
                self.connection.execute("INSERT INTO epoch VALUES (?,1) ON CONFLICT(scope) DO UPDATE SET value=value+1", (scope,))
            self.connection.commit()

    def _revision(self, project, basis):
        row = self._one("SELECT value FROM epoch WHERE scope=?", ("*" if basis == "global" else project,))
        return row[0] if row else 0

    def start(self, project, *, roster, labels, workspaces, statuses, coordinator=None,
              status_filter=None, basis="project", view="list"):
        exact_id(project)
        if basis not in {"global", "project"}:
            raise Unsupported("basis must be global or this one project's epoch")
        if view not in {"list", "board"}:
            raise Unsupported("unknown view")
        if coordinator is not None:
            exact_id(coordinator)
        build = uuid.uuid4().hex
        with self._guard(operation="start"):
            for kind, token in (("roster", roster), ("labels", labels), ("statuses", statuses)):
                self._set(token, kind)
            if workspaces is not None:
                self._set(workspaces, "workspaces")
            if status_filter is not None:
                self._set(status_filter, "status_filter")
            self.connection.execute("""INSERT INTO build(id,project,basis,revision,roster,labels,workspaces,statuses,
              status_filter,coordinator,view) VALUES (?,?,?,?,?,?,?,?,?,?,?)""",
              (build, project, basis, self._revision(project, basis), roster, labels, workspaces, statuses, status_filter, coordinator, view))
            for token in (roster, labels, workspaces, statuses, status_filter):
                if token is not None:
                    self.connection.execute("UPDATE input_set SET refs=refs+1 WHERE id=?", (token,))
        return build

    def checkpoint(self, build):
        row = self._one("SELECT * FROM build WHERE id=?", (build,))
        if row is None:
            raise KeyError(build)
        return dict(row)

    def _valid_basis(self, state):
        return state["revision"] == self._revision(state["project"], state["basis"])

    def _select_header(self, state, task):
        if state["workspaces"] is not None:
            if task["holder_kind"] != "agent" or not self._member(state["workspaces"], task["holder_id"]):
                return False
        if state["view"] == "list" and state["status_filter"]:
            chosen = self._set(state["status_filter"], "status_filter")
            if chosen["count"] and not self._member(state["status_filter"], task["status"]):
                return False
        return True

    def _rank(self, state, task):
        if task["state"] == "closed" or task["status"] == "done":
            return 3
        holder = task["holder_id"] if task["holder_kind"] == "agent" else (
            state["coordinator"] if task["holder_kind"] == "project_agent" else None)
        if holder is not None and self._member(state["roster"], holder):
            return 1
        return 2 if task["attention"] else 3

    def _record(self, state, task):
        rank = self._rank(state, task)
        self.connection.execute("INSERT INTO result VALUES (?,?,?,?,?,?,?,?,?,?)", (
            state["id"], task["id"], rank, task["number"], task["status"], task["priority"], task["blocked"],
            -task["number"], -task["priority"], -int(task["blocked"])))
        self.connection.execute("""INSERT INTO status_count VALUES (?,?,1,?)
          ON CONFLICT(owner,status) DO UPDATE SET count=count+1, maximum=max(maximum,excluded.maximum)""",
          (state["id"], task["status"], task["number"]))
        state["total"] += 1
        state[f"g{rank}"] += 1
        state["maximum"] = max(state["maximum"], task["number"])
        state["readiness"] = bool(state["readiness"] or (task["status"] == "backlog" and task["readiness"]))

    def _scan_labels(self, state, task, remaining):
        """A separate keyset cursor allows one task's labels to span quanta."""
        chosen = self._set(state["labels"], "labels")
        if not chosen["count"]:
            return True, True, 0
        rows = self.connection.execute("SELECT value FROM label WHERE task=? AND value>? ORDER BY value LIMIT ?",
                                       (task["id"], state["label_cursor"], remaining)).fetchall()
        self.metrics["label_bytes"] += sum(len(row["value"].encode()) for row in rows)
        for row in rows:
            state["label_cursor"] = row["value"]
            if self._member(state["labels"], row["value"]):
                return True, True, len(rows)
        return len(rows) < remaining, False, len(rows)

    def _advance(self, state, metrics):
        while metrics["headers"] < self.HEADER_LIMIT and metrics["labels"] < self.LABEL_LIMIT:
            if state["pending"] is None:
                row = self._one("SELECT header FROM task WHERE project=? AND id>? ORDER BY id LIMIT 1",
                                (state["project"], state["cursor"]))
                if row is None:
                    state["phase"] = "finish"
                    return
                metrics["headers"] += 1
                metrics["header_bytes"] += len(row["header"].encode())
                state["pending"] = row["header"]
                state["label_cursor"] = ""
            task = json.loads(state["pending"])
            if not task["supported"]:
                # Refuse before filters: a lossy field cannot prove exclusion.
                state["state"] = "unsupported"
                state["phase"] = "refused"
                return
            eligible = self._select_header(state, task)
            complete, matched, read = (True, False, 0) if not eligible else self._scan_labels(
                state, task, self.LABEL_LIMIT - metrics["labels"])
            metrics["labels"] += read
            if not complete:
                return
            if matched:
                self._record(state, task)
            state["cursor"] = task["id"]
            state["pending"] = None
            state["label_cursor"] = ""

    def quantum(self, build, *, vm_budget=100000, soft_ms=None):
        with self._guard(vm_budget, soft_ms, operation="quantum") as metrics:
            state = self.checkpoint(build)
            if state["state"] != "pending":
                return state["state"]
            if not self._valid_basis(state):
                self.connection.execute("UPDATE build SET state='invalid' WHERE id=?", (build,))
                return "invalid"
            if state["phase"] == "finish":
                # Same write transaction as the basis comparison: no writer can
                # slip between verification and this O(1) publication marker.
                self.connection.execute("UPDATE build SET state='sealed' WHERE id=?", (build,))
                outcome = "sealed"
            else:
                self._advance(state, metrics)
                self.connection.execute("""UPDATE build SET state=?,phase=?,cursor=?,pending=?,label_cursor=?,
                  total=?,maximum=?,readiness=?,g1=?,g2=?,g3=? WHERE id=?""", (state["state"], state["phase"], state["cursor"], state["pending"],
                  state["label_cursor"], state["total"], state["maximum"], state["readiness"], state["g1"], state["g2"], state["g3"], build))
                outcome = state["state"]
        self.evidence.append(dict(metrics, phase=state["phase"], outcome=outcome))
        return outcome

    def _sealed(self, build):
        state = self.checkpoint(build)
        if state["state"] != "sealed":
            raise Pending(f"selection is {state['state']}; counts are unavailable")
        # A previously sealed answer is readable only against the same basis.
        if not self._valid_basis(state):
            raise Pending("published basis is stale")
        return state

    def status_page(self, build, after=""):
        with self._guard():
            self._sealed(build)
            return [tuple(row) for row in self.connection.execute(
                "SELECT status,count FROM status_count WHERE owner=? AND status>? ORDER BY status LIMIT 64", (build, after))]

    def summary(self, build):
        """Harness consumer: assemble bounded status pages outside build work."""
        with self._guard():
            state = self._sealed(build)
        counts, after = {}, ""
        while page := self.status_page(build, after):
            counts.update(page)
            after = page[-1][0]
        return dict(total=state["total"], maximum=state["maximum"], readiness=bool(state["readiness"]), counts=counts,
                    groups={rank: state[f"g{rank}"] for rank in (1, 2, 3)})

    def column_page(self, build, after=-1):
        """Configured order, including exact zero columns; count/max are O(1) probes."""
        with self._guard():
            state = self._sealed(build)
            return [tuple(row) for row in self.connection.execute("""SELECT m.position,m.value,
              coalesce(c.count,0),coalesce(c.maximum,0) FROM member m LEFT JOIN status_count c
              ON c.owner=? AND c.status=m.value WHERE m.owner=? AND m.position>?
              ORDER BY m.position LIMIT 64""", (build, state["statuses"], after))]

    def list_page(self, build, after=(0, 0, "")):
        with self._guard():
            self._sealed(build)
            rank, number, task = after
            return [tuple(row) for row in self.connection.execute("""SELECT task,rank,number,status,priority,blocked
              FROM result WHERE owner=? AND (rank,number_key,task)>(?,?,?)
              ORDER BY rank,number_key,task LIMIT 64""", (build, rank, -number, task))]

    def all_rows(self, build):
        """Harness consumer, not one unbounded query or one build quantum."""
        rows, after = [], (0, 0, "")
        while page := self.list_page(build, after):
            rows.extend(page)
            after = (page[-1][1], page[-1][2], page[-1][0])
        return rows

    def board_rows(self, build, status):
        """Harness consumer of status-specific keyset pages."""
        with self._guard():
            state = self._sealed(build)
        grouped = status == "backlog" and state["readiness"] and self._member(state["statuses"], "backlog")
        rows, after = [], (-2, -5, -(2**63), "")
        while True:
            with self._guard():
                self._sealed(build)
                columns = "blocked_key,priority_key,number_key,task" if grouped else "number_key,task"
                values = after if grouped else after[2:]
                marks = ",".join("?" for _ in values)
                # Sort fragments/placeholder count are fixed internal variants; all values are bound.
                # nosemgrep: python.sqlalchemy.security.sqlalchemy-execute-raw-query.sqlalchemy-execute-raw-query
                page = [tuple(row) for row in self.connection.execute(f"""SELECT task,rank,number,status,priority,blocked
                  FROM result WHERE owner=? AND status=? AND ({columns})>({marks})
                  ORDER BY {columns} LIMIT 64""", (build, status, *values))]
            if not page:
                return rows
            rows.extend(page)
            last = page[-1]
            after = (-last[5] if grouped else 0, -last[4], -last[2], last[0])

    def cancel(self, build):
        """O(1) visibility change; deletion is always a separate operation."""
        with self._guard(operation="cancel"):
            self.connection.execute("UPDATE build SET state='cancelled' WHERE id=?", (build,))

    def exists(self, build):
        return self._one("SELECT 1 FROM build WHERE id=?", (build,)) is not None

    def gc(self, build):
        with self._guard(operation="gc"):
            state = self.checkpoint(build)
            if state["state"] not in {"cancelled", "invalid", "unsupported"}:
                raise ValueError("GC requires an unpublished terminal build")
            removed = 0
            for table in ("result", "status_count"):
                remaining = self.GC_LIMIT - removed
                # Table is one of the two fixed internal names in this loop; values are bound.
                # nosemgrep: python.sqlalchemy.security.sqlalchemy-execute-raw-query.sqlalchemy-execute-raw-query
                removed += self.connection.execute(f"""DELETE FROM {table} WHERE rowid IN
                  (SELECT rowid FROM {table} WHERE owner=? LIMIT ?)""", (build, remaining)).rowcount
            if removed < self.GC_LIMIT:
                for kind in ("roster", "labels", "workspaces", "statuses", "status_filter"):
                    if state[kind] is not None:
                        self.connection.execute("UPDATE input_set SET refs=refs-1 WHERE id=?", (state[kind],))
                removed += self.connection.execute("DELETE FROM build WHERE id=?", (build,)).rowcount
            return removed

    def cancel_set(self, token):
        """Only staging input can be abandoned; sealed sets may be shared."""
        with self._guard(operation="cancel_set"):
            self.connection.execute("UPDATE input_set SET state='cancelled' WHERE id=? AND state='staging'", (token,))

    def release_set(self, token):
        """Explicit owner release after all dependent builds have been collected."""
        with self._guard(operation="release_set"):
            state = self._one("SELECT * FROM input_set WHERE id=?", (token,))
            if state is None or state["state"] != "sealed" or state["refs"]:
                raise ValueError("sealed input is missing or still referenced")
            self.connection.execute("UPDATE input_set SET state='cancelled' WHERE id=?", (token,))

    def gc_set(self, token):
        with self._guard(operation="gc_set"):
            state = self._one("SELECT * FROM input_set WHERE id=?", (token,))
            if state is None or state["state"] != "cancelled":
                raise ValueError("only abandoned staging input can be collected")
            removed = self.connection.execute("""DELETE FROM member WHERE rowid IN
              (SELECT rowid FROM member WHERE owner=? LIMIT 64)""", (token,)).rowcount
            if removed < self.GC_LIMIT:
                removed += self.connection.execute("DELETE FROM input_set WHERE id=?", (token,)).rowcount
            return removed
