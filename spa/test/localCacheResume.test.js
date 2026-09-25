// #169: on iOS every surface went blank after the app resumed. WebKit drops a
// suspended page's IndexedDB connection, the first opens and transactions
// after the resume fail with "Connection to Indexed Database server lost", and
// the cache — which retried once — stood down for the session. These drive
// that failure through fake-indexeddb: the cache rides it out, stands down
// only for what no reopen can fix, and records each step where a phone can
// show it.

import { describe, it, expect, beforeEach, vi } from "vitest";
import { IDBDatabase, IDBFactory, IDBKeyRange, IDBObjectStore, forceCloseDatabase } from "fake-indexeddb";

const FAST_RECOVERY = { reopenDelaysMs: [0, 1, 2, 3], openTimeoutMs: 200, restMs: 40 };
const address = { deviceId: "dev-1", entityId: "run-1", kind: "status" };

let cache;
let diagnostics;

beforeEach(async () => {
  vi.restoreAllMocks();
  vi.resetModules();
  globalThis.indexedDB = new IDBFactory();
  globalThis.IDBKeyRange = IDBKeyRange;
  cache = await import("../src/core/localCache.js");
  diagnostics = await import("../src/core/connectionDiagnostics.js");
  cache.setCacheRecoveryTiming(FAST_RECOVERY);
  vi.spyOn(console, "info").mockImplementation(() => {});
});

const cacheEvents = () =>
  diagnostics.connectionDiagnosticHistory().filter((entry) => entry.connection === cache.CACHE_DIAGNOSTIC);
const eventNames = () => cacheEvents().map((entry) => entry.event);

const connectionLost = () => new DOMException("Connection to Indexed Database server lost. Refresh the page to try again", "UnknownError");

/** An open request that fails the way WebKit's does: asynchronously, through
 *  `onerror`, with the error on the request. */
function failingOpenRequest(error) {
  const request = { error, result: undefined };
  setTimeout(() => request.onerror?.({ preventDefault() {} }));
  return request;
}

/** `indexedDB.open` failing its first `times` calls with `error`. */
function failOpens(times, error = connectionLost()) {
  const originalOpen = indexedDB.open.bind(indexedDB);
  let failures = 0;
  return vi.spyOn(indexedDB, "open").mockImplementation((...args) => {
    if (failures < times) {
      failures += 1;
      return failingOpenRequest(error);
    }
    return originalOpen(...args);
  });
}

describe("a connection lost across a resume", () => {
  it("reopens through several failed opens without standing down", async () => {
    const handles = [];
    const originalTransaction = IDBDatabase.prototype.transaction;
    const transaction = vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      handles.push(this);
      return originalTransaction.apply(this, args);
    });
    await cache.writeCached(address, { head: "before-sleep" });
    transaction.mockRestore();
    // The page is suspended: WebKit closes the connection, and the next few
    // opens after the resume find its storage server gone.
    forceCloseDatabase(handles[0]);
    const open = failOpens(3);
    const warn = vi.spyOn(console, "warn");

    expect((await cache.readCached(address))?.value).toEqual({ head: "before-sleep" });
    expect(open).toHaveBeenCalledTimes(4);
    expect(cache.cacheHealth().state).toBe("ready");
    expect(warn).not.toHaveBeenCalled();
    expect(eventNames()).toEqual(["cache-connection-closed", "cache-connection-lost", "cache-recovered"]);
    expect(cacheEvents()[1]).toMatchObject({
      reason: "open-failed",
      error: "UnknownError",
      message: expect.stringContaining("Connection to Indexed Database server lost"),
      visibility: "unknown",
      sinceShownMs: null,
    });
    expect(cacheEvents()[2]).toMatchObject({ attempts: 3, afterMs: expect.any(Number) });
  });

  it("retries a transaction the lost connection keeps failing, and announces only the commit", async () => {
    await cache.writeCached(address, { head: "before" });
    const heard = [];
    cache.subscribeCache(address, (changed) => heard.push(changed));
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failures = 0;
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (failures < 3) {
        failures += 1;
        throw connectionLost();
      }
      return originalTransaction.apply(this, args);
    });

    await cache.writeCached(address, { head: "after" });

    expect(failures).toBe(3);
    expect(heard).toHaveLength(1);
    expect((await cache.readCached(address))?.value).toEqual({ head: "after" });
    expect(eventNames()).toEqual(["cache-connection-lost", "cache-recovered"]);
    expect(cacheEvents()[0]).toMatchObject({ reason: "transaction-failed", error: "UnknownError" });
  });

  it("rides out the error real WebKit raised when its storage process died under a transaction", async () => {
    // Playwright WebKit, the network process killed with reads and writes in
    // flight: the old cache matched only "connection … lost" and stood down on
    // this at once, answering nothing to every later read.
    await cache.writeCached(address, { head: "stored" });
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failed = false;
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw new DOMException("An internal error was encountered in the Indexed Database server", "UnknownError");
      }
      return originalTransaction.apply(this, args);
    });
    expect((await cache.readCached(address))?.value).toEqual({ head: "stored" });
    expect(cache.cacheHealth().state).toBe("ready");
    expect(eventNames()).toEqual(["cache-connection-lost", "cache-recovered"]);
  });

  it("rests when every open fails, keeps readers waiting, and answers them when the rest is over", async () => {
    await cache.writeCached(address, { head: "stored" });
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failed = false;
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw connectionLost();
      }
      return originalTransaction.apply(this, args);
    });
    const open = failOpens(FAST_RECOVERY.reopenDelaysMs.length + 2);
    let answered = false;
    const read = cache.readCached(address).then((record) => {
      answered = true;
      return record;
    });

    await vi.waitFor(() => expect(cache.cacheHealth().state).toBe("resting"));
    expect(answered).toBe(false);
    // Another reader asking during the rest waits with the first.
    const second = cache.readCached(address);

    expect((await read)?.value).toEqual({ head: "stored" });
    expect((await second)?.value).toEqual({ head: "stored" });
    expect(open.mock.calls.length).toBe(FAST_RECOVERY.reopenDelaysMs.length + 3);
    expect(eventNames()).toEqual(["cache-connection-lost", "cache-resting", "cache-recovered"]);
    expect(cacheEvents()[1]).toMatchObject({ forMs: FAST_RECOVERY.restMs, error: "UnknownError" });
  });

  it("makes a write asked for while the cache rests once the database is back", async () => {
    await cache.writeCached(address, { head: "before" });
    const heard = [];
    cache.subscribeCache(address, (changed) => heard.push(changed));
    const originalTransaction = IDBDatabase.prototype.transaction;
    let failed = false;
    vi.spyOn(IDBDatabase.prototype, "transaction").mockImplementation(function (...args) {
      if (!failed) {
        failed = true;
        throw connectionLost();
      }
      return originalTransaction.apply(this, args);
    });
    failOpens(FAST_RECOVERY.reopenDelaysMs.length);
    const write = cache.writeCached(address, { head: "during" });
    await vi.waitFor(() => expect(cache.cacheHealth().state).toBe("resting"));
    expect(heard).toEqual([]);

    await write;

    expect(heard).toHaveLength(1);
    expect((await cache.readCached(address))?.value).toEqual({ head: "during" });
  });
});

describe("an open blocked by another tab", () => {
  it("waits for the other connection to close rather than standing down", async () => {
    // Another tab running an older build holds version 2 open and does not
    // answer `versionchange` on its own.
    const older = await new Promise((resolve) => {
      const request = indexedDB.open("build-cache", 2);
      request.onupgradeneeded = () => request.result.createObjectStore("records");
      request.onsuccess = () => resolve(request.result);
    });
    const open = vi.spyOn(indexedDB, "open");
    const write = cache.writeCached(address, { head: "after-upgrade" });

    await vi.waitFor(() => expect(eventNames()).toContain("cache-open-blocked"));
    // Past the open timeout: a blocked open is not a lost one.
    await new Promise((resolve) => setTimeout(resolve, FAST_RECOVERY.openTimeoutMs + 50));
    expect(cache.cacheHealth().state).toBe("ready");
    older.close();

    await write;
    expect((await cache.readCached(address))?.value).toEqual({ head: "after-upgrade" });
    expect(open).toHaveBeenCalledTimes(1);
    expect(eventNames()).toEqual(["cache-open-blocked"]);
  });

  it("steps aside when a newer version opens in another tab", async () => {
    await cache.writeCached(address, { head: "stored" });
    const newer = await new Promise((resolve, reject) => {
      const request = indexedDB.open("build-cache", 4);
      request.onblocked = () => reject(new Error("the cache's connection blocked the newer version"));
      request.onsuccess = () => resolve(request.result);
    });
    newer.close();
    expect(eventNames()).toEqual(["cache-yielded"]);
  });
});

describe("what a reopen cannot fix", () => {
  it("stands down for a private window refusing IndexedDB", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    const open = vi.spyOn(indexedDB, "open").mockImplementation(() =>
      failingOpenRequest(new DOMException("A mutation operation was attempted on a database that did not allow mutations.", "InvalidStateError")));
    const heard = [];
    cache.subscribeCache({ deviceId: "dev-1" }, (changed) => heard.push(changed));

    expect(await cache.readCached(address)).toBeUndefined();
    await cache.writeCached(address, { head: "never" });

    expect(open).toHaveBeenCalledTimes(1);
    expect(heard).toEqual([]);
    expect(cache.cacheHealth()).toMatchObject({ state: "stood-down", reason: "open-failed", error: "InvalidStateError" });
    expect(warn).toHaveBeenCalledTimes(1);
    expect(eventNames()).toEqual(["cache-stood-down"]);
  });

  it("stands down for a browser that forbids storage outright", async () => {
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(indexedDB, "open").mockImplementation(() => {
      throw new DOMException("The operation is insecure.", "SecurityError");
    });
    expect(await cache.readCached(address)).toBeUndefined();
    expect(cache.cacheHealth()).toMatchObject({ state: "stood-down", error: "SecurityError" });
  });

  it("stands down when the quota is full", async () => {
    await cache.writeCached(address, { head: "stored" });
    vi.spyOn(console, "warn").mockImplementation(() => {});
    vi.spyOn(IDBObjectStore.prototype, "put").mockImplementation(() => {
      throw new DOMException("The quota has been exceeded.", "QuotaExceededError");
    });
    expect(await cache.writeCached(address, { head: "too-big" })).toBeUndefined();
    expect(cache.cacheHealth()).toMatchObject({ state: "stood-down", reason: "transaction-failed", error: "QuotaExceededError" });
    expect(await cache.readCached(address)).toBeUndefined();
  });
});

describe("the record of it", () => {
  it("says a refused write was refused, and leaves the cache ready", async () => {
    await cache.writeCached(address, () => "a function cannot be stored");
    expect(eventNames()).toEqual(["cache-write-refused"]);
    expect(cacheEvents()[0]).toMatchObject({ error: "DataCloneError" });
    expect(cache.cacheHealth().state).toBe("ready");
  });

  it("answers absent where there is no IndexedDB at all", async () => {
    vi.resetModules();
    delete globalThis.indexedDB;
    const bare = await import("../src/core/localCache.js");
    expect(bare.cacheHealth()).toEqual({ state: "absent" });
  });
});
