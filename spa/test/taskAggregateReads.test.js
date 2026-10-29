import { beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";
import countsFixture from "../../fixtures/api/v1/tasks.counts.json";
import lookupFixture from "../../fixtures/api/v1/tasks.lookup.json";

const scope = { deviceId: "aggregate-900", projectId: "proj-1" };
let reads, cache, support, session, active, epoch;
const deferred = () => { let resolve; const promise = new Promise((done) => { resolve = done; }); return { promise, resolve }; };
const options = (callRpc, selectedScope = scope) => ({ scope: selectedScope, callRpc,
  capabilities: () => support, sessionOf: () => session, active: () => active, mutationEpoch: () => epoch });
const counts = (ids = [], extra = {}) => ({ ...countsFixture.result,
  agents: ids.map((agent_id) => ({ agent_id, unfinished_open_count: 0, watched_unread_total: 0 })), ...extra });
const found = () => structuredClone(lookupFixture.examples[0].result);
const missing = () => ({ ...lookupFixture.result, number: 12 });

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory(); globalThis.IDBKeyRange = IDBKeyRange;
  support = { counts: true, lookup: true }; session = {}; active = true; epoch = {};
  reads = await import("../src/core/taskAggregateReads.js");
  cache = await import("../src/core/localCache.js");
});

it("requests only explicit canonical agent buckets and caches authoritative totals without arithmetic", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(counts(["AgentA", "agent-a"]))
    .mockResolvedValueOnce(counts(["AgentA", "agent-a"], { unfinished_open_count: 2, watched_unread_total: 17, basis: "other-store:0" }));
  const reader = reads.createTaskAggregateReader(options(rpc));
  expect(await reader.read(["agent-a", "AgentA"])).toBeNull();
  expect(await reader.refresh(["agent-a", "AgentA", "agent-a"])).toBe(true);
  expect(rpc.mock.calls).toEqual([["tasks.counts", { project_id: "proj-1", agent_ids: ["AgentA", "agent-a"] }]]);
  expect(await reader.read(["agent-a", "AgentA"])).toMatchObject({ unfinished_open_count: 12, watched_unread_total: 5,
    agents: [{ agent_id: "AgentA", unfinished_open_count: 0 }, { agent_id: "agent-a", unfinished_open_count: 0 }] });
  await reader.refresh(["AgentA", "agent-a"]);
  expect(await reader.read(["AgentA", "agent-a"])).toMatchObject({ unfinished_open_count: 2, watched_unread_total: 17, basis: "other-store:0" });
  expect(rpc).toHaveBeenCalledTimes(2);
});

it("preserves exact opaque IDs including NUL and validates the raw 100-entry/128-byte bounds before dispatch", async () => {
  const ids = [" \t ", "a\0", "é".repeat(64)];
  const rpc = vi.fn().mockResolvedValue(counts(ids));
  const reader = reads.createTaskAggregateReader(options(rpc));
  await reader.refresh(ids);
  expect(rpc.mock.calls[0][1].agent_ids).toEqual(ids);
  for (const bad of [[""], [null], ["é".repeat(65)], Array(101).fill("same"), "agent"]) {
    await expect(reader.refresh(bad)).rejects.toThrow();
  }
  expect(rpc).toHaveBeenCalledTimes(1);
});

it("keeps counts and lookup capabilities independent and retains cache while unsupported or inactive", async () => {
  const rpc = vi.fn().mockResolvedValue(counts());
  const aggregate = reads.createTaskAggregateReader(options(rpc));
  const lookup = reads.createTaskLookupReader(options(rpc));
  await aggregate.refresh(); const before = await aggregate.read();
  support = { listSummaries: true, listPaged: true, lookup: true };
  expect(await aggregate.refresh()).toBe(false);
  support = { counts: true };
  expect(await lookup.refresh(12)).toBe(false);
  active = false;
  expect(await aggregate.refresh()).toBe(false);
  expect(await aggregate.read()).toEqual(before);
  expect(rpc).toHaveBeenCalledTimes(1);
});

it("preserves totals on marker refusal and retries only when explicitly refreshed", async () => {
  const refusal = Object.assign(new Error("marker unsupported"), countsFixture.refusals[0].reply);
  const rpc = vi.fn().mockResolvedValueOnce(counts()).mockRejectedValueOnce(refusal).mockResolvedValueOnce(counts([], { watched_unread_total: 1 }));
  const reader = reads.createTaskAggregateReader(options(rpc));
  await reader.refresh(); const before = await reader.read();
  await expect(reader.refresh()).rejects.toBe(refusal);
  expect(await reader.read()).toEqual(before);
  expect(rpc).toHaveBeenCalledTimes(2);
  await reader.refresh();
  expect((await reader.read()).watched_unread_total).toBe(1);
});

it.each(["pending", "acknowledged", "second mutation"])("rejects pre-mutation count reads after %s epoch changes", async () => {
  const pending = deferred(), rpc = vi.fn().mockResolvedValueOnce(counts()).mockImplementationOnce(() => pending.promise);
  const reader = reads.createTaskAggregateReader(options(rpc));
  await reader.refresh(); const before = await reader.read();
  const request = reader.refresh(); await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(2));
  epoch = {};
  pending.resolve(counts([], { watched_unread_total: 99 }));
  expect(await request).toBe(false);
  expect(await reader.read()).toEqual(before);
});

it("separates unknown, confirmed missing, found and failed lookup, retaining bounded summaries and identities", async () => {
  const rpc = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce(missing()).mockResolvedValueOnce(found());
  const reader = reads.createTaskLookupReader(options(rpc));
  expect(await reader.read(12)).toEqual({ status: "unknown", project_id: "proj-1", number: 12 });
  await expect(reader.refresh(12)).rejects.toThrow("offline");
  expect((await reader.read(12)).status).toBe("unknown");
  await reader.refresh(12);
  expect(await reader.read(12)).toMatchObject({ status: "missing", task: null, basis: lookupFixture.result.basis });
  await reader.refresh(12);
  expect(await reader.read(12)).toEqual({ ...found(), status: "found" });
  expect(rpc.mock.calls[2]).toEqual(["tasks.lookup", { project_id: "proj-1", number: 12 }]);
  expect(await cache.readCached({ deviceId: scope.deviceId, entityId: scope.projectId, kind: "tracker-task-detail", sub: found().task.id })).toBeUndefined();
});

it("requires exact positive safe integer lookup keys and rejects number/project mismatches without replacing a hit", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(found()).mockResolvedValueOnce({ ...missing(), project_id: "proj-2" })
    .mockResolvedValueOnce({ ...missing(), number: 13 }).mockResolvedValueOnce({ ...found(), task: { ...found().task, number: 13 } })
    .mockResolvedValueOnce({ ...found(), task: { ...found().task, project_id: "proj-2" } });
  const reader = reads.createTaskLookupReader(options(rpc));
  for (const key of [0, -1, "12", 12.5, NaN, Infinity, Number.MAX_SAFE_INTEGER + 1]) await expect(reader.refresh(key)).rejects.toThrow();
  expect(rpc).not.toHaveBeenCalled();
  await reader.refresh(12); const before = await reader.read(12);
  for (let attempt = 0; attempt < 4; attempt += 1) await expect(reader.refresh(12)).rejects.toThrow();
  expect(await reader.read(12)).toEqual(before);
});

it("isolates scope and request buckets, sharing canonical requests across reader instances", async () => {
  const rpc = vi.fn().mockResolvedValue(counts(["agent-a"]));
  await reads.createTaskAggregateReader(options(rpc)).refresh(["agent-a", "agent-a"]);
  expect((await reads.createTaskAggregateReader(options(vi.fn())).read(["agent-a"])).unfinished_open_count).toBe(12);
  expect(await reads.readTaskCounts(scope, [])).toBeNull();
  expect(await reads.readTaskCounts({ ...scope, deviceId: "other" }, ["agent-a"])).toBeNull();
  expect(await reads.readTaskCounts({ ...scope, projectId: "other" }, ["agent-a"])).toBeNull();
});

it("cannot replace a newer positive lookup with a late negative across reader instances", async () => {
  const pending = deferred(), oldRpc = vi.fn(() => pending.promise);
  const older = reads.createTaskLookupReader(options(oldRpc));
  const request = older.refresh(12); await vi.waitFor(() => expect(oldRpc).toHaveBeenCalledTimes(1));
  await reads.createTaskLookupReader(options(vi.fn().mockResolvedValue(found()))).refresh(12);
  pending.resolve(missing()); expect(await request).toBe(false);
  expect((await older.read(12)).status).toBe("found");
});

it.each(["session", "account", "capability", "mutation", "dispose"])("fences pending lookup after %s retirement", async (kind) => {
  const pending = deferred(), rpc = vi.fn(() => pending.promise), reader = reads.createTaskLookupReader(options(rpc));
  const request = reader.refresh(12); await vi.waitFor(() => expect(rpc).toHaveBeenCalledTimes(1));
  if (kind === "session") session = {};
  if (kind === "account") active = false;
  if (kind === "capability") support = { counts: true };
  if (kind === "mutation") epoch = {};
  if (kind === "dispose") reader.dispose();
  pending.resolve(found()); expect(await request).toBe(false);
  expect((await reader.read(12)).status).toBe("unknown");
});

it("rechecks mutation authority inside the publication transaction", async () => {
  const rpc = vi.fn(async () => {
    const original = IDBObjectStore.prototype.get;
    vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (key) {
      epoch = {}; return original.call(this, key);
    });
    return found();
  });
  const reader = reads.createTaskLookupReader(options(rpc));
  expect(await reader.refresh(12)).toBe(false);
  vi.restoreAllMocks();
  expect((await reader.read(12)).status).toBe("unknown");
});

it("keeps a prior lookup atomic if publication aborts", async () => {
  const rpc = vi.fn().mockResolvedValueOnce(found()).mockResolvedValueOnce(missing());
  const reader = reads.createTaskLookupReader(options(rpc));
  await reader.refresh(12); const before = await reader.read(12);
  const original = IDBObjectStore.prototype.put;
  const spy = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (record, key) {
    if (record?.value?.result?.status === "missing") this.transaction.abort();
    return original.call(this, record, key);
  });
  expect(await reader.refresh(12)).toBe(false); spy.mockRestore();
  expect(await reader.read(12)).toEqual(before);
});

it.each([
  { unfinished_open_count: -1 }, { watched_unread_total: 0.5 }, { basis: null },
  { agents: [] }, { agents: [{ agent_id: "other", unfinished_open_count: 0, watched_unread_total: 0 }] },
  { agents: [{ agent_id: "agent-a", unfinished_open_count: 0, watched_unread_total: 0 }, { agent_id: "agent-a", unfinished_open_count: 0, watched_unread_total: 0 }] },
  { project_id: "other" }, { extra: "x".repeat(65_536) },
])("rejects malformed or unbounded aggregate reply %#", async (patch) => {
  const reader = reads.createTaskAggregateReader(options(vi.fn().mockResolvedValue(counts(["agent-a"], patch))));
  await expect(reader.refresh(["agent-a"])).rejects.toThrow();
  expect(await reader.read(["agent-a"])).toBeNull();
});

it.each([{ task: undefined }, { identities: [] }, { basis: "" }, { extra: "x".repeat(65_536) }, { task: { id: "", number: 12, project_id: "proj-1" } }])(
  "rejects malformed or unbounded lookup reply %#", async (patch) => {
    const reader = reads.createTaskLookupReader(options(vi.fn().mockResolvedValue({ ...found(), ...patch })));
    await expect(reader.refresh(12)).rejects.toThrow();
    expect((await reader.read(12)).status).toBe("unknown");
  },
);
