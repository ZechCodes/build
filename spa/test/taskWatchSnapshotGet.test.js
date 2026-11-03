// @vitest-environment jsdom
// Plain task GET compatibility through an actual details-feed owner.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";
import { fakeSession } from "./deviceSessionFixture.js";

const scope = { deviceId: "snapshot-get-contract", projectId: "proj-snapshot" };
const taskId = "task-snapshot";
const pair = { bridge_epoch: "snapshot-process", journal_incarnation: "snapshot-journal" };
const getCapability = "tasks.watchSnapshot.get.v1";
let runtime, feeds;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  feeds = [];
  runtime = {
    cache: await import("../src/core/localCache.js"),
    contexts: await import("../src/core/deviceContexts.js"),
    api: await import("../src/core/bridgeApi/index.js"),
    epoch: await import("../src/core/bridgeEpoch.js"),
    tracker: await import("../src/core/trackerCache.js"),
    summaries: await import("../src/core/taskSummaryCache.js"),
    watch: await import("../src/core/taskWatchAuthority.js"),
    feed: await import("../src/core/trackerTaskDetailsFeed.js"),
  };
});
afterEach(() => {
  for (const feed of feeds) feed.dispose();
  runtime.contexts.resetDeviceContexts();
  vi.restoreAllMocks();
});

function nativeTask(overrides = {}) {
  return task({ id: taskId, project_id: scope.projectId, number: 41,
    title: "Canonical GET body", body: "Full ordinary content remains useful",
    status: "ready", watched: true, watch_started_after: "te-05", read_through: "tc-10",
    attachments: [], identities: [], updated_at: "2026-10-30T20:00:00Z", ...overrides });
}

function pendingReply() {
  let resolve;
  return { promise: new Promise((yes) => { resolve = yes; }), resolve: (value) => resolve(value) };
}

async function greet(capabilities, { includeEpoch = true, identity = pair } = {}) {
  const session = fakeSession(scope.deviceId);
  session.bridgeEpoch = runtime.epoch.createBridgeEpoch();
  const context = runtime.contexts.adoptDeviceSession(session);
  const authority = runtime.contexts.greetingInFlight(context);
  const greeting = { api_version: "3.27.0", capabilities: [...(includeEpoch ? ["bridge.epoch"] : []), ...capabilities], ...identity };
  expect(session.bridgeEpoch.acceptGreeting(greeting, { isCurrent: authority.current })).toBe(true);
  const selection = runtime.api.selectAdapter(greeting);
  const adapter = selection.create(session.call);
  expect(runtime.contexts.adoptBridgeSelection(context, selection, adapter, authority)).toBe(context);
  if (includeEpoch) expect(session.bridgeEpoch.current()).toMatchObject({ ...identity, opening: 1 });
  else expect(session.bridgeEpoch.current()).toBeNull();
  return { session, context };
}

async function beginRead(capabilities, reply = { task: nativeTask(), timeline: [] }, greetingOptions) {
  const { session, context } = await greet(capabilities, greetingOptions);
  const requests = [], pending = [];
  session.call.mockImplementation((method, params) => {
    if (method !== "tasks.get") throw new Error(`Unexpected contract peer request: ${method}`);
    requests.push({ method, params: structuredClone(params) });
    const response = pendingReply();
    pending.push(response);
    return response.promise;
  });
  const feed = runtime.feed.createTrackerTaskDetailsFeed({ ...scope, callRpc: context.rpc });
  feeds.push(feed);
  await feed.updateTasks([nativeTask({ title: "Listed task" })]);
  await vi.waitFor(() => expect(requests).toHaveLength(1));
  return { session, context, requests, feed,
    answer(index = 0, answer = reply) {
      pending[index].resolve(answer);
    },
  };
}

async function awaitSaved(read, title = "Canonical GET body") {
  await vi.waitFor(() => expect(read.feed.read().get(taskId)?.task?.title).toBe(title));
  const detail = await runtime.tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  expect(detail.task.body).toBe("Full ordinary content remains useful");
  return (await runtime.cache.readCanonicalCached(runtime.summaries.summaryAddress(scope, taskId))).value;
}

it("uses plain GET parameters even when a greeting names an unsupported snapshot capability", async () => {
  const read = await beginRead([getCapability]);
  expect(read.requests[0]).toEqual({ method: "tasks.get", params: { task_id: taskId } });
  read.answer();
  await awaitSaved(read);
});

it.each([
  { label: "none", capabilities: [] },
  { label: "summary only", capabilities: ["tasks.watchSnapshot.v1"] },
  { label: "lookup only", capabilities: ["tasks.watchSnapshot.lookup.v1"] },
])(
  "UNITCONTRACT preserves legacy GET parameters when endpoint support is $label", async ({ capabilities }) => {
    const read = await beginRead(capabilities);
    expect(read.requests[0]).toEqual({ method: "tasks.get", params: { task_id: taskId } });
    read.answer();
    const saved = await awaitSaved(read);
    expect(saved.watch_uncertain).toBe(true);
    expect((await runtime.watch.readTaskWatchState(scope)).uncertain).toBe(true);
  });

it("keeps legacy GET usable without an epoch capability", async () => {
  const read = await beginRead([getCapability], undefined, { includeEpoch: false });
  expect(read.requests[0].params).toEqual({ task_id: taskId });
  read.answer();
  const saved = await awaitSaved(read);
  expect(saved.watch_uncertain).toBe(true);
});

it("keeps legacy GET usable with a long greeting identity", async () => {
  const read = await beginRead([getCapability], undefined, { identity: { ...pair, journal_incarnation: "é".repeat(65) } });
  expect(read.requests[0].params).toEqual({ task_id: taskId });
  read.answer();
  const saved = await awaitSaved(read);
  expect(saved.watch_uncertain).toBe(true);
  expect(read.session.bridgeEpoch.current().journal_incarnation).toBe("é".repeat(65));
});
