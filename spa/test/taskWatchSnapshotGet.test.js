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

async function beginRead(capabilities, reply = proofReply(), greetingOptions) {
  const { session, context } = await greet(capabilities, greetingOptions);
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


// Decoder refusals are peer-input UNITCONTRACT controls, not claims that the
// actual backend serializes malformed/oversize typed proof objects.
const invalidProofCases = [
  { label: "unknown envelope version", mutate: (reply) => { reply.watch_snapshot.version = 2; } },
  { label: "numeric sequence above JSON precision", mutate: (reply) => { reply.watch_snapshot.sequence = 9007199254740992; } },
  { label: "overflow u64 sequence", mutate: (reply) => { reply.watch_snapshot.sequence = "18446744073709551616"; } },
  { label: "noncanonical leading-zero sequence", mutate: (reply) => { reply.watch_snapshot.sequence = "007"; } },
  { label: "lone-surrogate marker", mutate: (reply) => { reply.task.watch_facts.total.read_through = "tc-\ud800"; } },
  { label: "marker above UTF8 byte bound", mutate: (reply) => { reply.task.watch_facts.total.read_through = "é".repeat(65); } },
  { label: "unsafe count", mutate: (reply) => { reply.task.watch_facts.total.unread_count = 9007199254740992; } },
  { label: "missing declared nullable field", mutate: (reply) => { delete reply.task.watch_facts.activity.last_activity_ms; } },
  { label: "unknown field inside known scope", mutate: (reply) => { reply.task.watch_facts.total.extra = true; } },
  { label: "dependent scope without watch", mutate: (reply) => { delete reply.task.watch_facts.watch; } },
  { label: "certified unavailable overlap", mutate: (reply) => { reply.task.watch_unavailable = { total: "coherence_unavailable" }; } },
  { label: "unknown unavailable reason", mutate: (reply) => { delete reply.task.watch_facts.total; reply.task.watch_unavailable = { total: "unknown_reason" }; } },
  { label: "escaped unknown scope exceeds row-proof bytes", mutate: (reply) => { reply.task.watch_facts.future = { raw: "\u0000".repeat(700) }; } },
  { label: "extra envelope bytes exceed envelope cap", mutate: (reply) => { reply.watch_snapshot.extra = "x".repeat(2050); } },
];
for (const { label, mutate } of invalidProofCases) {
  it(`UNITCONTRACT refuses ${label} without discarding independently useful full content`, async () => {
    const reply = proofReply();
    mutate(reply);
    const read = await beginRead([getCapability], reply);
    read.answer();
    const saved = await awaitSaved(read);
    expect(read.servedProof()).toBe(true);
    expect(saved).toMatchObject({ watch_uncertain: true, needs_unread_refresh: true });
    expect(saved.task).not.toHaveProperty("__task_watch_snapshot");
    expect(read.session.bridgeEpoch.current()).toMatchObject({ ...pair, opening: 1 });
  });
}

it("UNITCONTRACT preserves explicit empty floor and raw NUL marker in declared canonical scopes", async () => {
  const reply = proofReply({ sequence: "0" });
  reply.task.watch_started_after = "";
  reply.task.read_through = "tc-\u0000";
  reply.task.watch_facts.watch.watch_started_after = "";
  reply.task.watch_facts.total.read_through = "tc-\u0000";
  reply.task.watch_facts.total.effective_watch_floor = "";
  reply.task.watch_facts.attention.read_through = "tc-\u0000";
  const read = await beginRead([getCapability], reply);
  read.answer();
  const saved = await awaitSaved(read);
  expect(read.servedProof()).toBe(true);
  expect(saved).toMatchObject({ watch_uncertain: false, needs_unread_refresh: false });
  expect(saved.task).toMatchObject({ read_through: "tc-\u0000", watch_started_after: "", unread_count: 0 });
  expect(saved.task.__task_watch_snapshot?.scopes?.total?.value).toMatchObject({
    read_through: "tc-\u0000", effective_watch_floor: "",
  });
  expect(saved.task.__task_watch_snapshot?.scopes?.watch?.sequence).toBe("0");
});

it("UNITCONTRACT keeps the canonical first-hyphen UTF8 suffix domain distinct from the already extracted floor", async () => {
  const reply = proofReply({ sequence: "9007199254740993" });
  const rawRead = "tc-a-\u{10000}", rawWatch = "te-a-\uE000";
  reply.task.read_through = rawRead;
  reply.task.watch_started_after = rawWatch;
  reply.task.watch_facts.watch.watch_started_after = rawWatch;
  reply.task.watch_facts.total.read_through = rawRead;
  reply.task.watch_facts.total.effective_watch_floor = "a-\uE000";
  reply.task.watch_facts.attention.read_through = rawRead;
  const read = await beginRead([getCapability], reply);
  read.answer();
  const saved = await awaitSaved(read);
  expect(saved).toMatchObject({ watch_uncertain: false, needs_unread_refresh: false });
  expect(saved.task.read_through).toBe(rawRead);
  expect(saved.task.__task_watch_snapshot?.scopes?.total?.value?.effective_watch_floor).toBe("a-\uE000");
  expect(saved.task.__task_watch_snapshot?.scopes?.watch?.sequence).toBe("9007199254740993");
});

it("UNITCONTRACT admits declared unwatched proof when native TaskView omits false and nullable markers", async () => {
  const reply = proofReply();
  delete reply.task.watched;
  delete reply.task.watch_started_after;
  delete reply.task.read_through;
  reply.task.watch_facts = { watch: { watched: false, watch_started_after: null } };
  const read = await beginRead([getCapability], reply);
  read.answer();
  const saved = await awaitSaved(read);
  expect(read.servedProof()).toBe(true);
  expect(saved.watch_uncertain).toBe(false);
  expect(saved.task.watched).toBe(false);
  expect(saved.task).not.toHaveProperty("unread_count");
  expect(saved.task.__task_watch_snapshot?.scopes?.watch?.value).toEqual({ watched: false, watch_started_after: null });
  expect(saved.task.__task_watch_snapshot?.scopes).not.toHaveProperty("total");
});

it("UNITCONTRACT requires bridge epoch capability as well as the GET atom before adding the opt-in", async () => {
  const read = await beginRead([getCapability], proofReply(), { includeEpoch: false });
  expect(read.requests[0].params).toEqual({ task_id: taskId });
  read.answer();
  const saved = await awaitSaved(read);
  expect(read.servedProof()).toBe(false);
  expect(saved.watch_uncertain).toBe(true);
});

it("UNITCONTRACT keeps legacy GET usable when accepted greeting identity exceeds proof bounds", async () => {
  const read = await beginRead([getCapability], proofReply(), { identity: { ...pair, journal_incarnation: "é".repeat(65) } });
  expect(read.requests[0].params).toEqual({ task_id: taskId });
  read.answer();
  const saved = await awaitSaved(read);
  expect(saved.watch_uncertain).toBe(true);
  expect(read.session.bridgeEpoch.current().journal_incarnation).toBe("é".repeat(65));
});

// Direct existing actual GET-owner API gives a real awaited publication seam
// for whole-owner refusal. It neither creates a descriptor nor mocks a reducer.
async function directGet({ accept = () => true } = {}) {
  const { session, context } = await greet([getCapability]);
  const response = pendingReply(), requests = [];
  session.call.mockImplementation((method, params) => {
    expect(method).toBe("tasks.get");
    requests.push(structuredClone(params));
    return response.promise;
  });
  const current = () => accept() && runtime.contexts.contextFor(scope.deviceId) === context && context.session === session && context.active();
  const get = await import("../src/core/taskWatchSnapshotRead.js");
  const address = runtime.tracker.taskAddress(scope.deviceId, scope.projectId, taskId);
  const captured = await get.captureTaskGetRead({ address, call: context.rpc, accept: current });
  const reading = get.dispatchTaskGetRead(captured.request);
  await vi.waitFor(() => expect(requests).toHaveLength(1));
  expect(requests[0]).toEqual({ task_id: taskId, watch_snapshot: true });
  return { captured, session, context, address, current,
    async publish(reply) {
      response.resolve(reply);
      const received = await reading;
      return runtime.tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
        runtime.tracker.taskRecord(received.answer.task, received.answer.timeline), {
          contentRead: captured.contentRead, watchSnapshotRead: received.watchSnapshotRead, accept: current,
        });
    },
  };
}

for (const refusal of ["explicit owner refusal", "session replacement", "context retirement"]) {
  it(`UNITCONTRACT makes no historical or selected cache write after ${refusal}`, async () => {
    let active = true;
    const read = await directGet({ accept: () => active });
    const target = runtime.summaries.summaryAddress(scope, taskId);
    const beforeDetail = await runtime.cache.readCanonicalCached(read.address);
    const beforeSummary = await runtime.cache.readCanonicalCached(target);
    if (refusal === "explicit owner refusal") active = false;
    if (refusal === "session replacement") runtime.contexts.adoptDeviceSession(fakeSession(scope.deviceId));
    if (refusal === "context retirement") runtime.contexts.retireDeviceContext(scope.deviceId);
    expect(await read.publish(proofReply())).toBe(false);
    expect(await runtime.cache.readCanonicalCached(read.address)).toEqual(beforeDetail);
    expect(await runtime.cache.readCanonicalCached(target)).toEqual(beforeSummary);
  });
}

it("UNITCONTRACT chooses higher canonical sequence from an earlier dispatch and refuses a later-dispatched lower sequence", async () => {
  const first = await directGet();
  // This second capture uses the SAME current authenticated session/context.
  // Creating another fake greeting would wrongly invalidate the first owner.
  const get = await import("../src/core/taskWatchSnapshotRead.js");
  const pending = pendingReply();
  first.session.call.mockImplementationOnce(() => pending.promise);
  const second = await get.captureTaskGetRead({ address: first.address, call: first.context.rpc, accept: first.current });
  expect(second.contentRead.readOrder).toBeGreaterThan(first.captured.contentRead.readOrder);
  const secondReading = get.dispatchTaskGetRead(second.request);
  pending.resolve(proofReply({ sequence: "8" }));
  const received = await secondReading;
  await runtime.tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId,
    runtime.tracker.taskRecord(received.answer.task, received.answer.timeline), {
      contentRead: second.contentRead, watchSnapshotRead: received.watchSnapshotRead, accept: first.current,
    });
  const target = runtime.summaries.summaryAddress(scope, taskId);
  expect((await runtime.cache.readCanonicalCached(target)).value.watch_uncertain).toBe(false);
  await first.publish(proofReply({ sequence: "9" }));
  const higher = (await runtime.cache.readCanonicalCached(target)).value;
  expect(higher.task.__task_watch_snapshot?.scopes?.watch?.sequence).toBe("9");
  expect(higher.watch_uncertain).toBe(false);
  const read = await beginRead([getCapability], proofReply({ sequence: "7" }));
  read.answer();
  const lower = await awaitSaved(read);
  expect(lower.task.__task_watch_snapshot?.scopes?.watch?.sequence).toBe("9");
  expect(lower.watch_uncertain).toBe(false);
});

it("UNITCONTRACT copies cannot rebind an as-of source after blind retirement but a fresh same-sequence GET can", async () => {
  const read = await beginRead([getCapability], proofReply({ sequence: "9" }));
  read.answer();
  const first = await awaitSaved(read);
  expect(first.watch_uncertain).toBe(false);
  const oldProof = structuredClone(first.task.__task_watch_snapshot);
  const saved = await runtime.tracker.readTaskRecord(scope.deviceId, scope.projectId, taskId);
  const copy = structuredClone(saved);
  await runtime.tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    runtime.tracker.tasksRecord([{ ...nativeTask(), watched: false }]));
  await runtime.tracker.writeTasksRecord(scope.deviceId, scope.projectId,
    runtime.tracker.tasksRecord([nativeTask()]));
  const content = await import("../src/core/taskContentAuthority.js");
  const captured = await content.captureTaskContentRead(runtime.tracker.taskAddress(scope.deviceId, scope.projectId, taskId), { revalidate: "full" });
  // A genuine ordinary capture still supplies NO actual GET response brand.
  await runtime.tracker.writeTaskRecord(scope.deviceId, scope.projectId, taskId, copy, { contentRead: captured });
  const replay = (await runtime.cache.readCanonicalCached(runtime.summaries.summaryAddress(scope, taskId))).value;
  expect(replay.watch_uncertain).toBe(true);
  expect(replay.task.__task_watch_snapshot?.scopes?.watch?.client).toEqual(oldProof?.scopes?.watch?.client);
  const recovered = await refreshReply(read, proofReply({ sequence: "9" }));
  expect(recovered.watch_uncertain).toBe(false);
  expect(recovered.task.__task_watch_snapshot?.scopes?.watch?.sequence).toBe("9");
  expect(recovered.task.__task_watch_snapshot?.scopes?.watch?.client).not.toEqual(oldProof?.scopes?.watch?.client);
});
