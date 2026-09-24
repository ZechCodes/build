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
let REPAIRED_THREAD_ITEMS;

beforeEach(async () => {
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  ({ syncThreadWindow } = await import("../src/core/threadSync.js"));
  ({ REPAIRED_THREAD_ITEMS } = await import("../src/core/cacheThresholds.js"));
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

// #120: a forward read walks the conversation's own order (bridge
// `thread.page` with `after_sequence` reads `sequence`), so an item under the
// cursor that changed in place while nobody was connected is never shipped by
// it — a delivery status, a settled message, a tool call's answer.
describe("a conversation that changed under the cursor while the device was away", () => {
  const user = (sequence, status, updated = sequence) => ({
    type: "message",
    data: { sequence, updated_sequence: updated, role: "user", body: `m${sequence}`, delivery_status: status },
  });

  /** The bridge's two forward walks over one conversation (bridge
   *  thread/paging.rs): by `sequence`, the newest `limit` in `newest` mode,
   *  and `thread_last_sequence` the highest counter value any item wears. */
  const conversation = (items) => {
    const latest = (item) => Math.max(item.data.sequence, item.data.updated_sequence || 0);
    const asked = [];
    const call = async (method, params) => {
      asked.push(params);
      const after = items.filter((item) => item.data.sequence > params.after_sequence);
      const page = params.newest ? after.slice(-params.limit) : after.slice(0, params.limit);
      return {
        items: structuredClone(page),
        has_more: after.length > page.length,
        thread_total: items.length,
        thread_last_sequence: Math.max(...items.map(latest)),
      };
    };
    return { call, asked };
  };

  const sync = (call) => syncThreadWindow({ ...address, conversationId: address.sub, call });
  const saved = async () => (await cache.readCached(address)).value;
  const statusOf = (window, sequence) => window.items.find((item) => item.data.sequence === sequence).data.delivery_status;

  it("re-reads the recent items it holds and takes the changed one", async () => {
    await cache.writeCached(address, windowAt(user(1, "sent"), user(2, "sent"), user(3, "queued")));
    // While away: message 3 was delivered (counter 4), then message 5 posted.
    const bridge = conversation([user(1, "sent"), user(2, "sent"), user(3, "sent", 4), user(5, "queued")]);

    await sync(bridge.call);

    const window = await saved();
    expect(statusOf(window, 3)).toBe("sent");
    expect(window.items.map((item) => item.data.sequence)).toEqual([1, 2, 3, 5]);
    expect(window.deliveredSequence).toBe(5);
    expect(bridge.asked).toEqual([
      expect.objectContaining({ after_sequence: 3, newest: true }),
      expect.objectContaining({ after_sequence: 0 }),
    ]);
    expect(bridge.asked[1].newest).toBeUndefined();
  });

  it("asks once when every change past the cursor is on the forward page", async () => {
    await cache.writeCached(address, windowAt(user(1, "sent"), user(2, "queued")));
    const bridge = conversation([user(1, "sent"), user(2, "queued"), user(3, "sent", 4)]);

    await sync(bridge.call);

    expect(bridge.asked).toHaveLength(1);
    expect((await saved()).items.map((item) => item.data.sequence)).toEqual([1, 2, 3]);
  });

  it("re-reads no further back than the recent items it holds", async () => {
    const held = Array.from({ length: REPAIRED_THREAD_ITEMS + 10 }, (_, index) => user(index + 1, "sent"));
    await cache.writeCached(address, windowAt(...held));
    const changed = held.map((item) => (item.data.sequence === 1 ? user(1, "seen", held.length + 1) : item));
    const bridge = conversation(changed);

    await sync(bridge.call);

    expect(bridge.asked[1]).toEqual(expect.objectContaining({ after_sequence: 10, limit: REPAIRED_THREAD_ITEMS }));
    expect(statusOf(await saved(), 1)).toBe("sent");
  });

  it("adds nothing it did not already hold, and leaves the cursor where the forward read put it", async () => {
    await cache.writeCached(address, windowAt(user(4, "sent"), user(5, "queued")));
    const bridge = conversation([user(3, "sent"), user(4, "sent"), user(5, "sent", 7), user(6, "sent")]);
    // Between the two reads the bridge answers, a message the forward read
    // never saw is posted: the repair must not carry the cursor past it.
    const later = [user(3, "sent"), user(4, "sent"), user(5, "sent", 7), user(6, "sent"), user(8, "queued")];
    const call = async (method, params) => (params.newest ? bridge.call(method, params) : conversation(later).call(method, params));

    await sync(call);

    const window = await saved();
    expect(window.items.map((item) => item.data.sequence)).toEqual([4, 5, 6]);
    expect(statusOf(window, 5)).toBe("sent");
    expect(window.deliveredSequence).toBe(6);
  });
});

describe("an item arriving older than the copy held", () => {
  it("keeps the newer copy", async () => {
    const { mergeThreadItems } = await import("../src/core/thread.js");
    const newer = { type: "message", data: { sequence: 3, updated_sequence: 9, delivery_status: "seen" } };
    const older = { type: "message", data: { sequence: 3, updated_sequence: 4, delivery_status: "sent" } };
    expect(mergeThreadItems([newer], [older])).toEqual([newer]);
    expect(mergeThreadItems([older], [newer])).toEqual([newer]);
  });
});
