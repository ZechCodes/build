import { describe, expect, it } from "vitest";
import { assigneesOf, filterTasks, taskListParams } from "../src/core/trackerFilters.js";

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
// See bridge/src/app/tests/tracker_query_choice_roundtrip.rs:
// pending_assignment_transitions and pending_only_any_choice_disappears_on_cancel.
const canonical = [
  ["any", ["waiting", "queued", "provisioning", "failed", "pending-user", "pending-workspace", "assigned", "prefix", "replacement", "different-target"]],
  ["none", ["missing-target", "consumed", "cancelled", "unassigned"]],
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
    const pendingIds = ["waiting", "queued", "provisioning", "failed", "pending-user", "pending-workspace"];
    expect(differences).toEqual([
      { want: "any", added: pendingIds, removed: [] },
      { want: "none", added: [], removed: pendingIds },
    ]);
  });

  it("unions real exact actors and pending held choices without inventing target membership", () => {
    expect(ids(filterTasks(tasks, { assignee: ["none", exact] })))
      .toEqual(["missing-target", "consumed", "cancelled", "unassigned", "assigned"]);
    expect(assigneesOf(tasks).map(({ value }) => value))
      .toEqual([exact, "agent:AgentA", "agent:AgentA\uFFFDsuffix", "user"]);
  });
});

it("keeps all scalar legacy tasks.list parameter shapes unchanged", () => {
  for (const [value, wire] of [["any", "any"], ["none", "none"], ["user", { kind: "user" }], [exact, actor]]) {
    expect(taskListParams("project", { state: "open", status: "backlog", label: "api\0v2", assignee: value }))
      .toEqual({ project_id: "project", state: "open", status: "backlog", label: "api\0v2", assignee: wire });
  }
  expect(taskListParams("project", { assignee: ["any", "none"] })).toEqual({ project_id: "project" });
});
