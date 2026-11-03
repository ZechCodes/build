import { readFileSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";
import { describe, expect, it } from "vitest";
import { assigneesOf, filterTasks, taskListParams } from "../src/core/trackerFilters.js";
import { taskIsHeld } from "../src/core/trackerTaskReadiness.js";
import { summaryPatch } from "../src/core/taskSummaryProjection.js";
import { taskRecord, tasksRecord } from "../src/core/trackerCache.js";

const exact = "agent:AgentA\0suffix";
const actor = { kind: "agent", agent_id: "AgentA\0suffix" };
const pending = (id, state, target = actor) => ({ id, assignee: null, deferred_assignment: { state, target } });
const rows = [
  ...["waiting", "queued", "provisioning", "failed"].map((state) => pending(state, state)),
  pending("pending-user", "waiting", { kind: "user" }),
  pending("pending-workspace", "provisioning", { kind: "new_workspace" }),
  pending("missing-target", "waiting", null),
  pending("consumed", "consumed"),
  pending("cancelled", "cancelled"),
  { id: "unassigned", assignee: null },
  { id: "assigned", assignee: actor },
  { id: "prefix", assignee: { kind: "agent", agent_id: "AgentA" } },
  { id: "replacement", assignee: { kind: "agent", agent_id: "AgentA\uFFFDsuffix" } },
  { ...pending("different-target", "queued"), assignee: { kind: "user" } },
];

// Oracle one is the server contract, pinned independently as expected IDs.
// AssigneeFilter::matches consumes Option<&Value>, not target.kind truthiness.
// Store maps an active intent to &intent.target even for Value::Null. These
// source rules apply to both scalar lists and plural TaskSelections.
const canonical = [
  ["any", ["waiting", "queued", "provisioning", "failed", "pending-user", "pending-workspace", "missing-target", "assigned", "prefix", "replacement", "different-target"]],
  ["none", ["consumed", "cancelled", "unassigned"]],
  [exact, ["assigned"]],
  ["agent:AgentA", ["prefix"]],
  ["agent:AgentA\uFFFDsuffix", ["replacement"]],
  ["user", ["different-target"]],
];

// Oracle two freezes the previous consumer's observed behavior. It is kept
// separate from the server oracle so the intentional visible changes are
// explicit, rather than silently accepting parity with the old consumer.
const previousConsumer = (task, want) => {
  if (want === "any") return Boolean(task.assignee?.kind);
  const key = !task.assignee?.kind ? "none" : task.assignee.kind === "agent"
    ? `agent:${task.assignee.agent_id}` : task.assignee.kind;
  return key === want;
};
const ids = (tasks) => tasks.map((task) => task.id);

describe.each(["whole cached task", "summary projection"])("canonical pending membership from a %s source", (source) => {
  const tasks = rows.map((row) => source === "whole cached task"
    ? { ...row, body: "full task body", timeline: [] }
    : { ...row, number: 1, state: "open", status: "backlog" });

  it.each(canonical)("matches the server oracle for %s", (want, expected) => {
    expect(ids(filterTasks(tasks, { assignee: want }))).toEqual(expected);
    expect(ids(filterTasks(tasks, { assignee: [want] }))).toEqual(expected);
  });

  it("records exactly where the canonical and previous-consumer oracles differ", () => {
    const differences = canonical.map(([want, expected]) => {
      const before = ids(tasks.filter((task) => previousConsumer(task, want)));
      return { want, added: expected.filter((id) => !before.includes(id)), removed: before.filter((id) => !expected.includes(id)) };
    }).filter(({ added, removed }) => added.length || removed.length);
    const pendingIds = ["waiting", "queued", "provisioning", "failed", "pending-user", "pending-workspace", "missing-target"];
    expect(differences).toEqual([
      { want: "any", added: pendingIds, removed: [] },
      { want: "none", added: [], removed: pendingIds },
    ]);
  });

  it("unions real exact actors and pending held choices without inventing target membership", () => {
    expect(ids(filterTasks(tasks, { assignee: ["none", exact] })))
      .toEqual(["consumed", "cancelled", "unassigned", "assigned"]);
    expect(assigneesOf(tasks).map(({ value }) => value))
      .toEqual([exact, "agent:AgentA", "agent:AgentA\uFFFDsuffix", "user"]);
  });
});

describe("pending presence independent of the previous kind-based held consumer", () => {
  it("pins canonical Option presence and Store target extraction", () => {
    const source = (path) => readFileSync(new URL(`../../bridge/src/${path}`, import.meta.url), "utf8");
    const filters = source("app/tracker/edits.rs");
    const store = source("store/tracker.rs");
    expect(filters).toContain("AssigneeFilter::Unassigned => task.assignee.is_none() && pending_target.is_none()");
    expect(filters).toContain("AssigneeFilter::Assigned => task.assignee.is_some() || pending_target.is_some()");
    expect(filters).toContain("AssigneeFilter::Exactly(wanted) => task.assignee.as_ref() == Some(wanted)");
    expect(store).toContain('"waiting" | "queued" | "provisioning" | "failed"');
    expect(store).toContain(".map(|intent| &intent.target)");
    expect(source("store/delivery_intents.rs")).toContain("pub target: Value");
    const selections = source("app/tracker/selections.rs");
    expect(selections).toContain(".any(|filter| filter.matches(task, pending))");
    expect(selections).toContain('AssigneeFilter::Assigned => !row["assignee"].is_null() || pending.is_some()');
  });

  const targets = [
    ["null target", null, false],
    ["missing kind", {}, false],
    ["null kind", { kind: null }, false],
    ["empty kind", { kind: "" }, false],
    ["false kind", { kind: false }, false],
    ["zero kind", { kind: 0 }, false],
    ["NUL kind", { kind: "\0" }, true],
    ["Unicode kind", { kind: "用户\u{10000}" }, true],
    ["whitespace kind", { kind: " \t" }, true],
    ["array kind", { kind: [] }, true],
    ["object kind", { kind: {} }, true],
    ["numeric kind", { kind: 1 }, true],
    ["boolean kind", { kind: true }, true],
    ["exact actor target", actor, true],
    ["user target", { kind: "user" }, true],
    ["project target", { kind: "project_agent" }, true],
  ];
  const activeStates = ["waiting", "queued", "provisioning", "failed"];

  describe.each(activeStates)("active %s delivery", (state) => {
    it.each(targets)("uses pending presence for %s without manufacturing exact actor membership", (_label, target, heldBefore) => {
      const task = pending("pending", state, target);
      // Two independent oracles: the prior held consumer's literal result,
      // and the source's Some(&intent.target), including Some(&Value::Null).
      expect(taskIsHeld(task)).toBe(heldBefore);
      expect(ids(filterTasks([task], { assignee: "any" }))).toEqual(["pending"]);
      expect(filterTasks([task], { assignee: "none" })).toEqual([]);
      for (const want of [exact, "user", "project_agent"]) {
        expect(filterTasks([task], { assignee: want })).toEqual([]);
      }
    });
  });

  it.each(["consumed", "cancelled", "", "waiting\0", "等待", undefined])(
    "does not infer pending presence from target for inactive state %j", (state) => {
      for (const [, target] of targets) {
        const task = pending("inactive", state, target);
        expect(taskIsHeld(task)).toBe(false);
        expect(filterTasks([task], { assignee: "any" })).toEqual([]);
        expect(ids(filterTasks([task], { assignee: "none" }))).toEqual(["inactive"]);
      }
    },
  );

  it("keeps an actual typed assignee authoritative regardless of pending target or state", () => {
    for (const state of [...activeStates, "consumed", "cancelled"]) {
      for (const [, target] of targets) {
        const task = { ...pending("actual", state, target), assignee: actor };
        expect(ids(filterTasks([task], { assignee: "any" }))).toEqual(["actual"]);
        expect(filterTasks([task], { assignee: "none" })).toEqual([]);
        expect(ids(filterTasks([task], { assignee: exact }))).toEqual(["actual"]);
        expect(filterTasks([task], { assignee: "user" })).toEqual([]);
      }
    }
  });

  it("records the exact visible difference from the kind-based held consumer", () => {
    const tasks = targets.map(([id, target]) => pending(id, "waiting", target));
    const previousAssigned = ids(tasks.filter(taskIsHeld));
    const assigned = ids(filterTasks(tasks, { assignee: "any" }));
    const previousUnassigned = ids(tasks.filter((task) => !taskIsHeld(task)));
    const unassigned = ids(filterTasks(tasks, { assignee: "none" }));
    const changed = ["null target", "missing kind", "null kind", "empty kind", "false kind", "zero kind"];
    expect(assigned.filter((id) => !previousAssigned.includes(id))).toEqual(changed);
    expect(previousUnassigned.filter((id) => !unassigned.includes(id))).toEqual(changed);
  });
});

describe("serialized canonical producer facts", () => {
  const source = (path) => readFileSync(new URL(`../../bridge/src/${path}`, import.meta.url), "utf8");

  it("pins full list/get serialization of active intent state even for a null target", () => {
    const read = source("app/tracker/read.rs");
    expect(read).toContain("extend_task_summary(source, &task, &mut row)?");
    expect(read).toContain('extend_task_summary(source, &task, &mut answer["task"])?');
    expect(read).toContain("extend_assignment(source, &task.id, row)");
    expect(read).toContain('"waiting" | "queued" | "provisioning" | "failed"');
    expect(read).toContain('"provisioning" => "queued"');
    expect(read).toContain('row["deferred_assignment"] =\n        json!({"id":intent.id,"target":intent.target,"state":state,"error":intent.error})');
  });

  it("executes the actual Rust summary producer SQL before testing local pending membership", () => {
    const sql = source("store/task_summaries/sql.rs");
    const patch = /fn enrich_patch\(\) -> &'static str \{\s*r#"([\s\S]*?)"#/u.exec(sql)?.[1];
    expect(patch, "execute the source producer, not a JS rewrite").toBeTypeOf("string");
    expect(sql).toContain("json_patch(json_remove(tracker_task_summaries.record,'$.review_summary','$.legacy_review','$.deferred_assignment'),m.patch)");
    expect(sql).toContain("patch=enrich_patch()");
    expect(patch).toContain("json_object('legacy_review',json('true'))");
    const db = new DatabaseSync(":memory:");
    try {
      db.exec("CREATE TABLE reviews(task_id TEXT,record TEXT,version INTEGER); CREATE TABLE tracker_delivery_intents(id TEXT,task_id TEXT,generation INTEGER,state TEXT,record TEXT)");
      // This outer merge is the exact refresh_metadata producer expression,
      // pinned above. Inputs are saved intent records; outputs are serialized
      // summary rows, so a lost pending-state fact fails before filter checks.
      const produce = db.prepare(`SELECT json_patch(json_remove(?,'$.review_summary','$.legacy_review','$.deferred_assignment'),${patch}) AS record FROM (SELECT 'fixture' AS id) t`);
      const insert = db.prepare("INSERT INTO tracker_delivery_intents VALUES('intent','fixture',1,?,?)");
      for (const state of [null, "waiting", "queued", "provisioning", "failed", "consumed", "cancelled"]) {
        for (const target of [null, {}, { kind: "" }, { kind: "\0" }, { kind: "用户\u{10000}" }]) {
          for (const assignee of [null, actor]) {
            db.exec("DELETE FROM tracker_delivery_intents");
            if (state !== null) {
              insert.run(state, JSON.stringify({
                id: "intent", task_id: "fixture", project_path: "/fixture", generation: 1,
                target, actor: { kind: "user" }, sender: null, note: null,
                operation_id: "operation", state, reservation: {}, admitted: null, error: null,
              }));
            }
            const row = JSON.parse(produce.get(JSON.stringify({ id: "fixture", assignee, legacy_review: true })).record);
            const active = ["waiting", "queued", "provisioning", "failed"].includes(state);
            expect(row.assignee).toEqual(assignee);
            expect(row).not.toHaveProperty("legacy_review");
            if (active) {
              expect(row.deferred_assignment).toBeTypeOf("object");
              expect(row.deferred_assignment.state).toBe(state === "provisioning" ? "queued" : state);
            } else {
              expect(row).not.toHaveProperty("deferred_assignment");
            }
            if (active && assignee === null && target?.kind === "\0") {
              // SQLite's display substr loses a leading-NUL kind. The raw
              // held consumer therefore differs after this real projection,
              // while the producer's active-state presence remains lossless.
              expect(row.deferred_assignment.target.kind).toBe("");
              expect(taskIsHeld(row)).toBe(false);
            }
            const assigned = assignee !== null || active;
            // Production cache DTOs also preserve the serialized active-state
            // fact; target projection must not decide any/none membership.
            const cached = [row, summaryPatch(null, row), tasksRecord([row]).tasks[0], taskRecord(row).task];
            for (const task of cached) {
              expect(task.deferred_assignment?.state).toBe(row.deferred_assignment?.state);
              expect(filterTasks([task], { assignee: "any" })).toHaveLength(assigned ? 1 : 0);
              expect(filterTasks([task], { assignee: "none" })).toHaveLength(assigned ? 0 : 1);
              expect(filterTasks([task], { assignee: exact })).toHaveLength(assignee === null ? 0 : 1);
            }
          }
        }
      }
      db.exec("DELETE FROM tracker_delivery_intents");
      const writeReview = db.prepare("INSERT INTO reviews VALUES('fixture',?,1)");
      writeReview.run(JSON.stringify({})); // Historical rows omit mode.
      const legacy = JSON.parse(produce.get(JSON.stringify({ id: "fixture" })).record);
      expect(legacy.legacy_review).toBe(true);
      expect(legacy).not.toHaveProperty("review_summary");
      db.exec("DELETE FROM reviews");
      writeReview.run(JSON.stringify({ mode: "pull_request", pull_request: {
        originating_workspace_id: "workspace", status: "open", latest_published_snapshot_id: "revision",
      } }));
      const replacement = JSON.parse(produce.get(JSON.stringify({ id: "fixture", legacy_review: true })).record);
      expect(replacement).not.toHaveProperty("legacy_review");
      expect(replacement.review_summary).toMatchObject({ mode: "pull_request", status: "open" });
    } finally {
      db.close();
    }
  });
});

it("keeps all scalar legacy tasks.list parameter shapes unchanged", () => {
  for (const [value, wire] of [["any", "any"], ["none", "none"], ["user", { kind: "user" }], [exact, actor]]) {
    expect(taskListParams("project", { state: "open", status: "backlog", label: "api\0v2", assignee: value }))
      .toEqual({ project_id: "project", state: "open", status: "backlog", label: "api\0v2", assignee: wire });
  }
  expect(taskListParams("project", { assignee: ["any", "none"] })).toEqual({ project_id: "project" });
});
