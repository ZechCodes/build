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
    let writeTransactions = 0;
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      const opened = originalTransaction.apply(this, args);
      if (args[1] === "readwrite" && ++writeTransactions === 2) resolveMergeRead();
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
    expect(window.deliveredSequence).toBe(7);
  });

  // The forward read moves the cursor past the evidence that a repair is
  // owed. Whatever happens to the repair after it, the next read must still
  // know to make one.
  const awayWhileThreeWasDelivered = [user(1, "sent"), user(2, "sent"), user(3, "sent", 4), user(5, "queued")];

  it("repairs on the next read when the repair read failed", async () => {
    await cache.writeCached(address, windowAt(user(1, "sent"), user(2, "sent"), user(3, "queued")));
    const bridge = conversation(awayWhileThreeWasDelivered);
    let dropped = false;
    const call = async (method, params) => {
      if (!params.newest && !dropped) {
        dropped = true;
        throw new Error("the connection dropped during the repair");
      }
      return bridge.call(method, params);
    };

    await sync(call);
    expect((await saved()).deliveredSequence).toBe(5);
    expect(statusOf(await saved(), 3)).toBe("queued");

    await sync(call);
    expect(statusOf(await saved(), 3)).toBe("sent");
    expect((await saved()).repairThrough).toBeUndefined();
  });

  it("repairs on the next read when the pass was stood down under the repair", async () => {
    await cache.writeCached(address, windowAt(user(1, "sent"), user(2, "sent"), user(3, "queued")));
    const bridge = conversation(awayWhileThreeWasDelivered);
    let active = true;
    let stoodDown = false;
    const call = async (method, params) => {
      if (!params.newest && !stoodDown) {
        stoodDown = true;
        active = false;
      }
      return bridge.call(method, params);
    };

    await syncThreadWindow({ ...address, conversationId: address.sub, call, active: () => active });
    expect(statusOf(await saved(), 3)).toBe("queued");

    active = true;
    await syncThreadWindow({ ...address, conversationId: address.sub, call, active: () => active });
    expect(statusOf(await saved(), 3)).toBe("sent");
  });

  it("keeps what it owes across a reload", async () => {
    await cache.writeCached(address, windowAt(user(1, "sent"), user(2, "sent"), user(3, "queued")));
    const bridge = conversation(awayWhileThreeWasDelivered);
    const failing = async (method, params) => {
      if (!params.newest) throw new Error("the tab closed during the repair");
      return bridge.call(method, params);
    };
    await sync(failing);

    // A new page load: fresh modules over the same database.
    vi.resetModules();
    cache = await import("../src/core/localCache.js");
    ({ syncThreadWindow } = await import("../src/core/threadSync.js"));
    await sync(bridge.call);

    expect(statusOf(await saved(), 3)).toBe("sent");
  });

  it("keeps the debt a repair read before it cannot have paid", async () => {
    const { repairedThreadWindow } = await import("../src/core/threadSync.js");
    const held = { ...windowAt(user(1, "sent"), user(3, "queued")), repairThrough: 6 };
    const early = { items: [user(3, "sent", 4)], thread_last_sequence: 5 };

    const repaired = repairedThreadWindow(held, early);

    expect(statusOf(repaired, 3)).toBe("sent");
    expect(repaired.repairThrough).toBe(6);
    expect(repairedThreadWindow(repaired, { items: [], thread_last_sequence: 6 }).repairThrough).toBeUndefined();
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

describe("forward coverage and repair cursors", () => {
  const saved = async () => (await cache.readCached(address)).value;
  const sync = (call) => syncThreadWindow({ ...address, conversationId: address.sub, call });

  it("records an in-place-only forward watermark once and does not repair unchanged reads again", async () => {
    await cache.writeCached(address, windowAt(message(1, "old")));
    const updated = { ...message(1, "edited"), data: { ...message(1, "edited").data, updated_sequence: 4 } };
    const call = vi.fn(async (_method, params) => ({
      items: params.after_sequence === 0 ? [updated] : [],
      has_more: false, thread_last_sequence: 4,
    }));
    await sync(call);
    expect((await saved()).deliveredSequence).toBe(4);
    expect((await saved()).items).toEqual([updated]);
    expect((await saved()).repairThrough).toBeUndefined();
    await sync(call);
    await sync(call);
    expect(call.mock.calls.map(([, params]) => params.after_sequence)).toEqual([1, 0, 4, 4]);
  });

  it("retains debt after a failed repair while remembering the covered forward watermark", async () => {
    await cache.writeCached(address, windowAt(message(1, "old")));
    const call = vi.fn(async (_method, params) => {
      if (params.after_sequence === 0) throw new Error("repair unavailable");
      return { items: [], has_more: false, thread_last_sequence: 4 };
    });
    await sync(call);
    expect(await saved()).toMatchObject({ deliveredSequence: 4, repairThrough: 4 });
    await sync(call);
    expect(call.mock.calls.map(([, params]) => params.after_sequence)).toEqual([1, 0, 4, 0]);
  });

  it("never borrows the repair response watermark to cover a message not read forward", async () => {
    await cache.writeCached(address, windowAt(message(1, "old")));
    const call = vi.fn(async (_method, params) => params.newest
      ? { items: [], has_more: false, thread_last_sequence: 3 }
      : { items: [message(1, "old"), message(4, "not forward")], has_more: false, thread_last_sequence: 4 });
    await sync(call);
    expect((await saved()).deliveredSequence).toBe(3);
    expect((await saved()).items.map((item) => item.data.sequence)).toEqual([1]);
  });

  it("preserves the generation revision when the first inline append omits it", async () => {
    const { threadWindow } = await import("../src/core/threadSync.js");
    const held = { ...windowAt(), thread_id: "thread-1", thread_generation_revision: 7, retired_thread_ids: ["retired"] };
    const next = threadWindow(held, { thread_id: "thread-1", items: [message(1, "first")] });
    expect(next.thread_generation_revision).toBe(7);
    expect(next.retired_thread_ids).toEqual(["retired"]);
    expect(next.items).toEqual([message(1, "first")]);
  });

  it("rejects a stale generation page arriving after a reset", async () => {
    await cache.writeCached(address, { ...windowAt(message(1, "old")), thread_id: "old", thread_generation_revision: 1 });
    const call = vi.fn(async () => {
      await cache.writeCached(address, { ...windowAt(), thread_id: "new", thread_generation_revision: 2 });
      return { thread_id: "old", items: [message(2, "stale")], has_more: false, thread_last_sequence: 3 };
    });
    await sync(call);
    expect(await saved()).toMatchObject({ thread_id: "new", thread_generation_revision: 2, deliveredSequence: 0, items: [] });
  });
});

describe("lean forward coverage", () => {
  it("repairs an in-place activity update once, then asks after its covered counter", async () => {
    const { greetBridge } = await import("../src/core/changeEvents.js");
    await greetBridge(async () => ({ api_version: "3.24.0", capabilities: ["threads.leanActivity"], push_events: false }), { deviceId: address.deviceId });
    const activity = (updated) => ({ type: "event", data: { sequence: 2, updated_sequence: updated, event: "tool_use", summary: "Read a.js" } });
    await cache.writeCached(address, windowAt(message(1, "start"), activity(2)));
    const call = vi.fn(async (_method, params) => ({
      items: params.after_sequence === 0 ? [message(1, "start"), activity(5)] : [],
      has_more: false, thread_last_sequence: 5,
    }));
    const request = { ...address, conversationId: address.sub, call };
    await syncThreadWindow(request);
    await syncThreadWindow(request);
    expect(call.mock.calls.map(([, params]) => params.after_sequence)).toEqual([2, 0, 5]);
    const held = (await cache.readCached(address)).value;
    expect(held.items.at(-1).data.updated_sequence).toBe(5);
    expect(held.repairThrough).toBeUndefined();
  });

  it("represents a real newest-page gap as older history rather than a falsely joined window", async () => {
    await cache.writeCached(address, { ...windowAt(message(1, "old context")), thread_id: "thread-1", thread_generation_revision: 3, retired_thread_ids: ["old-thread"] });
    const call = vi.fn(async () => ({ items: [message(10, "new context")], thread_id: "thread-1", has_more: true, thread_last_sequence: 12 }));
    await syncThreadWindow({ ...address, conversationId: address.sub, call });
    const held = (await cache.readCached(address)).value;
    expect(held).toMatchObject({ deliveredSequence: 12, olderItemsRemain: true, thread_generation_revision: 3, retired_thread_ids: ["old-thread"] });
    expect(held.items.map((item) => item.data.sequence)).toEqual([10]);
    expect(held.repairThrough).toBeUndefined();
    expect(call).toHaveBeenCalledTimes(1);
  });
});

describe("capped repair coverage", () => {
  it("keeps debt when a repair cap omits a held item even though its snapshot counter is current", async () => {
    const { repairedThreadWindow } = await import("../src/core/threadSync.js");
    const held = { ...windowAt(message(1, "held"), message(100, "awaiting edit")), repairThrough: 103 };
    const partial = { items: [message(1, "held")], has_more: true, thread_last_sequence: 103 };
    const repaired = repairedThreadWindow(held, partial);
    expect((repaired || held).repairThrough).toBe(103);
    const complete = repairedThreadWindow(repaired || held, { ...partial, items: held.items });
    expect(complete.repairThrough).toBeUndefined();
    expect(complete.deliveredSequence).toBe(100);
  });
});

describe("sparse cached windows needing capped repair", () => {
  it("refreshes missing held representatives with bounded exact pages instead of repeating the same capped repair", async () => {
    const first = message(1, "start");
    const last = message(100, "old reply");
    await cache.writeCached(address, windowAt(first, last));
    const updated = { ...message(100, "edited reply"), data: { ...last.data, body: "edited reply", updated_sequence: 103 } };
    const items = [first, ...Array.from({ length: 98 }, (_, index) => ({
      type: "event", data: { sequence: index + 2, event: "tool_use", summary: "activity omitted from held window" },
    })), updated];
    const call = vi.fn(async (_method, params) => {
      const after = items.filter((item) => item.data.sequence > params.after_sequence);
      return { items: after.slice(0, params.limit), has_more: after.length > params.limit, thread_last_sequence: 103 };
    });
    const request = { ...address, conversationId: address.sub, call };
    await syncThreadWindow(request);
    const held = (await cache.readCached(address)).value;
    expect(held.items).toEqual([first, updated]);
    expect(held.deliveredSequence).toBe(103);
    expect(held.repairThrough).toBeUndefined();
    expect(call.mock.calls.map(([method, params]) => [method, params.after_sequence, params.limit])).toEqual([
      ["thread.page", 100, 100], ["thread.page", 0, 50], ["thread.page", 99, 1],
    ]);
    await syncThreadWindow(request);
    expect(call).toHaveBeenCalledTimes(4);
    expect(call.mock.calls.at(-1)[1].after_sequence).toBe(103);
  });
});

describe("bounded exact repair reads", () => {
  it.each(["unavailable", "wrong-generation", "wrong-sequence"])("retains debt after an %s exact copy and recovers on a later read", async (failure) => {
    const first = message(1, "start");
    const last = message(100, "old");
    const updated = { type: "message", data: { ...last.data, body: "edited", updated_sequence: 103 } };
    await cache.writeCached(address, { ...windowAt(first, last), thread_id: "thread-1", thread_generation_revision: 1 });
    let refuse = true;
    const call = vi.fn(async (_method, params) => {
      if (params.limit === 1) {
        expect(params.activity).toBeUndefined();
        expect(params.thread_id).toBe("thread-1");
        if (refuse && failure === "unavailable") throw new Error("offline");
        return { thread_id: refuse && failure === "wrong-generation" ? "thread-other" : "thread-1",
          items: [refuse && failure === "wrong-sequence" ? message(101, "unseen") : updated], thread_last_sequence: 110 };
      }
      return { thread_id: "thread-1", items: params.after_sequence === 0 ? [first] : [],
        has_more: params.after_sequence === 0, thread_last_sequence: 103 };
    });
    const request = { ...address, conversationId: address.sub, threadId: "thread-1", call };
    await syncThreadWindow(request);
    expect((await cache.readCached(address)).value).toMatchObject({ items: [first, last], deliveredSequence: 103, repairThrough: 103 });
    refuse = false;
    await syncThreadWindow(request);
    const held = (await cache.readCached(address)).value;
    expect(held.items).toEqual([first, updated]);
    expect(held.repairThrough).toBeUndefined();
    expect(held.deliveredSequence).toBe(103);
    expect(call.mock.calls.filter(([, params]) => params.limit === 1)).toHaveLength(2);
  });

  it("limits exact work to the fifty recent held representatives across a very sparse history", async () => {
    const heldItems = Array.from({ length: REPAIRED_THREAD_ITEMS + 10 }, (_, index) => message((index + 1) * 1000, "held"));
    await cache.writeCached(address, windowAt(...heldItems));
    const counter = heldItems.at(-1).data.sequence + 3;
    const call = vi.fn(async (_method, params) => {
      const remaining = Math.max(0, counter - 3 - params.after_sequence);
      const items = Array.from({ length: Math.min(params.limit, remaining) }, (_, index) => {
        const sequence = params.after_sequence + index + 1;
        return sequence % 1000 === 0 ? message(sequence, "held")
          : { type: "event", data: { sequence, event: "tool_use" } };
      });
      return { items, has_more: remaining > params.limit, thread_last_sequence: counter };
    });
    await syncThreadWindow({ ...address, conversationId: address.sub, call });
    const held = (await cache.readCached(address)).value;
    expect(held.items).toEqual(heldItems);
    expect(held.repairThrough).toBeUndefined();
    expect(held.deliveredSequence).toBe(counter);
    expect(call).toHaveBeenCalledTimes(REPAIRED_THREAD_ITEMS + 1);
    expect(call.mock.calls.filter(([, params]) => params.limit === 1)).toHaveLength(REPAIRED_THREAD_ITEMS - 1);
  });
});
