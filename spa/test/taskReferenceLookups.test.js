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
const deferred = () => { let resolve, reject; const promise = new Promise((yes, no) => { resolve = yes; reject = no; }); return { resolve, reject, promise }; };
const result = (number, found = true) => ({ project_id: scope.projectId, number,
  task: found ? { id: `task-${number}`, project_id: scope.projectId, number, title: `Target ${number}` } : null,
  identities: {}, basis: "opaque-basis" });
let cache, tracker, index, feed, reads, runtime, fence, host, page, stop;
const running = new Set();
const lookupCalls = () => live.context.rpc.mock.calls.filter(([method]) => method === "tasks.lookup");
const mounted = (content) => {
  host.innerHTML = `<div class="task-page-body markdown">${content}</div>`;
  const follower = runtime.mountTaskReferenceLookups(host, { ...scope, context: live.context });
  const wrapped = { ...follower, update: (content) => follower.update(typeof content === "string"
    ? { body: host.textContent, rows: [] } : content) };
  running.add(wrapped);
  return wrapped;
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

it("counts element-only DOM nodes and avoids whole-subtree selectors and ancestor searches", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const follower = mounted(`${"<i></i>".repeat(300)} #42`);
  const queries = vi.spyOn(host, "querySelectorAll");
  const ancestors = vi.spyOn(Element.prototype, "closest");
  try {
    await follower.update({ body: "#42", rows: [] });
    expect(queries).not.toHaveBeenCalled();
    expect(ancestors).not.toHaveBeenCalled();
    expect(lookupCalls()).toHaveLength(0);
  } finally { queries.mockRestore(); ancestors.mockRestore(); }
});

it("does not admit multiline references beyond the same bounded source horizon used for content identity", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const prefix = "x".repeat(4096);
  const follower = mounted(`<p>${prefix}</p><p>#42</p>`);
  await follower.update({ body: `${prefix}\n\n#42`, rows: [] });
  expect(lookupCalls()).toHaveLength(0);
  host.innerHTML = `<div class="task-page-body markdown"><p>${prefix}</p><p>#43</p></div>`;
  await follower.update({ body: `${prefix}\n\n#43`, rows: [] });
  expect(lookupCalls()).toHaveLength(0);
});

it("does not admit comment refs beyond the bounded source row horizon", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  host.innerHTML = '<li data-comment-id="late"><div class="task-comment-body markdown">#42</div></li>';
  const follower = runtime.mountTaskReferenceLookups(host, { ...scope, context: live.context }); running.add(follower);
  const rows = [...Array.from({ length: 256 }, () => ({ type: "event" })), { type: "comment", key: "late", body: "#42" }];
  await follower.update({ body: "", rows });
  expect(lookupCalls()).toHaveLength(0);
  rows[256] = { type: "comment", key: "late", body: "#43" };
  host.querySelector('.task-comment-body').textContent = "#43";
  await follower.update({ body: "", rows });
  expect(lookupCalls()).toHaveLength(0);
});

it("charges exact comment ID bytes and skips oversized imported identities", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const id = "x".repeat(20_000);
  host.innerHTML = `<li data-comment-id="${id}"><div class="task-comment-body markdown">#42</div></li>`;
  const follower = runtime.mountTaskReferenceLookups(host, { ...scope, context: live.context }); running.add(follower);
  await follower.update({ body: "", rows: [{ type: "comment", key: id, body: "#42" }] });
  expect(lookupCalls()).toHaveLength(0);
});

it("leaves NUL and literal replacement-character comment identities unadmitted without conflating their turns", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const pending = deferred(); live.context.rpc.mockImplementationOnce(() => pending.promise);
  const follower = runtime.mountTaskReferenceLookups(host, { ...scope, context: live.context }); running.add(follower);
  host.innerHTML = '<li data-comment-id="safe"><div class="task-comment-body markdown">#42</div></li>';
  void follower.update({ body: "", rows: [{ type: "comment", key: "safe", body: "#42" }] });
  await vi.waitFor(() => expect(lookupCalls()).toHaveLength(1));
  host.innerHTML = '<li data-comment-id="raw\u0000id"><div class="task-comment-body markdown">#43</div></li>';
  await follower.update({ body: "", rows: [{ type: "comment", key: "raw\u0000id", body: "#43" }] });
  pending.resolve(result(42));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect((await reads.readTaskLookup(scope, 42)).status).toBe("unknown");
  host.innerHTML = '<li data-comment-id="raw\ufffdid"><div class="task-comment-body markdown">#44</div></li>';
  await follower.update({ body: "", rows: [{ type: "comment", key: "raw\ufffdid", body: "#44" }] });
  expect(lookupCalls()).toHaveLength(1);
});

it("keeps adjacent above-safe JSON task numbers unknown instead of linking the old consumer's numeric alias", async () => {
  const firstRaw = "9007199254740992", secondRaw = "9007199254740993";
  const first = JSON.parse(firstRaw), second = JSON.parse(secondRaw);
  expect(first).toBe(second); // Actual Node JSON/Number loss, not an invented fixture coercion.
  const aliased = { id: "wrong-adjacent-task", number: first, title: "Wrong adjacent task" };
  expect([aliased].find((row) => Number(row.number) === Number(secondRaw))?.id).toBe(aliased.id);
  await tracker.writeTasksRecord(scope.deviceId, scope.projectId, tracker.tasksRecord([aliased]));
  const held = task({ id: "own-number-domain", project_id: scope.projectId, body: `See #${secondRaw}.`, watched: false });
  await tracker.writeTaskRecord(scope.deviceId, scope.projectId, held.id, tracker.taskRecord(held, []));
  const { mountTaskPage } = await import("../src/core/trackerTaskPage.js");
  page = mountTaskPage(host, { ...scope, projectKey, taskId: held.id, callRpc: live.context.rpc,
    catalog: () => ({ providers: [] }), feed: () => ({ projects: [], workspaces: [], items: [] }) });
  await vi.waitFor(() => expect(host.querySelector('.task-page-body')?.textContent).toContain(secondRaw));
  expect(host.querySelector('.task-page-body a.md-ref')).toBeNull();
  expect(host.querySelector('.task-page-body .md-ref-missing')).toBeNull();
  expect(lookupCalls()).toHaveLength(0);
});

it("coalesces many forced refreshes behind the same initial four in-flight lookups", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const initial = new Map([1, 2, 3, 4].map((number) => [number, deferred()]));
  live.context.rpc.mockImplementation((method, params) => {
    const first = initial.get(params.number);
    return lookupCalls().length <= 4 ? first.promise : Promise.resolve(result(params.number));
  });
  const follower = mounted("#1 #2 #3 #4 #5");
  const started = follower.update("initial");
  await vi.waitFor(() => expect(lookupCalls()).toHaveLength(4));
  const refreshes = Array.from({ length: 50 }, () => follower.refresh());
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(lookupCalls()).toHaveLength(4);
  for (const [number, pending] of initial) pending.resolve(result(number));
  await started; await Promise.all(refreshes);
  expect(lookupCalls()).toHaveLength(8);
  expect(lookupCalls().map(([, params]) => params.number)).toEqual([1, 2, 3, 4, 1, 2, 3, 4]);
});

it("drains initial siblings after one error before the explicitly queued trailing refresh", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const initial = new Map([1, 2, 3, 4].map((number) => [number, deferred()]));
  live.context.rpc.mockImplementation((method, params) => lookupCalls().length <= 4
    ? initial.get(params.number).promise : Promise.resolve(result(params.number)));
  const follower = mounted("#1 #2 #3 #4");
  const started = follower.update("initial");
  await vi.waitFor(() => expect(lookupCalls()).toHaveLength(4));
  const forced = follower.refresh();
  await Promise.resolve(); await Promise.resolve();
  initial.get(1).reject(new Error("refused"));
  await new Promise((resolve) => setTimeout(resolve, 20));
  expect(lookupCalls()).toHaveLength(4);
  for (const [number, pending] of initial) if (number !== 1) pending.resolve(result(number));
  await started; await forced;
  expect(lookupCalls()).toHaveLength(8);
});

it("does not hash oversized DOM comment IDs or tokenize oversized class attributes", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const oversized = "x".repeat(20_000);
  host.innerHTML = `<li data-comment-id="${oversized}"><div class="task-comment-body markdown">#42</div></li>
    <div id="oversized-class" class="${oversized} task-page-body markdown">#43</div>`;
  const element = host.querySelector('#oversized-class');
  const classTokens = vi.spyOn(element, "classList", "get");
  const originalHas = Set.prototype.has;
  const hashedOversized = [];
  const hashes = vi.spyOn(Set.prototype, "has").mockImplementation(function(value) {
    if (value === oversized) hashedOversized.push(value.length);
    return originalHas.call(this, value);
  });
  const follower = runtime.mountTaskReferenceLookups(host, { ...scope, context: live.context }); running.add(follower);
  try {
    await follower.update({ body: "#43", rows: [{ type: "comment", key: oversized, body: "#42" }] });
    expect(hashedOversized).toEqual([]);
    expect(classTokens).not.toHaveBeenCalled();
    expect(lookupCalls()).toHaveLength(0);
  } finally { hashes.mockRestore(); classTokens.mockRestore(); }
});

it("admits review-file references in the same fixed four-point budget without a full catalogue", async () => {
  runtime = await import("../src/core/taskReferenceLookups.js");
  const body = "[[#42:r3:all:src/a.js#old-L7]] #43 [[#44:r1:all:dir:src/b.js]] #45 #46";
  const follower = mounted(body);
  await follower.update("adopted-content");
  expect(lookupCalls().map(([, params]) => params.number)).toEqual([42, 43, 44, 45]);
  expect(await tracker.readTasksRecord(scope.deviceId, scope.projectId)).toBeNull();
  await cache.writeCached({ deviceId: scope.deviceId, entityId: scope.projectId, kind: "task-review", sub: "task-42" },
    { review: { snapshots: [{ number: 3, directories: [{ id: "review-dir" }] }] } });
  const resolve = () => index.referenceResolver({ place: scope }).review(42, 3, undefined, "src/a.js");
  await vi.waitFor(() => expect(resolve()).toMatchObject({ taskId: "task-42", directoryId: "review-dir" }));
  await follower.update("same-content");
  expect(lookupCalls()).toHaveLength(4);
  live.context.rpc.mockImplementation(async (_method, params) => result(params.number, params.number !== 42));
  await follower.refresh();
  await vi.waitFor(() => expect(resolve()).toBeNull());
  expect(lookupCalls().map(([, params]) => params.number)).toEqual([42, 43, 44, 45, 42, 43, 44, 45]);
});
