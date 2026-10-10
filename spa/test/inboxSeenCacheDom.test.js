/** @vitest-environment jsdom */
import { afterEach, beforeEach, expect, it, vi } from "vitest";
import { IDBFactory, IDBKeyRange } from "fake-indexeddb";

globalThis.indexedDB = new IDBFactory();
globalThis.IDBKeyRange = IDBKeyRange;
const call = vi.fn();
vi.mock("../src/core/inboxDevices.js", () => ({ verbCall: () => call }));
vi.mock("../src/core/deviceContexts.js", () => ({ homeContext: () => ({ deviceId: "dev-1" }) }));
const { markSeen } = await import("../src/core/inboxSeen.js");
const { readCached, wipeCache, writeCached } = await import("../src/core/localCache.js");
const address = { deviceId: "dev-1", entityId: "run-1", kind: "row", sub: "" };
const roster = (cursor) => ({
  kind: "project", run_id: "run-1",
  agents: [{ id: "agent-1", thread_id: "thread-1", read_through_sequence: cursor, unread_count: 12 - cursor }],
});
const deferred = () => {
  let resolve;
  const promise = new Promise((done) => { resolve = done; });
  return { promise, resolve };
};
beforeEach(async () => {
  call.mockReset();
  await wipeCache();
});
afterEach(async () => { await wipeCache(); });

it("keeps the latest confirmed read when overlapping older-bridge roster refreshes finish", async () => {
  await writeCached(address, roster(10));
  const older = deferred();
  const newer = deferred();
  let pulls = 0;
  call.mockImplementation(async (method) => {
    if (method === "entity.seen") return { ok: true };
    pulls += 1;
    return pulls === 1 ? older.promise : newer.promise;
  });
  const first = markSeen("run-1", "agent-1", 1, 11, "thread-1", "dev-1");
  await vi.waitFor(() => expect(pulls).toBe(1));
  const second = markSeen("run-1", "agent-1", 1, 12, "thread-1", "dev-1");
  // Let the second RPC and its pre-read capture happen before the older reply.
  await new Promise((done) => setTimeout(done, 20));
  older.resolve({ items: [], runs: [roster(11)] });
  await first;
  await vi.waitFor(() => expect(pulls).toBe(2));
  newer.resolve({ items: [], runs: [roster(12)] });
  await second;
  expect((await readCached(address)).value.agents[0].unread_count).toBe(0);
});
