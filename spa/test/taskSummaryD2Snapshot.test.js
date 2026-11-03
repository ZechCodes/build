import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

// D2 is an independent raw query snapshot lane. These tests deliberately keep
// watch authority UNKNOWN through a genuine pending local intent.
let cache, summaries, queries, readers, watch;
const scope = { deviceId: "d2-snapshot-891", projectId: "proj-1" };
const basis = "store:opaque-D2-snapshot";
const allCapabilities = {
  listSummaries: true, listWatched: true, listAttention: true, listLinkedWorkspace: true,
  listTracked: true, listReviewMode: true, listOrder: true, listReadiness: true, listExcludeAssignee: true,
};
const exactAgent = { kind: "agent", agent_id: "AgentA\u0000É" };
const maximalAllowed = {
  states: ["open"], statuses: ["ready"], labels: ["API", "É"], assignees: [exactAgent],
  linked_workspace_id: "Workspace\u0000É", tracker_agent_id: exactAgent.agent_id,
  exclude_review_mode: "snapshot", unfinished: true, blocked: false, held: true, undelivered: false, order: "priority",
};

function summaryRow(number, filters, extra = {}) {
  const task = {
    id: `task-${number}`, project_id: scope.projectId, number, title: `Task ${number}`,
    state: "open", status: "ready", labels: ["api", "É"], priority: "high", assignee: exactAgent,
    links: { workspace_ids: [], parent_task_id: null }, trackers: [],
    dependencies: { blocked: false, waiting_on_count: 0, blocks_count: 0 },
    created_by: { kind: "user" }, created_at: "2026-10-29T10:00:00Z",
    updated_at: "2026-10-29T12:00:00Z", closed_at: null, done_at: null,
    watched: true, read_through: "tc-10", watch_started_after: "te-01", dismissed_through: null,
    unread_count: 2, unread_mentions: 1, unread_agent_comments: 1,
    session_started_ms: 1793275200000, last_activity_ms: 1793275200000,
    review_summary: { task_id: `task-${number}`, version: 1, status: "open", mode: "pull_request" },
    deferred_assignment: null, attachment_count: 0, summary_truncated: false, ...extra,
  };
  const relations = ["linked_workspace_id", "tracker_agent_id"].filter((key) => Object.hasOwn(filters, key));
  if (relations.length) task.matched_memberships = Object.fromEntries(relations.map((key) => [key, filters[key]]));
  if (filters.order === "anchor") task.order_anchor_ms = 1793275200000;
  if (task.watched === false) {
    for (const key of ["unread_count", "unread_mentions", "unread_agent_comments", "session_started_ms", "last_activity_ms"]) delete task[key];
  }
  return task;
}

const page = (task, next_cursor) => ({ project_id: scope.projectId, summary: true, basis,
  tasks: task ? [task] : [], identities: {}, next_cursor });

function setupReader(filters = {}, extra = {}, capabilities = allCapabilities) {
  const first = summaryRow(2, filters, extra), second = summaryRow(1, filters, extra);
  const rpc = vi.fn().mockResolvedValueOnce(page(first, "opaque-next-D2")).mockResolvedValueOnce(page(second, null));
  const session = {};
  const view = readers.createTaskSummaryReader({ scope, filters, callRpc: rpc,
    capabilities: () => capabilities, sessionOf: () => session });
  return { view, rpc, first, second };
}

function setupEmptyReader(filters) {
  const result = setupReader(filters);
  // A bounded server scan may return no matching rows and a continuation.
  // This remains valid even when a denied conjunction has no possible match.
  result.rpc.mockReset().mockResolvedValueOnce(page(null, "opaque-next-D2"));
  return result;
}

async function expectUnknown(view) {
  const result = await view.read();
  expect(result.stale).toBe(true);
  expect(await watch.readTaskWatchState(scope)).toMatchObject({ pendingCount: 1, uncertain: true, eligible: false });
  for (const id of result.ids) {
    const row = (await cache.readCached(summaries.summaryAddress(scope, id))).value;
    expect(row.watch_uncertain).toBe(true);
    expect(row.basis_uncertain).toBe(true);
    expect(row).not.toHaveProperty("basis", basis);
  }
  expect(result).not.toHaveProperty("total");
  return result;
}

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  summaries = await import("../src/core/taskSummaryCache.js");
  queries = await import("../src/core/taskSummaryQuery.js");
  readers = await import("../src/core/taskSummaryReader.js");
  watch = await import("../src/core/taskWatchAuthority.js");
  expect(await watch.beginTaskWatchMutation(scope, "d2-local-pending", { watched: false, metadataOnly: true })).not.toBeNull();
});

it("D2 advances default-number raw snapshot only after explicit more while watch authority stays UNKNOWN", async () => {
  const { view, rpc } = setupReader({}, {}, { listSummaries: true });
  expect(await view.refresh()).toBe(true);
  const first = await expectUnknown(view);
  expect(first).toMatchObject({ ids: ["task-2"], basis, next_cursor: "opaque-next-D2", complete: false });
  expect(rpc.mock.calls).toEqual([["tasks.list", { project_id: scope.projectId, summary: true, limit: 100 }]]);

  // This is the first behavior RED on the existing public API: no new helper
  // import, mocked reducer or private known-zero proof precedes the assertion.
  expect(await view.more()).toBe(true);
  expect(rpc.mock.calls[1]).toEqual(["tasks.list", {
    project_id: scope.projectId, summary: true, limit: 100, cursor: "opaque-next-D2",
  }]);
  const continued = await expectUnknown(view);
  expect(continued).toMatchObject({ ids: ["task-2", "task-1"], basis, next_cursor: null, complete: true });
  expect(continued.tasks.map(({ number }) => number)).toEqual([2, 1]);
  expect(await view.more()).toBe(false);
  expect(rpc).toHaveBeenCalledTimes(2);
  view.dispose();
});

// Classify the complete effective query through the actual reader rather than
// a test-local classifier. Every table case keeps the same real UNKNOWN store.
const allowed = [
  ["omitted dimensions and order", {}],
  ["null scalar dimensions", { state: null, status: null, label: null, assignee: null }],
  ["empty plural dimensions", { states: [], statuses: [], labels: [], assignees: [] }],
  ["explicit default number", { order: "number" }],
  ["priority order", { order: "priority" }],
  ["scalar state", { state: "open" }],
  ["plural states", { states: ["open", "closed", "open"] }],
  ["scalar status alias", { status: "In review" }, { status: "in_review" }],
  ["plural status aliases", { statuses: ["In review", "ready", "in_review"] }],
  ["scalar ASCII label", { label: "API" }],
  ["plural ASCII and exact non-ASCII labels", { labels: ["API", "api", "É"] }],
  ["actual user", { assignee: { kind: "user" } }, { assignee: { kind: "user" } }],
  ["actual project agent", { assignee: { kind: "project_agent" } }, { assignee: { kind: "project_agent" } }],
  ["exact opaque actual agent", { assignee: exactAgent }],
  ["only actual actors in plural OR", { assignees: [{ kind: "user" }, exactAgent] }],
  ["workspace membership outside compact prefix", { linked_workspace_id: "Workspace\u0000É" }],
  ["tracker membership outside compact prefix", { tracker_agent_id: exactAgent.agent_id }],
  ["exclude snapshot review", { exclude_review_mode: "snapshot" }],
  ["exclude pull request review", { exclude_review_mode: "pull_request" }, { review_summary: null }],
  ["unfinished true", { unfinished: true }],
  ["unfinished false", { unfinished: false }, { status: "done", done_at: "2026-10-29T11:00:00Z" }],
  ["blocked true", { blocked: true }, { dependencies: { blocked: true, waiting_on_count: 1, blocks_count: 0 } }],
  ["blocked false", { blocked: false }],
  ["held true", { held: true }],
  ["held false", { held: false }, { assignee: null }],
  ["undelivered true", { undelivered: true }, { deferred_assignment: { state: "queued", target: null } }],
  ["undelivered false", { undelivered: false }],
  ["all allowed dimensions combined", maximalAllowed],
];

describe("D2 complete effective query allow classes", () => {
  it.each(allowed)("continues %s without current-watch certification", async (_name, filters, extra) => {
    const { view, rpc } = setupReader(filters, extra);
    expect(await view.refresh()).toBe(true);
    await expectUnknown(view);
    expect(rpc).toHaveBeenCalledTimes(1);
    expect(await view.more()).toBe(true);
    expect(rpc).toHaveBeenCalledTimes(2);
    expect(rpc.mock.calls[1][1]).toEqual({ project_id: scope.projectId, summary: true,
      ...queries.canonicalSummaryFilters(filters), limit: 100, cursor: "opaque-next-D2" });
    await expectUnknown(view);
    view.dispose();
  });
});

const denied = [
  ["watched true", { watched: true }], ["watched false", { watched: false }],
  ["asked attention", { attention_rule: "asked" }], ["legacy attention", { attention_rule: "legacy" }],
  ["updated order", { order: "updated" }], ["anchor order", { order: "anchor" }],
  ["any actual or pending", { assignee: "any" }], ["none actual or pending", { assignee: "none" }],
  ["plural any plus exact actual", { assignees: [exactAgent, "any"] }],
  ["plural none plus exact actual", { assignees: [exactAgent, "none"] }],
  ["exclude actual user", { exclude_assignee: { kind: "user" } }],
  ["exclude project agent", { exclude_assignee: { kind: "project_agent" } }],
  ["exclude exact agent", { exclude_assignee: { kind: "agent", agent_id: "OtherAgent" } }],
  ["zero done cutoff", { done_since: 0 }],
];

describe("D2 excluded classes remain excluded in complete combinations", () => {
  it.each(denied)("does not continue %s under UNKNOWN", async (_name, filters) => {
    const { view, rpc } = setupEmptyReader(filters);
    expect(await view.refresh()).toBe(true);
    const before = await expectUnknown(view);
    expect(await view.more()).toBe(false);
    expect(rpc).toHaveBeenCalledTimes(1);
    expect(await view.read()).toEqual(before);
    view.dispose();
  });

  it.each(denied)("does not hide %s inside allowed dimensions", async (_name, excluded) => {
    const filters = { ...maximalAllowed, ...excluded };
    if (Object.hasOwn(excluded, "assignee")) delete filters.assignees;
    const { view, rpc } = setupEmptyReader(filters);
    expect(await view.refresh()).toBe(true);
    await expectUnknown(view);
    expect(await view.more()).toBe(false);
    expect(rpc).toHaveBeenCalledTimes(1);
    view.dispose();
  });
});

describe("D2 preserves original canonicalization and capability requirements", () => {
  it("keeps explicit default capability distinct despite equivalent canonical identity", () => {
    expect(queries.canonicalSummaryFilters({ order: "number" })).toEqual(queries.canonicalSummaryFilters({}));
    expect(queries.summaryFiltersSupported({}, { listSummaries: true })).toBe(true);
    expect(queries.summaryFiltersSupported({ order: "number" }, { listSummaries: true })).toBe(false);
    expect(queries.canonicalSummaryFilters({ labels: ["API", "api", "É"], statuses: ["In review", "in_review"],
      assignees: [exactAgent] })).toEqual({ labels: ["api", "É"], statuses: ["in_review"], assignees: [exactAgent] });
  });

  it.each([
    ["summaries", {}, "listSummaries"], ["explicit number", { order: "number" }, "listOrder"],
    ["priority", { order: "priority" }, "listOrder"],
    ["workspace", { linked_workspace_id: "W" }, "listLinkedWorkspace"],
    ["tracker", { tracker_agent_id: "A" }, "listTracked"],
    ["review mode", { exclude_review_mode: "snapshot" }, "listReviewMode"],
    ["readiness", { held: false }, "listReadiness"],
  ])("does not dispatch %s without its original capability", async (_name, filters, capability) => {
    const caps = { ...allCapabilities, [capability]: false };
    expect(queries.summaryFiltersSupported(filters, caps)).toBe(false);
    const { view, rpc } = setupReader(filters, {}, caps);
    expect(await view.refresh()).toBe(false);
    expect(rpc).not.toHaveBeenCalled();
    view.dispose();
  });

  it.each(["unknown_selector", "number", "priority", "search", "cursor", "limit", "summary", "project_id"])(
    "retains existing unknown-filter validation for %s", (key) => {
      const rpc = vi.fn(), filters = { [key]: "unapproved" };
      expect(() => queries.canonicalSummaryFilters(filters)).toThrow("Unsupported summary selector");
      expect(() => readers.createTaskSummaryReader({ scope, filters, callRpc: rpc,
        capabilities: () => allCapabilities, sessionOf: () => ({}) })).toThrow("Unsupported summary selector");
      expect(rpc).not.toHaveBeenCalled();
    },
  );
});
