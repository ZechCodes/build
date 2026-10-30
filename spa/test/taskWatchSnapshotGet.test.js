// @vitest-environment jsdom
// UNITCONTRACT ONLY: c129/4511 typed GET replies served by a test peer. These
// tests exercise actual SPA greeting/GET-owner/cache paths, not backend SQL
// coherence or D3-08 genuine integration. No private proof/control is seeded.
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";
import { task } from "./trackerWireFixture.js";
import { fakeSession } from "./deviceSessionFixture.js";

const scope = { deviceId: "snapshot-get-contract", projectId: "proj-snapshot" };
const taskId = "task-snapshot";
const pair = { bridge_epoch: "snapshot-process", journal_incarnation: "snapshot-journal" };
const getCapability = "tasks.watchSnapshot.get.v1";
const unavailable = { watch: "coherence_unavailable", total: "coherence_unavailable",
  attention: "coherence_unavailable", activity: "coherence_unavailable" };
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

// Proof fields have stricter required-nullable semantics than native TaskView.
// Total's already-extracted suffix is deliberately distinct from its raw ID.
function proofReply({ sequence = "18446744073709551615", incarnation = pair } = {}) {
  return { task: { ...nativeTask(), watch_facts: {
    watch: { watched: true, watch_started_after: "te-05" },
    total: { read_through: "tc-10", effective_watch_floor: "05", unread_count: 0 },
    attention: { read_through: "tc-10", unread_agent_comments: 0, unread_mentions: 0 },
    activity: { session_started_ms: null, last_activity_ms: null },
  } }, timeline: [], watch_snapshot: { version: 1, incarnation: { ...incarnation }, sequence } };
}

function pendingReply() {
  let resolve;
  return { promise: new Promise((yes) => { resolve = yes; }), resolve: (value) => resolve(value) };
}

async function greet(capabilities) {
  const session = fakeSession(scope.deviceId);
  session.bridgeEpoch = runtime.epoch.createBridgeEpoch();
  const context = runtime.contexts.adoptDeviceSession(session);
  const authority = runtime.contexts.greetingInFlight(context);
  const greeting = { api_version: "3.27.0", capabilities: ["bridge.epoch", ...capabilities], ...pair };
  expect(session.bridgeEpoch.acceptGreeting(greeting, { isCurrent: authority.current })).toBe(true);
  const selection = runtime.api.selectAdapter(greeting);
  const adapter = selection.create(session.call);
  expect(runtime.contexts.adoptBridgeSelection(context, selection, adapter, authority)).toBe(context);
  expect(session.bridgeEpoch.current()).toMatchObject({ ...pair, opening: 1 });
  return { session, context };
}

async function beginRead(capabilities, reply = proofReply()) {
  const { session, context } = await greet(capabilities), pending = pendingReply();
  const requests = [];
  let servedProof = false;
  session.call.mockImplementation((method, params) => {
    if (method !== "tasks.get") throw new Error(`Unexpected contract peer request: ${method}`);
    requests.push({ method, params: structuredClone(params) });
    return pending.promise;
  });
  const feed = runtime.feed.createTrackerTaskDetailsFeed({ ...scope, callRpc: context.rpc });
  feeds.push(feed);
  await feed.updateTasks([nativeTask({ title: "Listed task" })]);
  await vi.waitFor(() => expect(requests).toHaveLength(1));
  return { session, context, requests, feed,
    answer() {
      // Strict endpoint contract: absent/false option returns legacy bytes.
      const optedIn = requests[0].params.watch_snapshot === true;
      servedProof = optedIn && Boolean(reply.watch_snapshot);
      pending.resolve(optedIn ? reply : { task: nativeTask(), timeline: [] });
    },
    servedProof: () => servedProof,
  };
}

async function awaitSaved(read) {
  await vi.waitFor(() => expect(read.feed.read().get(taskId)?.task?.title).toBe("Canonical GET body"));
  const detail = await runtime.tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  expect(detail.task.body).toBe("Full ordinary content remains useful");
  return (await runtime.cache.readCanonicalCached(runtime.summaries.summaryAddress(scope, taskId))).value;
}

it("UNITCONTRACT sends the independently advertised GET snapshot opt-in from an actual admitted details-feed read", async () => {
  const read = await beginRead([getCapability]);
  expect(read.requests[0]).toEqual({ method: "tasks.get", params: { task_id: taskId, watch_snapshot: true } });
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
    expect(read.servedProof()).toBe(false);
    expect(saved.watch_uncertain).toBe(true);
    expect((await runtime.watch.readTaskWatchState(scope)).uncertain).toBe(true);
  });

it("UNITCONTRACT accepts a first declared as-of proof without changing fresh sticky control uncertainty", async () => {
  expect(await runtime.watch.readTaskWatchState(scope)).toMatchObject({ uncertain: true, eligible: false });
  const read = await beginRead([getCapability]);
  const captured = await runtime.watch.readTaskWatchState(scope);
  expect(captured).toMatchObject({ uncertain: true, eligible: false, pendingCount: 0 });
  read.answer();
  const saved = await awaitSaved(read);
  // This precondition separates missing request opt-in from scope admission.
  expect(read.servedProof()).toBe(true);
  expect(saved).toMatchObject({ watch_uncertain: false, needs_unread_refresh: false });
  expect(saved.task).toMatchObject({ watched: true, unread_count: 0, unread_agent_comments: 0, unread_mentions: 0 });
  expect(saved.task.__task_watch_snapshot.scopes.watch.sequence).toBe("18446744073709551615");
  expect(await runtime.watch.readTaskWatchState(scope)).toEqual(captured);
});

it("UNITCONTRACT retains useful full content when opted-in GET reports all scopes coherence-unavailable", async () => {
  const reply = { task: { ...nativeTask(), watch_unavailable: unavailable }, timeline: [] };
  const read = await beginRead([getCapability], reply);
  read.answer();
  const saved = await awaitSaved(read);
  expect(read.servedProof()).toBe(false);
  expect(saved).toMatchObject({ watch_uncertain: true, needs_unread_refresh: true });
  expect(saved.task).not.toHaveProperty("__task_watch_snapshot");
  expect((await runtime.watch.readTaskWatchState(scope)).uncertain).toBe(true);
});

it("UNITCONTRACT cannot activate an unexpected reply incarnation even with maximum sequence", async () => {
  const read = await beginRead([getCapability], proofReply({ incarnation: {
    bridge_epoch: "unsolicited-process", journal_incarnation: "unsolicited-journal",
  } }));
  read.answer();
  const saved = await awaitSaved(read);
  expect(saved).toMatchObject({ watch_uncertain: true, needs_unread_refresh: true });
  expect(read.session.bridgeEpoch.current()).toMatchObject({ ...pair, opening: 1 });
  expect(saved.task).not.toHaveProperty("__task_watch_snapshot");
});
