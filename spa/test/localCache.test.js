// The local cache: one IndexedDB store holding what the surfaces last saw, so
// a revisit paints from disk before the bridge answers. Plaintext by decision
// (2026-08-31): E2EE protects the wire; the browser profile is trusted.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange, IDBObjectStore, forceCloseDatabase } from "fake-indexeddb";

const DB_NAME = "build-cache";
const STORE = "records";

let cache;

const freshFactory = () => {
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
};

/** A database in an earlier format, holding one record, closed again — what a
 *  browser that ran a previous build has on disk. */
const seedVersion = (version, key, record) =>
  new Promise((resolve, reject) => {
    const request = indexedDB.open(DB_NAME, version);
    request.onupgradeneeded = () => request.result.createObjectStore(STORE);
    request.onerror = () => reject(request.error);
    request.onsuccess = () => {
      const db = request.result;
      const transaction = db.transaction(STORE, "readwrite");
      transaction.objectStore(STORE).put(record, key);
      transaction.oncomplete = () => {
        db.close(); // an open v1 connection would block the upgrade
        resolve();
      };
    };
  });

/** The recovery schedule, shortened so a test waits milliseconds for what a
 *  phone waits seconds for. Same shape: several attempts, then a rest. */
const FAST_RECOVERY = { reopenDelaysMs: [0, 1, 2, 3], openTimeoutMs: 200, restMs: 40 };

beforeEach(async () => {
  vi.resetModules();
  freshFactory();
  cache = await import("../src/core/localCache.js");
  cache.setCacheRecoveryTiming(FAST_RECOVERY);
  vi.spyOn(console, "info").mockImplementation(() => {});
});

describe("records", () => {
  it("round-trips a record and stamps when it was written", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "abc" });
    const record = await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" });
    expect(record.value).toEqual({ head: "abc" });
    expect(typeof record.at).toBe("number");
  });

  it("answers undefined for what was never written", async () => {
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-9", kind: "status" })).toBeUndefined();
  });

  it("keeps sub-keyed records apart", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, ["a.js"]);
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "test" }, ["b.js"]);
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" })).value).toEqual(["a.js"]);
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "test" })).value).toEqual(["b.js"]);
  });

  it("does not let a separator in a sub key bleed into another record", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "odd|name" }, ["x"]);
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "odd" })).toBeUndefined();
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "odd|name" })).value).toEqual(["x"]);
  });

  it("keeps two devices' records apart even for the same entity id", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "one" });
    await cache.writeCached({ deviceId: "dev-2", entityId: "run-1", kind: "status" }, { head: "two" });
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).value.head).toBe("one");
    expect((await cache.readCached({ deviceId: "dev-2", entityId: "run-1", kind: "status" })).value.head).toBe("two");
  });
});

describe("eviction", () => {
  it("evicts every record one entity holds and nothing anyone else does", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "show", sub: "abc123" }, {});
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-12", kind: "status" }, { keep: true });
    await cache.writeCached({ deviceId: "dev-2", entityId: "run-1", kind: "status" }, { keep: true });
    await cache.evictEntity("dev-1", "run-1");
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).toBeUndefined();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "show", sub: "abc123" })).toBeUndefined();
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-12", kind: "status" })).value.keep).toBe(true);
    expect((await cache.readCached({ deviceId: "dev-2", entityId: "run-1", kind: "status" })).value.keep).toBe(true);
  });

  it("wipes everything on request", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.wipeCache();
    expect(await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).toBeUndefined();
  });
});

describe("a browser without IndexedDB", () => {
  it("stays silent: reads answer undefined, writes and evictions do not throw", async () => {
    vi.resetModules();
    delete globalThis.indexedDB;
    const bare = await import("../src/core/localCache.js");
    await expect(bare.writeCached({ deviceId: "d", entityId: "e", kind: "status" }, {})).resolves.toBeUndefined();
    // Nothing is held in memory: with no store there is no cache, and every
    // read answers "never seen" rather than this session's own writes.
    expect(await bare.readCached({ deviceId: "d", entityId: "e", kind: "status" })).toBeUndefined();
    expect(await bare.readCachedMany([{ deviceId: "d", entityId: "e", kind: "status" }])).toEqual([undefined]);
    await expect(bare.evictEntity("d", "e")).resolves.toBeUndefined();
  });

  it("announces nothing, because nothing changed", async () => {
    vi.resetModules();
    delete globalThis.indexedDB;
    const bare = await import("../src/core/localCache.js");
    const heard = [];
    bare.subscribeCache({ deviceId: "d" }, (changed) => heard.push(changed));
    // A surface told its record changed re-reads it and finds nothing there,
    // and blanks what it was correctly painting a frame earlier.
    await bare.writeCached({ deviceId: "d", entityId: "e", kind: "status" }, { head: "abc" });
    await bare.deleteCached([{ deviceId: "d", entityId: "e", kind: "status" }]);
    await bare.evictEntity("d", "e");
    await bare.wipeCache();
    expect(heard).toEqual([]);
  });
});

describe("a write that fails", () => {
  it("says nothing changed, rather than sending a subscriber to re-read it", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "abc" });
    const heard = [];
    cache.subscribeCache({ deviceId: "dev-1" }, (changed) => heard.push(changed));
    // A value IndexedDB cannot store: the put throws inside the transaction.
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, () => "not storable");
    expect(heard).toEqual([]);
    // That write failed and nothing else did: what was stored still reads.
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }))?.value).toEqual({ head: "abc" });
    expect(cache.cacheHealth().state).toBe("ready");
  });
});

describe("a suspended browser whose IndexedDB connection closes", () => {
  it("retries a transient put failure inside an atomic merge", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    await cache.writeCached(address, { count: 1 });
    const heard = [];
    cache.subscribeCache(address, (changed) => heard.push(changed));
    const originalPut = IDBObjectStore.prototype.put;
    let failed = false;
    const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw new DOMException("connection closed", "InvalidStateError");
      }
      return originalPut.apply(this, args);
    });
    try {
      expect(await cache.mergeCachedAtomically(address, (previous) => ({ count: previous.count + 1 }))).toBe(true);
      expect(put).toHaveBeenCalledTimes(2);
      expect((await cache.readCached(address))?.value).toEqual({ count: 2 });
      expect(heard).toHaveLength(1);
      expect(await cache.cacheAvailable()).toBe(true);
      await cache.writeCached(address, { count: 3 });
      expect((await cache.readCached(address))?.value).toEqual({ count: 3 });
      expect(heard).toHaveLength(2);
    } finally {
      put.mockRestore();
    }
  });

  it("retries a transient put failure inside a feed update", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "feed" };
    await cache.writeCached(address, { items: [], runs: [], count: 1 });
    const originalPut = IDBObjectStore.prototype.put;
    let failed = false;
    const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw new DOMException("connection closed", "InvalidStateError");
      }
      return originalPut.apply(this, args);
    });
    try {
      expect(await cache.updateCachedFeed(address, (previous) => ({ ...previous, count: previous.count + 1 }))).toBe(true);
      expect((await cache.readCached(address))?.value.count).toBe(2);
    } finally {
      put.mockRestore();
    }
  });

  it("retries a transient put failure without bypassing a bridge generation", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "bridge-update" };
    await cache.writeCached(address, { count: 1 });
    const generation = (await cache.readCached(address)).generation;
    const originalPut = IDBObjectStore.prototype.put;
    let failed = false;
    const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw new DOMException("connection closed", "InvalidStateError");
      }
      return originalPut.apply(this, args);
    });
    try {
      expect(await cache.writeCachedIfGeneration(address, { count: 2 }, generation)).toBe(true);
      expect((await cache.readCached(address))?.value).toEqual({ count: 2 });
      expect(await cache.writeCachedIfGeneration(address, { count: 3 }, generation)).toBe(false);
      expect((await cache.readCached(address))?.value).toEqual({ count: 2 });
    } finally {
      put.mockRestore();
    }
  });

  it("fails a caller's merge error alone, even when it resembles a connection error", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    await cache.writeCached(address, { count: 1 });
    let calls = 0;
    expect(await cache.mergeCachedAtomically(address, () => {
      calls += 1;
      throw new DOMException("caller failed", "InvalidStateError");
    })).toBe(false);
    // Not retried as a cache fault, and not a reason to stop answering.
    expect(calls).toBe(1);
    expect((await cache.readCached(address))?.value).toEqual({ count: 1 });
    expect(cache.cacheHealth().state).toBe("ready");
  });

  it("refuses an uncloneable record from an atomic merge without standing down", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    await cache.writeCached(address, { count: 1 });
    const heard = [];
    cache.subscribeCache(address, (changed) => heard.push(changed));
    expect(await cache.mergeCachedAtomically(address, () => ({ uncloneable: () => {} }))).toBe(false);
    expect(heard).toEqual([]);
    expect((await cache.readCached(address))?.value).toEqual({ count: 1 });
    expect(cache.cacheHealth().state).toBe("ready");
  });

  it("bounds repeated storage put failures inside a merge, failing that write alone", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    await cache.writeCached(address, { count: 1 });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const put = vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => {
      throw new DOMException("connection closed", "InvalidStateError");
    });
    try {
      expect(await cache.mergeCachedAtomically(address, (previous) => ({ count: previous.count + 1 }))).toBe(false);
      expect(put).toHaveBeenCalledTimes(FAST_RECOVERY.reopenDelaysMs.length);
      expect(warn).not.toHaveBeenCalled();
    } finally {
      put.mockRestore();
      warn.mockRestore();
    }
    // The cache never stood down: the record reads, and the next write lands.
    expect((await cache.readCached(address))?.value).toEqual({ count: 1 });
    expect(await cache.mergeCachedAtomically(address, (previous) => ({ count: previous.count + 1 }))).toBe(true);
    expect(cache.cacheHealth().state).toBe("ready");
  });

  it("reopens the store and keeps records readable after wake", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    let openedDb;
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      openedDb = this;
      return originalTransaction.apply(this, args);
    });
    try {
      await cache.writeCached(address, { head: "before-sleep" });
      expect(openedDb).toBeDefined();
      forceCloseDatabase(openedDb);
      expect((await cache.readCached(address))?.value).toEqual({ head: "before-sleep" });
      await cache.writeCached(address, { head: "after-wake" });
      expect((await cache.readCached(address))?.value).toEqual({ head: "after-wake" });
    } finally {
      transaction.mockRestore();
    }
  });

  it("recovers a closed handle before its close event arrives", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    const handles = [];
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      handles.push(this);
      return originalTransaction.apply(this, args);
    });
    try {
      await cache.writeCached(address, { head: "stored" });
      handles[0].close(); // close() leaves the retained handle without an onclose event
      expect((await cache.readCached(address))?.value).toEqual({ head: "stored" });
      expect(new Set(handles).size).toBe(2);
    } finally {
      transaction.mockRestore();
    }
  });

  it("lets concurrent reads recover from the same closed handle", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    const handles = [];
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      handles.push(this);
      return originalTransaction.apply(this, args);
    });
    try {
      await cache.writeCached(address, { head: "stored" });
      handles[0].close();
      const records = await Promise.all([cache.readCached(address), cache.readCached(address)]);
      expect(records.map((record) => record?.value)).toEqual([{ head: "stored" }, { head: "stored" }]);
      expect(new Set(handles).size).toBe(2);
      await cache.writeCached(address, { head: "later" });
      expect((await cache.readCached(address))?.value).toEqual({ head: "later" });
    } finally {
      transaction.mockRestore();
    }
  });

  it("retries an aborted write and announces only its committed result", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    await cache.writeCached(address, { head: "before" });
    const heard = [];
    cache.subscribeCache(address, (changed) => heard.push(changed));
    const originalTransaction = IDBDatabase.prototype.transaction;
    let aborted = false;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      const opened = originalTransaction.apply(this, args);
      if (args[1] === "readwrite" && !aborted) {
        aborted = true;
        queueMicrotask(() => opened.abort());
      }
      return opened;
    });
    try {
      await cache.writeCached(address, { head: "after" });
      expect(aborted).toBe(true);
      expect((await cache.readCached(address))?.value).toEqual({ head: "after" });
      expect(heard).toHaveLength(1);
    } finally {
      transaction.mockRestore();
    }
  });

  it("does not let an old pending transaction's abort invalidate a reopened handle", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    const handles = [];
    await cache.writeCached(address, { head: "stored" });
    const open = vi.spyOn(indexedDB, "open");
    let oldTransaction;
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      handles.push(this);
      const opened = originalTransaction.apply(this, args);
      if (!oldTransaction) {
        oldTransaction = opened;
        // Keep a real fake-indexeddb transaction active until after the next
        // connection has opened, so its abort event arrives genuinely late.
        opened._start = () => {};
      }
      return opened;
    });
    try {
      const pendingRead = cache.readCached(address);
      await vi.waitFor(() => expect(oldTransaction).toBeDefined());
      const oldHandle = handles[0];
      oldHandle.onclose(new Event("close"));
      expect(await cache.cacheAvailable()).toBe(true);
      expect(open).toHaveBeenCalledTimes(1);
      oldTransaction.abort();
      expect((await pendingRead)?.value).toEqual({ head: "stored" });
      const currentHandle = handles.at(-1);
      expect(currentHandle).not.toBe(oldHandle);
      expect(open).toHaveBeenCalledTimes(1);
      await cache.writeCached(address, { head: "after" });
      expect(handles.at(-1)).toBe(currentHandle);
      expect((await cache.readCached(address))?.value).toEqual({ head: "after" });
    } finally {
      transaction.mockRestore();
      open.mockRestore();
    }
  });

  it("retries a transient WebKit connection-loss error", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    await cache.writeCached(address, { head: "stored" });
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failed = false;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw new DOMException("Connection to Indexed Database server lost", "UnknownError");
      }
      return originalTransaction.apply(this, args);
    });
    try {
      expect((await cache.readCached(address))?.value).toEqual({ head: "stored" });
      expect(transaction).toHaveBeenCalledTimes(2);
    } finally {
      transaction.mockRestore();
    }
  });

  it("retries a transient connection error while opening the database", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    const originalOpen = indexedDB.open.bind(indexedDB);
    let failed = false;
    const open = vi.spyOn(indexedDB, "open").mockImplementation((...args) => {
      if (!failed) {
        failed = true;
        throw new DOMException("Connection to Indexed Database server lost", "UnknownError");
      }
      return originalOpen(...args);
    });
    try {
      await cache.writeCached(address, { head: "stored" });
      expect(open).toHaveBeenCalledTimes(2);
      expect((await cache.readCached(address))?.value).toEqual({ head: "stored" });
    } finally {
      open.mockRestore();
    }
  });

  it("keeps a read waiting through persistent connection failures, never answering it with nothing", async () => {
    const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };
    await cache.writeCached(address, { head: "stored" });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failing = true;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (failing) throw new DOMException("connection closed", "InvalidStateError");
      return originalTransaction.apply(this, args);
    });
    try {
      let answered = false;
      const read = cache.readCached(address).then((record) => {
        answered = true;
        return record;
      });
      // Past a whole round of attempts and into the next: still no answer.
      await vi.waitFor(() => expect(transaction.mock.calls.length).toBeGreaterThan(FAST_RECOVERY.reopenDelaysMs.length));
      expect(answered).toBe(false);
      expect(cache.cacheHealth()).toMatchObject({ state: "recovering", error: "InvalidStateError" });
      failing = false;
      expect((await read)?.value).toEqual({ head: "stored" });
      expect(cache.cacheHealth().state).toBe("ready");
      expect(warn).not.toHaveBeenCalled();
    } finally {
      transaction.mockRestore();
      warn.mockRestore();
    }
  });
});

describe("the format version", () => {
  // v2 is as stale as v1 here: it has the records but not the write-time
  // index the lifetime sweeps walk, and a store missing an index a reader
  // asks for would stand the cache down for good.
  for (const version of [1, 2]) {
    it(`clears a database v${version} wrote, rather than reading its shapes`, async () => {
      vi.resetModules();
      freshFactory();
      await seedVersion(version, "dev-1|run-1|status|", { at: 1, value: { head: `from v${version}` } });
      const upgraded = await import("../src/core/localCache.js");
      expect(await upgraded.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).toBeUndefined();
      // And the store is usable afterwards: a cold start, not a broken cache.
      await upgraded.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "now" });
      expect((await upgraded.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).value.head).toBe("now");
      // Including the sweeps, which need the index the old format lacks.
      expect(await upgraded.cachedAddressesWrittenBefore({ deviceId: "dev-1", entityId: "run-1" }, Date.now() + 1))
        .toEqual([{ deviceId: "dev-1", entityId: "run-1", kind: "status", sub: "" }]);
    });
  }
});

describe("readCachedMany", () => {
  it("answers one record per address, in the order asked, undefined for what is missing", async () => {
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "abc" });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, ["a.js"]);
    const records = await cache.readCachedMany([
      { deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" },
      { deviceId: "dev-1", entityId: "run-1", kind: "log" },
      { deviceId: "dev-1", entityId: "run-1", kind: "status" },
    ]);
    expect(records.map((record) => record?.value)).toEqual([["a.js"], undefined, { head: "abc" }]);
  });

  it("answers nothing for no addresses", async () => {
    expect(await cache.readCachedMany([])).toEqual([]);
  });
});

describe("records merged together", () => {
  const list = { deviceId: "dev-1", entityId: "p1", kind: "tracker-issues" };
  const note = { deviceId: "dev-1", entityId: "p1", kind: "tracker-issue-reads", sub: "list" };
  const other = { deviceId: "dev-1", entityId: "p1", kind: "status" };

  it("hands the merge every value as held and writes and announces what it changed", async () => {
    await cache.writeCached(list, { count: 1 });
    await cache.writeCached(other, { head: "abc" });
    const heard = [];
    cache.subscribeCache({ deviceId: "dev-1" }, (changed) => heard.push(changed.kind));
    let seen;
    const wrote = await cache.mergeCachedTogether([list, note, other], (values) => {
      seen = values;
      return [{ count: values[0].count + 1 }, { reads: 1 }, null];
    });
    expect(wrote).toBe(true);
    expect(seen).toEqual([{ count: 1 }, undefined, { head: "abc" }]);
    const records = await cache.readCachedMany([list, note, other]);
    expect(records.map((record) => record?.value)).toEqual([{ count: 2 }, { reads: 1 }, { head: "abc" }]);
    expect(heard).toEqual(["tracker-issues", "tracker-issue-reads"]);
  });

  it("writes none of them when one cannot be written", async () => {
    await cache.writeCached(list, { count: 1 });
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    try {
      expect(await cache.mergeCachedTogether([list, note], () => [{ count: 2 }, { uncloneable: () => {} }])).toBe(false);
    } finally {
      warn.mockRestore();
    }
    vi.resetModules();
    const reopened = await import("../src/core/localCache.js");
    expect((await reopened.readCached(list))?.value).toEqual({ count: 1 });
    expect(await reopened.readCached(note)).toBeUndefined();
  });
});

describe("a count every tab shares", () => {
  const COUNT = { deviceId: "", entityId: "", kind: "count" };

  it("answers one more each time it is taken, in whichever tab takes it", async () => {
    expect(await cache.takeCachedCount(COUNT)).toBe(1);
    vi.resetModules();
    const otherTab = await import("../src/core/localCache.js");
    expect(await otherTab.takeCachedCount(COUNT)).toBe(2);
    expect(await cache.takeCachedCount(COUNT)).toBe(3);
  });

  it("never answers under its floor, nor under what it held", async () => {
    expect(await cache.takeCachedCount(COUNT, 100)).toBe(100);
    expect(await cache.takeCachedCount(COUNT, 50)).toBe(101);
  });

  it("is not announced", async () => {
    const heard = [];
    cache.subscribeCache({}, (changed) => heard.push(changed));
    await cache.takeCachedCount(COUNT);
    expect(heard).toEqual([]);
  });

  it("answers nothing where there is no cache to count in", async () => {
    delete globalThis.indexedDB;
    vi.resetModules();
    const without = await import("../src/core/localCache.js");
    expect(await without.takeCachedCount(COUNT)).toBeUndefined();
  });
});

describe("announcements", () => {
  /** Resolve on the subscriber's first call, or reject if nothing arrives —
   *  the timer also keeps the loop alive while a channel message is in
   *  flight. */
  const announced = (address, { within = 2000 } = {}) =>
    new Promise((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error("nothing was announced")), within);
      cache.subscribeCache(address, (changed) => {
        clearTimeout(timer);
        resolve(changed);
      });
    });

  it("tells a subscriber on the entity which address was written", async () => {
    const heard = announced({ deviceId: "dev-1", entityId: "run-1" });
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "abc" });
    expect(await heard).toEqual({ deviceId: "dev-1", entityId: "run-1", kind: "status", sub: "" });
  });

  it("announces only to the listeners the address is under", async () => {
    const seen = [];
    cache.subscribeCache({ deviceId: "dev-1" }, (changed) => seen.push(["device", changed.kind]));
    cache.subscribeCache({ deviceId: "dev-1", entityId: "run-1", kind: "tree" }, (changed) => seen.push(["tree", changed.sub]));
    cache.subscribeCache({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src" }, () => seen.push(["one-tree"]));
    cache.subscribeCache({ deviceId: "dev-2" }, () => seen.push(["other-device"]));
    cache.subscribeCache({ deviceId: "dev-1", entityId: "run-2" }, () => seen.push(["other-entity"]));
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: "src-two" }, []);
    expect(seen).toEqual([["device", "tree"], ["tree", "src-two"]]);
  });

  it("announces an address that reads the record back, whatever the writer passed", async () => {
    const heard = [];
    cache.subscribeCache({ deviceId: "dev-1" }, (changed) => heard.push(changed));
    // A caller that names `sub` as null rather than leaving it out: whatever
    // key that stores under, that is the key a listener must be sent to. An
    // announcement of some other reading of the address sends every listener
    // under it to re-read a record that is not there, and leaves the one that
    // did change unannounced.
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "tree", sub: null }, ["x"]);
    expect(heard).toHaveLength(1);
    expect((await cache.readCached(heard[0])).value).toEqual(["x"]);
  });

  it("announces an eviction to everyone holding any of that entity", async () => {
    const heard = announced({ deviceId: "dev-1", entityId: "run-1", kind: "status" });
    await cache.evictEntity("dev-1", "run-1");
    expect(await heard).toEqual({ deviceId: "dev-1", entityId: "run-1" });
  });

  it("stops announcing once the subscriber lets go", async () => {
    const seen = [];
    const unsubscribe = cache.subscribeCache({ deviceId: "dev-1" }, (changed) => seen.push(changed.kind));
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    unsubscribe();
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "log" }, {});
    expect(seen).toEqual(["status"]);
  });

  it("keeps one throwing subscriber from stopping the next one, or the write", async () => {
    const seen = [];
    cache.subscribeCache({ deviceId: "dev-1" }, () => {
      throw new Error("a view that is already unmounted");
    });
    cache.subscribeCache({ deviceId: "dev-1" }, (changed) => seen.push(changed.kind));
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "abc" });
    expect(seen).toEqual(["status"]);
    expect((await cache.readCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" })).value.head).toBe("abc");
  });

  it("hears another tab's write over the channel and re-reads", async () => {
    // The other tab writes the record into the shared database, then says so.
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, { head: "elsewhere" });
    // Only now does this tab listen. Subscribing before that write would let
    // its own local announcement satisfy both assertions, and the test would
    // pass with the inbound channel handler deleted.
    const heard = announced({ deviceId: "dev-1", entityId: "run-1" });
    const otherTab = new BroadcastChannel("build-cache");
    otherTab.postMessage({ key: "dev-1|run-1|status|" });
    const changed = await heard;
    otherTab.close();
    expect(changed).toEqual({ deviceId: "dev-1", entityId: "run-1", kind: "status", sub: "" });
    expect((await cache.readCached(changed)).value.head).toBe("elsewhere");
  });

  it("tells every subscriber, on every tab, that a wipe took the lot", async () => {
    const heard = announced({ deviceId: "dev-1", entityId: "run-1", kind: "status" });
    const posted = [];
    const otherTab = new BroadcastChannel("build-cache");
    otherTab.onmessage = (event) => posted.push(event.data);
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.wipeCache();
    await heard;
    await new Promise((resolve) => setTimeout(resolve, 20));
    otherTab.close();
    expect(posted).toEqual([{ key: "dev-1|run-1|status|" }, { key: "" }]);
  });

  it("tells the other tabs what it wrote, and does not echo its own message back", async () => {
    const posted = [];
    const otherTab = new BroadcastChannel("build-cache");
    otherTab.onmessage = (event) => posted.push(event.data);
    const echoed = [];
    cache.subscribeCache({ deviceId: "dev-1" }, () => echoed.push(1));
    await cache.writeCached({ deviceId: "dev-1", entityId: "run-1", kind: "status" }, {});
    await cache.evictEntity("dev-1", "run-1");
    await new Promise((resolve) => setTimeout(resolve, 20));
    otherTab.close();
    expect(posted).toEqual([{ key: "dev-1|run-1|status|" }, { key: "dev-1|run-1" }]);
    expect(echoed).toHaveLength(2); // the two local writes, not four
  });
});
