// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange, IDBObjectStore } from "fake-indexeddb";

let cache, retireConversationFeed, writeConversationFeed, resetAliasesInRow, reconcileConversationGeneration;
const feedAddress = { deviceId: "dev-1", entityId: "", kind: "feed", sub: "" };
const ownership = { deviceId: "dev-1", entityId: "run-1", conversationId: "canonical", threadId: "old-thread" };
const current = { id: "agent-1", conversation_id: "canonical", thread_id: "new-thread", thread_generation_revision: 1, topic: null };
const oldAgent = { ...current, thread_id: "old-thread", thread_generation_revision: 0, topic: "old secret topic" };
const sibling = { id: "sibling", conversation_id: "other", thread_id: "other-thread", topic: "keep sibling" };

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ retireConversationFeed, writeConversationFeed } = await import("../src/core/conversationFeedReset.js"));
  ({ resetAliasesInRow, reconcileConversationGeneration } = await import("../src/core/conversationReset.js"));
  await cache.writeCached({ ...ownership, kind: "thread", sub: ownership.conversationId }, { thread_id: current.thread_id, thread_generation_revision: 1 });
});
afterEach(() => vi.restoreAllMocks());

it("admits a large board once while guards change before its transaction", async () => {
  const items = Array.from({ length: 204 }, (_, index) => ({
    run_id: `run-${index}`, kind: "branch", agents: [{ ...oldAgent, id: `agent-${index}`, conversation_id: `conversation-${index}` }],
  }));
  await Promise.all(items.flatMap((row) => [
    cache.writeCached({ deviceId: "dev-1", entityId: row.run_id, kind: "row" }, row),
    cache.writeCached({ deviceId: "dev-1", entityId: row.run_id, kind: "thread", sub: row.agents[0].conversation_id }, {
      thread_id: "old-thread", thread_generation_revision: 0,
    }),
  ]));
  const update = cache.updateCachedFeed;
  let attempts = 0;
  vi.spyOn(cache, "updateCachedFeed").mockImplementation(async (...args) => {
    attempts += 1;
    // The push is committed after any separate guard captures, before the
    // feed transaction. More board retries would see more independent pushes.
    if (attempts <= 3) {
      const replacement = { ...items[0].agents[0], thread_id: "new-thread", thread_generation_revision: attempts, topic: null };
      await cache.writeCached({ deviceId: "dev-1", entityId: "run-0", kind: "row" }, { ...items[0], agents: [replacement] });
      await cache.writeCached({ deviceId: "dev-1", entityId: "run-0", kind: "thread", sub: replacement.conversation_id }, {
        thread_id: replacement.thread_id, thread_generation_revision: attempts,
      });
    }
    return update(...args);
  });
  const gets = vi.spyOn(IDBObjectStore.prototype, "get");
  expect(await writeConversationFeed(feedAddress, { items }, { active: () => true, rewriteRow: resetAliasesInRow })).toBe(true);
  const counts = (kind) => gets.mock.calls.filter(([key]) => key.split("|")[2] === kind).length;
  expect({ rows: counts("row"), threads: counts("thread"), attempts }).toEqual({ rows: 204, threads: 204, attempts: 1 });
  const held = (await cache.readCached(feedAddress)).value;
  expect(held.items[0].agents[0].thread_id).toBe("new-thread");
  expect(held.items[0].agents[0].topic).toBeNull();
});

it("lets a reset queued after board admission retire the newly admitted copy", async () => {
  const address = { deviceId: ownership.deviceId, entityId: ownership.entityId, kind: "thread", sub: ownership.conversationId };
  await cache.writeCached(address, { thread_id: "old-thread", thread_generation_revision: 0 });
  const get = IDBObjectStore.prototype.get;
  let resetting;
  const spy = vi.spyOn(IDBObjectStore.prototype, "get").mockImplementation(function (key) {
    const request = get.call(this, key);
    if (key === "dev-1||feed|" && !resetting) {
      resetting = reconcileConversationGeneration({ ...ownership, agent: current });
    }
    return request;
  });
  await writeConversationFeed(feedAddress, { items: [{ run_id: ownership.entityId, agents: [oldAgent] }] }, {
    active: () => true, rewriteRow: resetAliasesInRow,
  });
  expect(resetting).toBeInstanceOf(Promise);
  await resetting;
  spy.mockRestore();
  const held = (await cache.readCached(feedAddress)).value;
  expect(held.items[0].agents[0].thread_id).toBe("new-thread");
  expect(JSON.stringify(held)).not.toContain("old secret");
});

it("retires every exact canonical feed copy while preserving sibling text and observation stamps", async () => {
  const first = { run_id: "run-1", summary: "old secret summary", agents: [oldAgent, sibling], __cacheObserved: { at: 3, order: 4 } };
  const alias = { run_id: "run-2", summary: "keep primary summary", agents: [sibling, { ...oldAgent, id: "alias" }] };
  const legacy = { run_id: "run-3", agents: [oldAgent], thread: { id: "old-thread", items: [{ body: "old secret transcript" }] } };
  await cache.writeCached(feedAddress, { items: [first, alias], runs: [legacy] });
  const observed = (await cache.readCached(feedAddress)).value.items[0].__cacheObserved;
  await retireConversationFeed(ownership, current, resetAliasesInRow);
  const held = (await cache.readCached(feedAddress)).value;
  expect(JSON.stringify(held)).not.toContain("old secret");
  expect(held.items[0].__cacheObserved).toEqual(observed);
  expect(held.items[0].agents[1]).toEqual(sibling);
  expect(held.items[1].summary).toBe("keep primary summary");
  expect(held.items[1].agents[0]).toEqual(sibling);
  expect(held.items[1].agents[1].thread_id).toBe(current.thread_id);
});

it("refuses delayed retirement after another reset advances the authoritative generation", async () => {
  const fresh = { run_id: "run-1", summary: "newer words", agents: [{ ...current, thread_id: "newer-thread", thread_generation_revision: 2, topic: "newer topic" }] };
  await cache.writeCached(feedAddress, { items: [fresh] });
  await cache.writeCached({ ...ownership, kind: "thread", sub: ownership.conversationId }, { thread_id: "newer-thread", thread_generation_revision: 2 });
  expect(await retireConversationFeed(ownership, current, resetAliasesInRow)).toBe(false);
  expect((await cache.readCached(feedAddress)).value.items[0]).toMatchObject(fresh);
});

it("does not restore a legacy feed transcript that names its agent through the thread", async () => {
  const legacy = { run_id: "run-1", thread: { id: "old-thread", agent: { id: "canonical" }, items: [{ body: "old secret transcript" }] } };
  await writeConversationFeed(feedAddress, { runs: [legacy] }, { active: () => true, rewriteRow: resetAliasesInRow });
  const held = (await cache.readCached(feedAddress)).value;
  expect(JSON.stringify(held)).not.toContain("old secret");
  expect(held.runs[0].thread.thread_id).toBe("new-thread");
});
