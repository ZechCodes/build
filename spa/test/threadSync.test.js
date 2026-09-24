import { beforeEach, describe, expect, it, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange } from "fake-indexeddb";

const address = { deviceId: "dev-1", entityId: "run-1", kind: "thread", sub: "conversation-1" };
const message = (sequence, body) => ({ type: "message", data: { sequence, role: "agent", body } });
const windowAt = (...items) => ({
  items,
  deliveredSequence: items.at(-1)?.data.sequence || 0,
  activityDigests: [],
});

let cache;
let syncThreadWindow;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ syncThreadWindow } = await import("../src/core/threadSync.js"));
});

describe("a thread page overtaken by connection restoration", () => {
  it("leaves a fresh cache window alone when its request becomes stale during the merge read", async () => {
    const first = message(1, "before restore");
    const fresh = message(3, "fresh after restore");
    const stale = message(2, "old connection page");
    await cache.writeCached(address, windowAt(first));

    let active = true;
    let resolveMergeRead;
    const mergeReadStarted = new Promise((resolve) => { resolveMergeRead = resolve; });
    let readTransactions = 0;
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      const opened = originalTransaction.apply(this, args);
      if (args[1] === "readonly" && ++readTransactions === 2) resolveMergeRead();
      return opened;
    });
    try {
      const syncing = syncThreadWindow({
        ...address,
        conversationId: address.sub,
        active: () => active,
        call: async () => {
          // The restored connection has already delivered the newer window
          // before the old request's page reaches its queued cache merge.
          await cache.writeCached(address, windowAt(first, fresh));
          return { items: [stale], thread_last_sequence: 2, has_more: false };
        },
      });

      await mergeReadStarted;
      active = false;
      await syncing;

      const saved = (await cache.readCached(address)).value;
      expect(saved.items.map((item) => item.data.body)).toEqual(["before restore", "fresh after restore"]);
      expect(saved.deliveredSequence).toBe(3);
    } finally {
      transaction.mockRestore();
    }
  });
});
