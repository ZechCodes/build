import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import {
  agentTaskSummaryGroups, compareTaskSummaryNumber, compareTaskSummaryPriority,
  compareTaskSummaryUpdated, compareTaskSummaryAnchor, compareTaskSummaryAnchors,
  createTaskSummaryGroupState, retargetTaskSummaryGroups,
  captureTaskSummaryGroupRead, applyTaskSummaryGroupPage,
} from "../src/core/taskSummaryGroups.js";
import { agentTaskEntries } from "../src/core/trackerAgentTasks.js";
import { mergeKnownTaskHeads, taskHeadReadPlan } from "../src/core/taskOrderedStreams.js";

const fixture = JSON.parse(readFileSync(new URL("../../fixtures/api/v1/tasks.list.json", import.meta.url)));
const columns = ["backlog", "in_progress", "in_review", "done"].map((id) => ({ id }));
const options = (extra = {}) => ({ projectId: "proj-1", agentId: "agent-1", columns, ...extra });
const task = (number, extra = {}) => ({ id: `task-${number}`, number, ...extra });
const page = (tasks, next_cursor = null, basis = "opaque-basis") => ({ tasks, next_cursor, basis });
const read = (state, id = "assigned", continuation = false) => captureTaskSummaryGroupRead(state, id, { continuation });
const stream = (id, items, complete = false) => ({ id, items, complete });

// This deliberately models server selections, independent of the source's
// readiness grouping, so exhaustive parity catches missing static exclusions.
function matches(task, params) {
  if (params.assignee && JSON.stringify(task.assignee) !== JSON.stringify(params.assignee)) return false;
  if (params.exclude_assignee && JSON.stringify(task.assignee) === JSON.stringify(params.exclude_assignee)) return false;
  if (params.tracker_agent_id && !task.trackers.includes(params.tracker_agent_id)) return false;
  if (params.state && task.state !== params.state) return false;
  if (params.status && task.status !== params.status) return false;
  if (params.statuses && !params.statuses.includes(task.status)) return false;
  if (Object.hasOwn(params, "blocked") && params.blocked !== task.dependencies.blocked) return false;
  const undelivered = ["waiting", "queued", "provisioning", "failed"].includes(task.deferred_assignment?.state);
  return !Object.hasOwn(params, "undelivered") || params.undelivered === undelivered;
}

describe("static agent streams", () => {
  it("uses the exact pinned #900 fixture requests in both cached working modes", () => {
    const specs = fixture.exact_queries.agent_surface_streams.streams;
    const idle = agentTaskSummaryGroups(options());
    const working = agentTaskSummaryGroups(options({ working: true }));
    expect(idle.map(({ id }) => id)).toEqual(["assigned", "blocked", "tracked", "in_review", "done", "closed"]);
    expect(working.map(({ id }) => id)).toEqual(["working", "assigned", "blocked", "tracked", "in_review", "done", "closed"]);
    expect(working[0].params).toEqual(specs[0].params);
    expect(idle[0].params).toEqual(specs[1].params);
    expect(working[1].params).toEqual(specs[2].params);
    expect(idle.slice(1).map(({ params }) => params)).toEqual(specs.slice(3).map(({ params }) => params));
  });

  it("takes the full configured active columns and never issues an empty status selection", () => {
    const custom = [...columns, { id: "investigating" }, { id: "backlog" }];
    expect(agentTaskSummaryGroups(options({ columns: custom }))[0].params.statuses).toEqual(["backlog", "in_progress", "investigating"]);
    const historyOnly = agentTaskSummaryGroups(options({ columns: [{ id: "done" }, { id: "in_review" }] }));
    expect(historyOnly.map(({ id }) => id)).toEqual(["tracked", "in_review", "done", "closed"]);
  });

  it.each([false, true])("matches source group and priority order with disjoint membership (working=%s)", (working) => {
    const tasks = [];
    for (const state of ["open", "closed"]) for (const status of columns.map(({ id }) => id)) {
      for (const blocked of [false, true]) for (const deferred of [null, "waiting", "queued", "provisioning", "failed", "delivered"]) {
        for (const assignee of [null, { kind: "agent", agent_id: "agent-1" }, { kind: "agent", agent_id: "other" }]) {
          tasks.push(task(tasks.length + 1, { state, status, assignee, trackers: ["agent-1"],
            priority: ["none", "low", "medium", "high", "urgent"][tasks.length % 5],
            dependencies: { blocked }, deferred_assignment: deferred ? { state: deferred } : null }));
        }
      }
    }
    const source = [...tasks].sort(compareTaskSummaryNumber);
    const expected = agentTaskEntries(source, "agent-1", { working }).map(({ id, state }) => [id, state]);
    const actual = agentTaskSummaryGroups(options({ working })).flatMap(({ id, params }) =>
      tasks.filter((row) => matches(row, params)).sort(compareTaskSummaryPriority).map((row) => [row.id, id]));
    expect(actual).toEqual(expected);
    expect(new Set(actual.map(([id]) => id)).size).toBe(actual.length);
  });
});

describe("authoritative summary ordering", () => {
  it("orders priority then number, and updated milliseconds then number with missing rank zero", () => {
    const rows = [task(8, { priority: "none" }), task(2, { priority: "urgent" }), task(9, { priority: "high" }), task(3, { priority: "urgent" })];
    expect(rows.sort(compareTaskSummaryPriority).map(({ number }) => number)).toEqual([3, 2, 9, 8]);
    const updated = [task(7), task(6, { updated_at: "1970-01-01T00:00:00.000Z" }), task(2, { updated_at: "2026-01-01T00:00:00.123Z" }), task(3, { updated_at: "2026-01-01T00:00:00.123Z" })];
    expect(updated.sort(compareTaskSummaryUpdated).map(({ number }) => number)).toEqual([3, 2, 7, 6]);
  });

  it("uses only authoritative anchor metadata, null last, number descending within a project", () => {
    const rows = [task(99, { order_anchor_ms: null, created_at: "1970-01-01T00:00:00Z" }), task(4, { order_anchor_ms: 0 }), task(6, { order_anchor_ms: 10 }), task(7, { order_anchor_ms: 10 })];
    expect(rows.sort(compareTaskSummaryAnchor).map(({ number }) => number)).toEqual([4, 7, 6, 99]);
    expect(compareTaskSummaryAnchors(task(1, { order_anchor_ms: 10 }), task(999, { order_anchor_ms: 10 }))).toBe(0);
    expect(compareTaskSummaryAnchors(task(1, { order_anchor_ms: null }), task(999, { order_anchor_ms: null }))).toBe(0);
  });

  it("retains cross-project source order for tied anchors and blocks sparse unknown heads", () => {
    const streams = [stream("z-project", [task(2, { order_anchor_ms: 10 }), task(1, { order_anchor_ms: 10 })], true), stream("a-project", [task(99, { order_anchor_ms: 10 })], true), stream("exhausted", [], true)];
    expect(mergeKnownTaskHeads(streams, compareTaskSummaryAnchors, 10).items.map(({ number }) => number)).toEqual([2, 1, 99]);
    expect(mergeKnownTaskHeads([...streams, stream("sparse", [])], compareTaskSummaryAnchors, 10).items).toEqual([]);
    const unknown = Array.from({ length: 1000 }, (_, index) => stream(`p${index}`, []));
    expect(taskHeadReadPlan(unknown, 2)).toEqual(["p0", "p1"]);
    expect(mergeKnownTaskHeads(unknown, compareTaskSummaryAnchors, 10).complete).toBe(false);
  });

  it("deduplicates complete branch overlap without adding an ID tie-break", () => {
    const rows = [task(3, { priority: "high" }), task(2, { priority: "high" })];
    const result = mergeKnownTaskHeads([stream("a", rows, true), stream("b", rows, true)], compareTaskSummaryPriority, 10);
    expect(result.items).toEqual(rows);
    expect(result.complete).toBe(true);
  });
});

describe("cached observation retargeting and independent continuation", () => {
  it("keeps sparse continuation and exhausted state independent for every group", () => {
    let state = createTaskSummaryGroupState(options());
    state = applyTaskSummaryGroupPage(state, read(state), page([], "sparse-next"));
    state = applyTaskSummaryGroupPage(state, read(state, "tracked"), page([task(7)]));
    expect(state.groups.assigned).toMatchObject({ items: [], nextCursor: "sparse-next", complete: false, stale: false });
    expect(state.groups.tracked).toMatchObject({ items: [task(7)], nextCursor: null, complete: true });
    expect(read(state, "assigned", true).cursor).toBe("sparse-next");
    expect(captureTaskSummaryGroupRead(state, "tracked", { continuation: true })).toBeNull();
  });

  it("retargets cached working as a unit and rejects both older first-page and continuation replies", () => {
    let state = createTaskSummaryGroupState(options());
    const oldFirst = read(state);
    state = applyTaskSummaryGroupPage(state, oldFirst, page([task(9)], "next"));
    const oldNext = read(state, "assigned", true);
    state = retargetTaskSummaryGroups(state, options({ working: true }));
    expect(state.groups.assigned).toMatchObject({ items: [task(9)], stale: true, nextCursor: null });
    expect(state.groups.working.params.undelivered).toBe(false);
    expect(state.groups.assigned.params.undelivered).toBe(true);
    expect(applyTaskSummaryGroupPage(state, oldFirst, page([task(8)]))).toBe(state);
    expect(applyTaskSummaryGroupPage(state, oldNext, page([task(7)]))).toBe(state);
    expect(read(state, "assigned", true)).toBeNull();
    state = applyTaskSummaryGroupPage(state, read(state), page([task(6)], "fresh"));
    expect(state.groups.assigned.items).toEqual([task(6)]);
    expect(retargetTaskSummaryGroups(state, options({ working: true }))).toBe(state);
  });

  it("fences coordinator and roster changes even when static request parameters stay equal", () => {
    const initial = createTaskSummaryGroupState(options({ coordinatorId: "c1", rosterKey: "r1" }));
    const token = read(initial);
    const coordinator = retargetTaskSummaryGroups(initial, options({ coordinatorId: "c2", rosterKey: "r1" }));
    const roster = retargetTaskSummaryGroups(initial, options({ coordinatorId: "c1", rosterKey: "r2" }));
    expect(applyTaskSummaryGroupPage(coordinator, token, page([task(1)]))).toBe(coordinator);
    expect(applyTaskSummaryGroupPage(roster, token, page([task(1)]))).toBe(roster);
  });

  it("appends only an explicit matching cursor and basis, keeping the previous cache on a basis change", () => {
    let state = createTaskSummaryGroupState(options());
    state = applyTaskSummaryGroupPage(state, read(state), page([task(9)], "next", "basis-a"));
    const token = read(state, "assigned", true);
    expect(applyTaskSummaryGroupPage(state, token, page([task(8)], null, "basis-b"))).toBe(state);
    state = applyTaskSummaryGroupPage(state, token, page([task(9), task(8)], null, "basis-a"));
    expect(state.groups.assigned.items.map(({ number }) => number)).toEqual([9, 8]);
    expect(state.groups.assigned.complete).toBe(true);
    expect(applyTaskSummaryGroupPage(state, token, page([task(7)], null, "basis-a"))).toBe(state);
  });
});
