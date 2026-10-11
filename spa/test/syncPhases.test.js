import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { liveFeedSnapshot } from "../src/core/feedMerge.js";
import { createSyncPhases } from "../src/core/syncPhases.js";
import { createBackgroundStreams } from "../src/core/syncBackgroundStreams.js";
import { registerSyncDiff, registerSyncRead, registerSyncThread } from "../src/core/syncVisibleScopes.js";

vi.mock("../src/core/localCache.js", () => ({ cachedSubKeys: async () => [] }));

const address = (entityId, kind, sub = "") => ({ deviceId: "dev-1", entityId, kind, sub });
const sourceId = 'workspace:["ws-1","secondary"]';
const routeTo = (number) => ({
  name: "workspace", deviceId: "dev-1", projectId: "p1", workspaceId: `ws-${number}`, agent: `ag-${number}-b`,
});
const gate = () => {
  let release;
  const promise = new Promise((resolve) => { release = resolve; });
  return { promise, release };
};

let events, disposers, route, readers;
const event = (entityId, kind, priority, sub = "") => events.push({ entityId, kind, priority, sub });
const mounted = (entityId, kind, sub = "", active = () => true, read = null) => {
  const callback = read || (() => event(entityId, kind, "foreground", sub));
  disposers.push(registerSyncRead(() => address(entityId, kind, sub), callback, active));
  return callback;
};

function fixture(recoverWorkingDiff = false, backgroundStreams = null) {
  const rows = [1, 2].map((number) => ({
    kind: "branch", run_id: `run-${number}`, worktree_id: `wt-${number}`, branch: `build/${number}`,
    project_id: "p1", workspace_id: `ws-${number}`, state: "building",
    agents: ["a", "b"].map((suffix) => ({ id: `ag-${number}-${suffix}` })),
  }));
  const workspaces = rows.map((row, index) => ({
    id: `ws-${index + 1}`, project_id: "p1", entity_id: row.run_id, status: "ready",
  }));
  const view = liveFeedSnapshot({ items: rows }, { projects: [{ project_id: "p1" }] }, { workspaces }, "dev-1");
  const context = { deviceId: "dev-1", active: () => true, recoverWorkingDiff, backgroundStreams };
  return createSyncPhases({
    context, view, route: () => route,
    pass: { order: ["run-1", "run-2"], rows: new Map(view.items.map((row) => [row.run_id, row])) },
    entityContext: () => context,
    readers,
    tasks: {
      first: async (projectId) => event(projectId, "tasks-first", "foreground"),
      rest: async (projectId) => event(projectId, "tasks-rest", "background"),
    },
  });
}

beforeEach(() => {
  events = [];
  disposers = [];
  route = routeTo(1);
  const read = (kind) => async (_context, entityId, _row, priority) => event(entityId, kind, priority);
  readers = {
    scope: (row) => ({ run_id: row.run_id }),
    status: read("status"), terminals: read("terminals"), log: read("log"), unpushed: read("unpushed"),
    visibleDiff: read("diff"),
    tree: async (_context, entityId, _row, path, priority) => event(entityId, "tree", priority, path),
    thread: async (_context, entityId, agent, priority) => event(entityId, "thread", priority, agent.id),
    extras: async (_context, entityId) => { event(entityId, "extras", "background"); return true; },
  };
});

afterEach(() => disposers.forEach((dispose) => dispose()));

const microtasks = async () => { for (let turn = 0; turn < 500; turn += 1) await Promise.resolve(); };

describe("foreground record scopes", () => {
  it("promotes every read of the selected record and keeps unrelated records in background", () => {
    const phases = fixture();
    const background = { priority: "background" };
    for (const [entityId, kind, sub] of [
      ["run-1", "thread", "ag-1-b"], ["run-1", "status", ""],
      ["run-1", "tree", ""], ["p1", "tasks-first", ""],
    ]) {
      expect(phases.priorityForRead(address(entityId, kind, sub), background)?.priority).not.toBe("background");
    }
    for (const [entityId, kind, sub] of [
      ["run-1", "thread", "ag-1-a"], ["run-2", "thread", "ag-2-b"],
      ["run-1", "tree", "hidden"], ["run-1", "file", "a.js"],
      ["run-1", "patch", "hash"], ["p1", "tasks-rest", "cursor"],
    ]) {
      expect(phases.priorityForRead(address(entityId, kind, sub), background)?.priority).toBe("background");
    }
    route = routeTo(2);
    expect(phases.priorityForRead(address("run-1", "thread", "ag-1-b"), background)?.priority).toBe("background");
    expect(phases.priorityForRead(address("run-2", "thread", "ag-2-b"), background)?.priority).not.toBe("background");
  });
});

describe("foreground scope supersession", () => {
  it("wakes a selected checkout queued behind two push reads when the planner starts its foreground turn", async () => {
    const streams = createBackgroundStreams().forContext({ deviceId: "dev-1", session: {}, requestScope: {}, lifetime: 1 });
    const held = gate(), queued = gate();
    const sideRead = () => streams.read({ active: () => true, priority: () => ({ priority: "background" }), load: () => held.promise });
    const pushes = [sideRead(), sideRead()];
    let phases;
    route = { name: "inbox", deviceId: "dev-1" };
    readers.scope = (row) => row ? { run_id: row.run_id } : null;
    readers.status = (_context, entityId, _row, priority) => {
      queued.release();
      return streams.read({
        active: () => true,
        priority: () => phases.priorityForRead("git.status", { run_id: entityId }, priority === "background" ? { priority } : {}),
        before: () => phases.beforeBackgroundRead("git.status", { run_id: entityId }),
        load: (envelope) => event(entityId, "status", envelope?.priority || "foreground"),
      });
    };
    phases = fixture(false, streams);
    const pass = phases.run();
    await queued.promise;
    await microtasks();
    route = routeTo(1);
    const selected = phases.foreground();
    await microtasks();
    const dispatched = events.some((item) => item.entityId === "run-1" && item.kind === "status" && item.priority === "foreground");
    held.release();
    await Promise.all([...pushes, pass, selected]);
    expect(dispatched).toBe(true);
  });

  it("lets a queued recovery diff cross its own foreground barrier without a Changes pane", async () => {
    const held = gate(), asked = gate();
    let phases;
    readers.visibleDiff = async (_context, entityId, _row, priority) => {
      asked.release();
      await phases.beforeBackgroundRead("run.diff", { run_id: entityId });
      event(entityId, "diff", priority);
      await held.promise;
    };
    phases = fixture(true);
    const pass = phases.run();
    await asked.promise;
    await microtasks();
    const beforeRelease = events.slice();
    held.release();
    await pass;
    expect(beforeRelease.some((item) => item.entityId === "run-1" && item.kind === "diff")).toBe(true);
    expect(phases.priorityForRead("run.diff", { run_id: "run-1" }, { priority: "background" })?.priority).not.toBe("background");
  });

  it("promotes a mounted recovery diff on this device when the route belongs to another device", async () => {
    route = { ...routeTo(1), deviceId: "dev-2" };
    disposers.push(registerSyncDiff(address("run-2", "diff")));
    const phases = fixture(true);
    expect(phases.priorityForRead("run.diff", { run_id: "run-2" }, { priority: "background" })?.priority).not.toBe("background");
    expect(phases.priorityForRead("run.diff", { run_id: "run-1" }, { priority: "background" })?.priority).toBe("background");
    await phases.run();
    expect(events[0]).toEqual({ entityId: "run-2", kind: "diff", priority: "foreground", sub: "" });
  });

  it("wakes a background loader's barrier when that loader becomes the selected scope", async () => {
    const heldLookup = gate(), lookupStarted = gate(), heldOther = gate(), otherStarted = gate();
    let phases;
    route = { name: "inbox", deviceId: "dev-1" };
    readers.status = async (_context, entityId, _row, priority) => {
      if (entityId === "run-1" && priority === "background") {
        lookupStarted.release();
        await heldLookup.promise;
        await phases.beforeBackgroundRead("git.status", { run_id: entityId });
      }
      event(entityId, "status", priority);
      // The second stream may already be warming B when navigation selects
      // it. Its foreground turn joins that pending read and owns the barrier.
      if (entityId === "run-2") { otherStarted.release(); await heldOther.promise; }
    };
    phases = fixture();
    const pass = phases.run();
    const outcome = pass.then(() => null, (error) => error);
    await lookupStarted.promise;
    route = routeTo(2);
    const second = phases.foreground();
    await otherStarted.promise;
    heldLookup.release();
    await microtasks();
    route = routeTo(1);
    const first = phases.foreground();
    await microtasks();
    const dispatched = events.some((item) => item.entityId === "run-1" && item.kind === "status");
    heldOther.release();
    await Promise.all([outcome, second, first]);
    expect(dispatched).toBe(true);
    expect(await outcome).toBeNull();
  });

  it("starts B while A's foreground status is held, and keeps inbox behind B", async () => {
    const heldA = gate(), askedA = gate(), heldB = gate();
    const status = readers.status, thread = readers.thread;
    readers.status = async (...args) => {
      await status(...args);
      if (args[1] === "run-1") { askedA.release(); await heldA.promise; }
    };
    readers.thread = async (...args) => {
      await thread(...args);
      if (args[2].id === "ag-2-b") await heldB.promise;
    };
    const phases = fixture();
    const pass = phases.run();
    await askedA.promise;
    route = routeTo(2);
    const current = phases.foreground();
    const joined = phases.foreground();
    expect(joined).toBe(current);
    await microtasks();
    const startedB = events.filter((item) => item.sub === "ag-2-b");
    heldA.release();
    await microtasks();
    const whileB = events.slice();
    heldB.release();
    await Promise.all([pass, current, joined]);
    expect(startedB).toHaveLength(1);
    expect(startedB[0].priority).toBe("foreground");
    expect(whileB.filter((item) => item.kind === "thread").map((item) => item.sub)).toEqual(["ag-1-b", "ag-2-b"]);
  });

  it("lets the new scope and inbox progress before the superseded status answers", async () => {
    const held = gate(), asked = gate();
    const status = readers.status;
    readers.status = async (...args) => {
      await status(...args);
      if (args[1] === "run-1") { asked.release(); await held.promise; }
    };
    const phases = fixture();
    const pass = phases.run();
    await asked.promise;
    route = routeTo(2);
    const current = phases.foreground();
    await microtasks();
    const beforeRelease = events.slice();
    held.release();
    await Promise.all([pass, current]);
    expect(beforeRelease.filter((item) => item.kind === "thread")).toHaveLength(4);
    expect(beforeRelease.some((item) => item.kind === "tasks-rest")).toBe(true);
    expect(beforeRelease.some((item) => item.entityId === "run-1" && item.kind === "tree" && item.priority === "foreground")).toBe(false);
  });

  it("deduplicates in-flight reads across A to B to A and abandons old drains", async () => {
    const heldA = gate(), askedA = gate(), heldB = gate();
    const status = readers.status, thread = readers.thread;
    readers.status = async (...args) => {
      await status(...args);
      if (args[1] === "run-1") { askedA.release(); await heldA.promise; }
    };
    readers.thread = async (...args) => {
      await thread(...args);
      if (args[2].id === "ag-2-b") await heldB.promise;
    };
    const phases = fixture();
    const pass = phases.run();
    await askedA.promise;
    route = routeTo(2);
    const duringB = phases.foreground();
    await microtasks();
    const bStarted = events.some((item) => item.sub === "ag-2-b");
    route = routeTo(1);
    const backToA = phases.foreground();
    await microtasks();
    const aTreesWhileStatusHeld = events.filter((item) => item.entityId === "run-1" && item.kind === "tree");
    heldB.release();
    await microtasks();
    const bTreesAfterItsRead = events.filter((item) => item.entityId === "run-2" && item.kind === "tree");
    heldA.release();
    await Promise.all([pass, duringB, backToA]);
    expect(bStarted).toBe(true);
    expect(aTreesWhileStatusHeld).toHaveLength(0);
    expect(bTreesAfterItsRead).toHaveLength(0);
    expect(events.filter((item) => item.entityId === "run-1" && item.kind === "status")).toHaveLength(1);
    expect(events.filter((item) => item.sub === "ag-2-b")).toHaveLength(1);
  });

  it("preempts a held source when remembered thread and mounted visibility change without a URL change", async () => {
    let selected = "ag-1-b", showingOld = true;
    const held = gate(), asked = gate();
    disposers.push(registerSyncThread(() => ({ deviceId: "dev-1", entityId: selected === "ag-1-b" ? "run-1" : "run-2", agentId: selected })));
    mounted(sourceId, "status", "", () => showingOld, async () => { event(sourceId, "status", "foreground"); asked.release(); await held.promise; });
    const newSource = 'workspace:["ws-1","new-source"]';
    mounted(newSource, "tree", "", () => !showingOld);
    const phases = fixture();
    const pass = phases.run();
    await asked.promise;
    selected = "ag-2-b";
    showingOld = false;
    const current = phases.foreground();
    await microtasks();
    const beforeRelease = events.slice();
    held.release();
    await Promise.all([pass, current]);
    expect(beforeRelease.some((item) => item.sub === "ag-2-b" && item.priority === "foreground")).toBe(true);
    expect(beforeRelease.some((item) => item.entityId === newSource)).toBe(true);
  });
});

describe("mounted source reads in foreground phases", () => {
  it("validates mounted source Changes on recovery when the route is on another device", async () => {
    route = { ...routeTo(1), deviceId: "dev-2" };
    for (const kind of ["status", "tree", "log", "diff"]) mounted(sourceId, kind);
    await fixture(true).run();
    expect(events.filter((item) => item.entityId === sourceId)).toEqual([
      { entityId: sourceId, kind: "diff", priority: "foreground", sub: "" },
    ]);
  });

  it("reads source status, shown trees, history and diff after the open thread/scope and before inbox", async () => {
    disposers.push(registerSyncDiff(address("run-1", "diff")));
    // Registration order is incidental; the planner owns foreground ordering.
    for (const kind of ["diff", "unpushed", "log", "tree", "status"]) mounted(sourceId, kind);
    mounted(sourceId, "tree", "src");
    await fixture().run();

    const sourceEvents = events.filter((item) => item.entityId === sourceId);
    expect(sourceEvents.map(({ kind, sub }) => [kind, sub])).toEqual([
      ["status", ""], ["tree", ""], ["tree", "src"], ["log", ""], ["unpushed", ""], ["diff", ""],
    ]);
    expect(events[0]).toEqual({ entityId: "run-1", kind: "thread", priority: "foreground", sub: "ag-1-b" });
    const sourceStart = events.findIndex((item) => item.entityId === sourceId);
    const routedDiff = events.findIndex((item) => item.entityId === "run-1" && item.kind === "diff");
    const sourceEnd = events.findLastIndex((item) => item.entityId === sourceId);
    const firstTask = events.findIndex((item) => item.kind === "tasks-first");
    const otherThread = events.findIndex((item) => item.kind === "thread" && item.sub !== "ag-1-b");
    expect(sourceStart).toBeGreaterThan(routedDiff);
    expect(sourceEnd).toBeLessThan(firstTask);
    expect(sourceEnd).toBeLessThan(otherThread);
    expect(sourceEvents.every((item) => item.priority === "foreground")).toBe(true);
  });

  it("leaves reads for board entities to the core readers", async () => {
    const duplicate = vi.fn();
    for (const kind of ["status", "tree", "log", "unpushed", "diff"]) mounted("run-1", kind, "", () => true, duplicate);
    disposers.push(registerSyncDiff(address("run-1", "diff")));
    await fixture().run();
    expect(duplicate).not.toHaveBeenCalled();
    expect(events.filter((item) => item.entityId === "run-1" && item.kind === "status")).toHaveLength(1);
    expect(events.filter((item) => item.entityId === "run-1" && item.kind === "diff")).toHaveLength(1);
  });

  it("excludes hidden callbacks and callbacks on another device", async () => {
    const hidden = vi.fn();
    const otherDevice = vi.fn();
    mounted(sourceId, "tree", "hidden", () => false, hidden);
    disposers.push(registerSyncRead(() => ({ ...address(sourceId, "tree"), deviceId: "dev-2" }), otherDevice));
    const shown = vi.fn(() => event(sourceId, "tree", "foreground"));
    mounted(sourceId, "tree", "", () => true, shown);
    await fixture().run();
    expect(hidden).not.toHaveBeenCalled();
    expect(otherDevice).not.toHaveBeenCalled();
    expect(shown).toHaveBeenCalledOnce();
  });

  it("starts a newly shown source before a dispatched background read answers", async () => {
    let shown = false;
    const sourceRead = vi.fn(() => event(sourceId, "status", "foreground"));
    mounted(sourceId, "status", "", () => shown, sourceRead);
    const held = gate();
    const asked = gate();
    readers.status = async (_context, entityId, _row, priority) => {
      event(entityId, "status", priority);
      if (entityId === "run-2" && priority === "background") { asked.release(); await held.promise; }
    };
    const phases = fixture();
    const pass = phases.run();
    await asked.promise;
    shown = true;
    // #504: the newly shown source belongs to ws-1; ws-2's held read stays unrelated.
    route = { ...route, sourceId: "secondary" };
    try {
      await phases.foreground();
      expect(sourceRead).toHaveBeenCalledOnce();
      expect(events.at(-1).entityId).toBe(sourceId);
    } finally {
      held.release();
      await pass;
    }
  });

  it("continues the inbox and background pass after a mounted source read refuses", async () => {
    const refuse = vi.fn(async () => { throw new Error("source disappeared"); });
    mounted(sourceId, "status", "", () => true, refuse);
    const after = vi.fn(() => event(sourceId, "tree", "foreground"));
    mounted(sourceId, "tree", "", () => true, after);
    await expect(fixture().run()).resolves.toBe(true);
    expect(refuse).toHaveBeenCalledOnce();
    expect(after).toHaveBeenCalledOnce();
    expect(events.some((item) => item.kind === "tasks-rest")).toBe(true);
    expect(events.some((item) => item.entityId === "run-2" && item.kind === "extras")).toBe(true);
  });
});
