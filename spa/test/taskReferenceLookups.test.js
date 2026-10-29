/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";

const live = vi.hoisted(() => ({}));
vi.mock("../src/core/deviceContexts.js", async (original) => ({ ...await original(),
  contextFor: () => live.current ? live.context : null,
  deviceContextIdentity: () => live.identity,
  whenGreeted: async (context, dispatch) => {
    const session = live.session, greeting = context.greeted;
    await live.greetingWait;
    const stands = () => live.current && !live.away && live.session === session && context.greeted === greeting;
    return stands() ? { sent: dispatch(stands), stands } : null;
  },
}));
vi.mock("../src/core/deviceReconnect.js", () => ({
  deviceSession: () => live.session,
  deviceWatch: () => ({ away: () => live.away, reconnecting: () => false,
    moved: (listener) => { live.moved.add(listener); return () => live.moved.delete(listener); } }),
}));
vi.mock("../src/core/changeEvents.js", () => ({
  bridgeCapabilities: () => live.context?.adapter?.capabilities || {},
  onBridgeGreeted: (listener) => { live.greeted.add(listener); return () => live.greeted.delete(listener); },
  watchChanges: (spec) => { live.watches.add(spec); return { dispose: () => live.watches.delete(spec) }; },
}));

const scope = { deviceId: "device-ref", projectId: "project-ref" };
const projectKey = "device-ref/project-ref";
const deferred = () => { let resolve; const promise = new Promise((yes) => { resolve = yes; }); return { resolve, promise }; };
const result = (number, found = true) => ({ project_id: scope.projectId, number,
  task: found ? { id: `task-${number}`, project_id: scope.projectId, number, title: `Target ${number}` } : null,
  identities: {}, basis: "opaque-basis" });
let cache, tracker, index, feed, reads, runtime, fence, host, page, stop;
const running = new Set();
const lookupCalls = () => live.context.rpc.mock.calls.filter(([method]) => method === "tasks.lookup");
const mounted = (content) => {
  host.innerHTML = `<div class="task-page-body markdown">${content}</div>`;
  const follower = runtime.mountTaskReferenceLookups(host, { ...scope, context: live.context });
  running.add(follower);
  return follower;
};

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  live.current = true; live.identity = "identity"; live.away = false; live.session = {}; live.greetingWait = undefined;
  live.moved = new Set(); live.greeted = new Set(); live.watches = new Set();
  live.context = { deviceId: scope.deviceId, greeted: {}, active: () => live.current,
    adapter: { capabilities: { tasks: { lookup: true }, changes: { subscriptions: true, kinds: ["tasks"] } } },
    rpc: vi.fn(async (method, params) => method === "tasks.lookup" ? result(params.number) : new Promise(() => {})) };
  cache = await import("../src/core/localCache.js");
  tracker = await import("../src/core/trackerCache.js");
  index = await import("../src/core/referenceIndex.js");
  feed = await import("../src/core/referenceIndexFeed.js");
  reads = await import("../src/core/taskAggregateReads.js");
  fence = await import("../src/core/taskMutationFence.js");
  host = document.createElement("div"); document.body.append(host);
  stop = feed.feedReferenceIndex({ subscribeFeed(listener) {
    listener({ projects: [{ id: scope.projectId, deviceId: scope.deviceId, projectKey }], workspaces: [], items: [] }); return () => {};
  } });
});
afterEach(() => { page?.dispose(); page = null; for (const one of running) one.dispose(); running.clear(); stop?.(); host.remove(); });

it("admits four fixed unknown refs from actual mounted task-page content and never refills on index paints", async () => {
  const held = task({ id: "task-own", project_id: scope.projectId, body: "#11 #12 #13 #14 #15 `#16`", watched: false });
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, held.id, tracker.taskRecord(held, []));
  const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
  page = mountTaskPage(host, { ...scope, projectKey, taskId: held.id, callRpc: live.context.rpc,
    catalog: () => ({ providers: [] }), feed: () => ({ projects: [], workspaces: [], items: [] }) });
  await vi.waitFor(() => expect(lookupCalls()).toHaveLength(4));
  await vi.waitFor(() => expect(host.querySelectorAll('.task-page-body a.md-ref')).toHaveLength(4));
  expect(lookupCalls().map(([, params]) => params.number)).toEqual([11, 12, 13, 14]);
  expect(host.querySelector('.task-page-body').textContent).toContain("#15");
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, held.id, tracker.taskRecord({ ...held, title: "Other title" }, []));
  await new Promise((resolve) => setTimeout(resolve, 30));
  expect(lookupCalls()).toHaveLength(4);
});

it("keeps an off-page reference unknown until a real task-page lookup confirms missing", async () => {
  const held = task({ id: "task-own", project_id: scope.projectId, body: "See #42.", watched: false });
  const taskOrder = await import("../src/core/taskReadOrder.js");
  await cache.writeCached(tracker.tasksAddress(scope.deviceId, scope.projectId), { tasks: [{ id: "other", number: 100 }] });
  await cache.writeCached(taskOrder.readsAddress(tracker.tasksAddress(scope.deviceId, scope.projectId)), {
    stretches: [{ above: Infinity, through: 100, read: 1 }], rows: {},
  });
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, held.id, tracker.taskRecord(held, []));
  const pending = deferred();
  live.context.rpc.mockImplementation((method) => method === "tasks.lookup" ? pending.promise : new Promise(() => {}));
  const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
  page = mountTaskPage(host, { ...scope, projectKey, taskId: held.id, callRpc: live.context.rpc,
    catalog: () => ({ providers: [] }), feed: () => ({ projects: [], workspaces: [], items: [] }) });
  await vi.waitFor(() => expect(lookupCalls()).toHaveLength(1));
  expect(host.querySelector('.task-page-body').textContent).toContain("#42");
  expect(host.querySelector('.task-page-body .md-ref-missing')).toBeNull();
  pending.resolve(result(42, false));
  await vi.waitFor(() => expect(host.querySelector('.task-page-body .md-ref-missing')?.textContent).toBe("#42"));
  expect(lookupCalls()).toHaveLength(1);
});

it("rejects a reply for a superseded mounted content turn", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const old = deferred();
  live.context.rpc.mockImplementationOnce(() => old.promise);
  const follower = mounted("#42");
  void follower.update("old-content");
  await vi.waitFor(() => expect(lookupCalls()).toHaveLength(1));
  host.innerHTML = '<div class="task-page-body markdown">#43</div>';
  await follower.update("new-content");
  old.resolve(result(42));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect((await reads.readTaskLookup(scope, 42)).status).toBe("unknown");
  expect((await reads.readTaskLookup(scope, 43)).status).toBe("found");
});

it("excludes code, pre and existing anchors and enforces byte/node scan budgets", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const follower = mounted('<code>#1</code><pre>#2</pre><a href="#existing">#3</a><p>#4 #5</p>');
  await follower.update("one");
  expect(lookupCalls().map(([, params]) => params.number)).toEqual([4, 5]);
  host.innerHTML = `<div class="task-page-body markdown">${"x".repeat(20_000)} #99</div>`;
  await follower.update("large");
  expect(lookupCalls().map(([, params]) => params.number)).toEqual([4, 5]);
  host.innerHTML = `<div class="task-page-body markdown">${"<i>text</i>".repeat(300)} #98</div>`;
  await follower.update("nodes");
  expect(lookupCalls().map(([, params]) => params.number)).toEqual([4, 5]);
  const rows = Array.from({ length: 256 }, () => ({ type: "event" }));
  rows.push({ get type() { throw new Error("Content fingerprint exceeded its row budget"); } });
  expect(() => follower.update({ body: "same adopted text", rows })).not.toThrow();
});

it("keeps a lookup confirmed null distinct from unknown partial membership and retains it on error", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  live.context.rpc.mockResolvedValueOnce(result(42, false)).mockRejectedValueOnce(new Error("offline"));
  const follower = mounted("#42");
  await follower.update("one");
  await vi.waitFor(() => expect(index.referenceResolver({ place: scope }).task(42)).toBeNull());
  await follower.refresh();
  expect((await reads.readTaskLookup(scope, 42)).status).toBe("missing");
  expect(index.referenceResolver({ place: scope }).task(42)).toBeNull();
  expect(lookupCalls()).toHaveLength(2);
});

it("rejects old content/session and mutation responses, suppresses pending dispatch and refreshes after acknowledgment", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const old = deferred(), write = deferred();
  live.context.rpc.mockImplementationOnce(() => old.promise);
  const follower = mounted("#42");
  void follower.update("one");
  await vi.waitFor(() => expect(lookupCalls()).toHaveLength(1));
  const mutation = fence.wrapTaskMutationCall(scope.deviceId, "tasks.watch", () => write.promise);
  await follower.refresh();
  expect(lookupCalls()).toHaveLength(1);
  old.resolve(result(42));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect((await reads.readTaskLookup(scope, 42)).status).toBe("unknown");
  write.resolve({}); await mutation;
  await vi.waitFor(async () => expect((await reads.readTaskLookup(scope, 42)).status).toBe("found"));
  expect(lookupCalls()).toHaveLength(2);
  const stale = deferred(); live.context.rpc.mockImplementationOnce(() => stale.promise);
  void follower.refresh(); await vi.waitFor(() => expect(lookupCalls()).toHaveLength(3));
  live.session = {}; stale.resolve(result(42, false));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect((await reads.readTaskLookup(scope, 42)).status).toBe("found");
});

it("denies unsupported greetings and disposed content without catalogue or bootstrap lookups", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  expect(lookupCalls()).toHaveLength(0);
  live.context.adapter.capabilities.tasks = { counts: true, listSummaries: true };
  const follower = mounted("#42");
  await follower.update("one"); expect(lookupCalls()).toHaveLength(0);
  live.context.adapter.capabilities.tasks.lookup = true;
  follower.dispose(); await follower.refresh(); expect(lookupCalls()).toHaveLength(0);
});
