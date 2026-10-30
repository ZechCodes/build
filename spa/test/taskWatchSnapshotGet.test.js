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
  const { session, context } = await greet(capabilities);
  const requests = [], pending = [], served = [];
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
      // Strict endpoint contract: absent/false option returns legacy bytes.
      const optedIn = requests[index].params.watch_snapshot === true;
      served[index] = optedIn && Boolean(answer.watch_snapshot);
      pending[index].resolve(optedIn ? answer : { task: nativeTask(), timeline: [] });
    },
    servedProof: (index = 0) => served[index],
  };
}

async function awaitSaved(read, title = "Canonical GET body") {
  await vi.waitFor(() => expect(read.feed.read().get(taskId)?.task?.title).toBe(title));
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
  expect(saved.task.__task_watch_snapshot?.scopes?.watch?.sequence).toBe("18446744073709551615");
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


async function refreshReply(read, reply) {
  const index = read.requests.length;
  const refreshed = read.feed.refresh(taskId);
  await vi.waitFor(() => expect(read.requests).toHaveLength(index + 1));
  read.answer(index, reply);
  await refreshed;
  return awaitSaved(read, reply.task.title);
}

it("UNITCONTRACT keeps a captured same-task pending snapshot historical after settlement and requires fresh same-sequence recovery", async () => {
  const owner = await runtime.watch.beginTaskWatchMutation(scope, taskId, { watched: false, metadataOnly: true });
  expect(owner).not.toBeNull();
  const read = await beginRead([getCapability]);
  expect((await runtime.watch.readTaskWatchState(scope)).pendingCount).toBe(1);
  expect(await runtime.watch.settleTaskWatchMutation(owner, { rollback: true })).toBe(true);
  const settled = await runtime.watch.readTaskWatchState(scope);
  expect(settled).toMatchObject({ pendingCount: 0, uncertain: true });
  read.answer();
  const blocked = await awaitSaved(read);
  expect(read.servedProof()).toBe(true);
  expect(blocked).toMatchObject({ watch_uncertain: true, needs_unread_refresh: true });
  expect(blocked.task.__task_watch_snapshot?.scopes?.watch).toMatchObject({
    sequence: "18446744073709551615", admission: "historical",
  });
  // Merely rereading or reaching pending0 must not promote that old source.
  expect((await runtime.cache.readCanonicalCached(runtime.summaries.summaryAddress(scope, taskId))).value.watch_uncertain).toBe(true);
  const recovered = await refreshReply(read, proofReply());
  expect(read.servedProof(1)).toBe(true);
  expect(recovered).toMatchObject({ watch_uncertain: false, needs_unread_refresh: false });
  expect(recovered.task.__task_watch_snapshot?.scopes?.watch).toMatchObject({
    sequence: "18446744073709551615", admission: "selected",
  });
  expect(await runtime.watch.readTaskWatchState(scope)).toEqual(settled);
});

it("UNITCONTRACT admits a first positive proof under unrelated sticky uncertainty without settling that task's history", async () => {
  const other = await runtime.watch.beginTaskWatchMutation(scope, "unrelated-task", { watched: false, metadataOnly: true });
  expect(other).not.toBeNull();
  expect(await runtime.watch.settleTaskWatchMutation(other, { rollback: true })).toBe(true);
  const state = await runtime.watch.readTaskWatchState(scope);
  expect(state).toMatchObject({ pendingCount: 0, uncertain: true, eligible: false });
  const order = await import("../src/core/taskReadOrder.js");
  const noteAddress = order.taskWatchIntentAddress(scope, "unrelated-task");
  const before = await runtime.cache.readCanonicalCached(noteAddress);
  expect(before.value.latest_watch_intent).toMatchObject({ taskId: "unrelated-task", disposition: "rejected" });
  const read = await beginRead([getCapability]);
  read.answer();
  const saved = await awaitSaved(read);
  expect(read.servedProof()).toBe(true);
  expect(saved).toMatchObject({ watch_uncertain: false, needs_unread_refresh: false });
  expect(await runtime.cache.readCanonicalCached(noteAddress)).toEqual(before);
  expect(await runtime.watch.readTaskWatchState(scope)).toEqual(state);
});

it("UNITCONTRACT keeps a valid reply historical when the same task becomes pending after capture", async () => {
  const read = await beginRead([getCapability]);
  const owner = await runtime.watch.beginTaskWatchMutation(scope, taskId, { watched: false, metadataOnly: true });
  expect(owner).not.toBeNull();
  const pending = await runtime.watch.readTaskWatchState(scope);
  expect(pending.pendingCount).toBe(1);
  try {
    read.answer();
    const saved = await awaitSaved(read);
    expect(read.servedProof()).toBe(true);
    expect(saved).toMatchObject({ watch_uncertain: true, needs_unread_refresh: true });
    expect(saved.task.__task_watch_snapshot?.scopes?.watch?.admission).toBe("historical");
    expect(await runtime.watch.readTaskWatchState(scope)).toEqual(pending);
  } finally {
    await runtime.watch.settleTaskWatchMutation(owner, { rollback: true });
  }
});

it("UNITCONTRACT refuses captured canonical authority across an unowned same-lineage ABA without losing useful body", async () => {
  const seed = nativeTask();
  await runtime.tracker.writeTasksRecord(scope.deviceId, scope.projectId, runtime.tracker.tasksRecord([seed]));
  const read = await beginRead([getCapability]);
  const order = await import("../src/core/taskReadOrder.js");
  const control = order.taskWatchControlAddress(scope);
  const before = (await runtime.cache.readCanonicalCached(control)).value;
  await runtime.tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    runtime.tracker.tasksRecord([{ ...seed, watched: false }]));
  await runtime.tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    runtime.tracker.tasksRecord([seed]));
  const after = (await runtime.cache.readCanonicalCached(control)).value;
  expect(after.blindRetirementEpoch).not.toBe(before.blindRetirementEpoch);
  expect((await runtime.tracker.readTasksRecord(scope.deviceId, scope.projectId)).tasks[0]).toMatchObject({
    watched: true, watch_started_after: "te-05",
  });
  read.answer();
  const saved = await awaitSaved(read);
  expect(read.servedProof()).toBe(true);
  expect(saved).toMatchObject({ watch_uncertain: true, needs_unread_refresh: true });
  expect(saved.task.__task_watch_snapshot?.scopes?.watch?.admission).toBe("historical");
  expect((await runtime.cache.readCanonicalCached(control)).value).toEqual(after);
});

it("UNITCONTRACT adds equal-sequence complementary scopes only with agreeing watch prerequisites and rejects conflicting overlap atomically", async () => {
  const complete = proofReply({ sequence: "7" });
  const watchOnly = structuredClone(complete);
  watchOnly.task.watch_facts = { watch: complete.task.watch_facts.watch };
  const read = await beginRead([getCapability], watchOnly);
  read.answer();
  const initial = await awaitSaved(read);
  expect(read.servedProof()).toBe(true);
  expect(initial.watch_uncertain).toBe(false);
  expect(initial.task.__task_watch_snapshot.scopes).not.toHaveProperty("total");
  const enriched = await refreshReply(read, complete);
  expect(enriched).toMatchObject({ watch_uncertain: false, needs_unread_refresh: false });
  const known = enriched.task.__task_watch_snapshot;
  expect(known.scopes.total.value.unread_count).toBe(0);
  const conflict = structuredClone(complete);
  conflict.task.title = "Ordinary bytes remain independently admissible";
  conflict.task.updated_at = "2026-10-30T20:01:00Z";
  conflict.task.watch_facts.total.unread_count = 8;
  conflict.task.watch_facts.attention.unread_mentions = 6;
  const conflicted = await refreshReply(read, conflict);
  expect(conflicted.task.title).toBe("Ordinary bytes remain independently admissible");
  expect(conflicted.task.unread_count).toBe(0);
  expect(conflicted.task.unread_mentions).toBe(0);
  expect(conflicted.task.__task_watch_snapshot).toEqual(known);
  expect(conflicted.needs_unread_refresh).toBe(true);
});
